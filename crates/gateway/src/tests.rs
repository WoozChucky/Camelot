//! The gateway driven with crafted Ethernet frames, as a guest's card would send
//! them: ARP, DHCP, ICMP, DNS, UDP and TCP through NAT to sockets on 127.0.0.1,
//! forwarded ports, the timers and limits, and broken frames (dropped, never a panic).
//!
//! Nothing here needs the internet: the "remote" ends are sockets on 127.0.0.1,
//! reached through 10.0.2.2, and DNS uses a resolver made up for the test. Waiting for
//! the host side polls with a deadline; timers are driven with made-up instants.

use super::*;
use std::io::{Read, Write};
use std::net::{TcpListener, UdpSocket};
use wire::*;

const GUEST_MAC: Mac = DEFAULT_GUEST_MAC;
const GUEST: Ipv4Addr = FIRST_LEASE;
/// How long a test waits for the host side before failing.
const DEADLINE: Duration = Duration::from_secs(10);

fn limits() -> Limits {
    Limits {
        service_interval: Duration::ZERO,
        ..Limits::default()
    }
}

fn gateway() -> Gateway {
    gateway_with(Config {
        limits: limits(),
        resolver: Arc::new(|name: &str| match name {
            "camelot.test" => Ok(vec![Ipv4Addr::new(192, 0, 2, 7), Ipv4Addr::new(192, 0, 2, 8)]),
            _ => Err(io::Error::new(io::ErrorKind::NotFound, "no such name")),
        }),
        ..Config::default()
    })
}

fn gateway_with(config: Config) -> Gateway {
    let mut gw = Gateway::new(config).unwrap();
    // The guest announces itself (as DHCP or ARP would), so frames for it need no ARP.
    gw.send(&arp_frame(ARP_REQUEST, GATEWAY_ADDR));
    assert!(gw.recv().is_some(), "ARP reply");
    gw
}

fn arp_frame(op: u16, target: Ipv4Addr) -> Vec<u8> {
    let arp = arp_packet(&Arp {
        op,
        sender_mac: GUEST_MAC,
        sender_ip: GUEST,
        target_mac: if op == ARP_REPLY { GATEWAY_MAC } else { [0; 6] },
        target_ip: target,
    });
    eth_frame(BROADCAST_MAC, GUEST_MAC, ETHERTYPE_ARP, &arp)
}

fn ip_frame(src: Ipv4Addr, dst: Ipv4Addr, proto: u8, payload: &[u8]) -> Vec<u8> {
    eth_frame(
        GATEWAY_MAC,
        GUEST_MAC,
        ETHERTYPE_IPV4,
        &ipv4_packet(src, dst, proto, 1, payload),
    )
}

fn udp_frame(src_port: u16, dst: SocketAddrV4, payload: &[u8]) -> Vec<u8> {
    let d = udp_datagram(GUEST, *dst.ip(), src_port, dst.port(), payload);
    ip_frame(GUEST, *dst.ip(), PROTO_UDP, &d)
}

fn tcp_frame(src_port: u16, dst: SocketAddrV4, flags: u8, seq: u32, ack: u32, payload: &[u8]) -> Vec<u8> {
    let h = TcpHeader {
        src_port,
        dst_port: dst.port(),
        seq,
        ack,
        flags,
        window: 8192,
        mss: (flags & SYN != 0).then_some(1000),
    };
    ip_frame(
        GUEST,
        *dst.ip(),
        PROTO_TCP,
        &tcp_segment(GUEST, *dst.ip(), &h, payload),
    )
}

/// A frame for the guest, taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Out {
    Arp(Arp),
    Udp {
        src: SocketAddrV4,
        dst: SocketAddrV4,
        payload: Vec<u8>,
    },
    Tcp(Seg),
    Icmp {
        src: Ipv4Addr,
        echo: (u8, u16, u16, Vec<u8>),
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Seg {
    src: SocketAddrV4,
    dst: SocketAddrV4,
    flags: u8,
    seq: u32,
    ack: u32,
    window: u16,
    mss: Option<u16>,
    payload: Vec<u8>,
}

fn decode(frame: &[u8]) -> Out {
    let eth = parse_eth(frame).expect("an Ethernet frame");
    assert_eq!(eth.src, GATEWAY_MAC);
    if eth.ethertype == ETHERTYPE_ARP {
        return Out::Arp(parse_arp(eth.payload).expect("ARP"));
    }
    let ip = parse_ipv4(eth.payload).expect("a good IPv4 packet");
    match ip.proto {
        PROTO_UDP => {
            let u = parse_udp(&ip).expect("a good UDP datagram");
            Out::Udp {
                src: SocketAddrV4::new(ip.src, u.src_port),
                dst: SocketAddrV4::new(ip.dst, u.dst_port),
                payload: u.payload.to_vec(),
            }
        }
        PROTO_TCP => {
            assert_eq!(eth.dst, GUEST_MAC);
            let t = parse_tcp(&ip).expect("a good TCP segment");
            Out::Tcp(Seg {
                src: SocketAddrV4::new(ip.src, t.src_port),
                dst: SocketAddrV4::new(ip.dst, t.dst_port),
                flags: t.flags,
                seq: t.seq,
                ack: t.ack,
                window: t.window,
                mss: t.mss,
                payload: t.payload.to_vec(),
            })
        }
        PROTO_ICMP => {
            let e = parse_echo(ip.payload).expect("a good echo");
            Out::Icmp {
                src: ip.src,
                echo: (e.kind, e.ident, e.seq, e.data.to_vec()),
            }
        }
        p => panic!("protocol {p}"),
    }
}

/// Every frame the gateway has for the guest now.
fn drain(gw: &mut Gateway) -> Vec<Out> {
    std::iter::from_fn(|| gw.recv()).map(|f| decode(&f)).collect()
}

/// Polls the gateway until it gives a frame `pick` accepts, which it returns; frames
/// before it are returned too, in order.
fn wait<T>(gw: &mut Gateway, mut pick: impl FnMut(&Out) -> Option<T>) -> (T, Vec<Out>) {
    let start = Instant::now();
    let mut before = Vec::new();
    loop {
        while let Some(f) = gw.recv() {
            let out = decode(&f);
            if let Some(t) = pick(&out) {
                return (t, before);
            }
            before.push(out);
        }
        assert!(start.elapsed() < DEADLINE, "nothing came; got {before:?}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn tcp_seg(out: &Out) -> Option<Seg> {
    match out {
        Out::Tcp(s) => Some(s.clone()),
        _ => None,
    }
}

/// Polls a host-side condition with a deadline.
fn until<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(t) = f() {
            return t;
        }
        assert!(start.elapsed() < DEADLINE, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn host_side(port: u16) -> SocketAddrV4 {
    SocketAddrV4::new(GATEWAY_ADDR, port)
}

#[test]
fn arp_answers_for_the_gateway_and_dns_only() {
    let mut gw = Gateway::new(Config::default()).unwrap();
    for target in [GATEWAY_ADDR, DNS_ADDR] {
        gw.send(&arp_frame(ARP_REQUEST, target));
        let frame = gw.recv().unwrap();
        assert_eq!(&frame[..6], &GUEST_MAC);
        assert_eq!(
            decode(&frame),
            Out::Arp(Arp {
                op: ARP_REPLY,
                sender_mac: GATEWAY_MAC,
                sender_ip: target,
                target_mac: GUEST_MAC,
                target_ip: GUEST,
            })
        );
    }
    // Another guest's address, or a reply: nothing to say.
    gw.send(&arp_frame(ARP_REQUEST, Ipv4Addr::new(10, 0, 2, 16)));
    gw.send(&arp_frame(ARP_REPLY, GATEWAY_ADDR));
    assert_eq!(drain(&mut gw), []);
    // Truncated ARP, or not for IPv4 over Ethernet.
    let good = arp_frame(ARP_REQUEST, GATEWAY_ADDR);
    gw.send(&good[..good.len() - 1]);
    let mut odd = good.clone();
    odd[ETH_HEADER + 1] = 2; // hardware type
    gw.send(&odd);
    assert_eq!(drain(&mut gw), []);
    assert_eq!(gw.stats().malformed, 2);
}

#[test]
fn dhcp_leases_over_frames() {
    let mut gw = Gateway::new(Config::default()).unwrap();
    let dhcp_frame = |kind, xid, requested| {
        let m = dhcp::client_message(kind, xid, GUEST_MAC, requested);
        let d = udp_datagram(Ipv4Addr::UNSPECIFIED, Ipv4Addr::BROADCAST, 68, 67, &m);
        eth_frame(
            BROADCAST_MAC,
            GUEST_MAC,
            ETHERTYPE_IPV4,
            &ipv4_packet(Ipv4Addr::UNSPECIFIED, Ipv4Addr::BROADCAST, PROTO_UDP, 0, &d),
        )
    };
    let check = |gw: &mut Gateway, kind: u8| {
        let frame = gw.recv().expect("a DHCP reply");
        assert_eq!(&frame[..6], &GUEST_MAC, "unicast to the client");
        let Out::Udp { src, dst, payload } = decode(&frame) else {
            panic!("not UDP");
        };
        assert_eq!(
            (src, dst),
            (SocketAddrV4::new(GATEWAY_ADDR, 67), SocketAddrV4::new(GUEST, 68))
        );
        assert_eq!(ipv4_at(&payload, 16), GUEST, "yiaddr");
        let opts = dhcp::reply_options(&payload).unwrap();
        assert_eq!(opts[&53], [kind]);
        assert_eq!(opts[&3], GATEWAY_ADDR.octets());
        assert_eq!(opts[&6], DNS_ADDR.octets());
    };
    gw.send(&dhcp_frame(dhcp::DISCOVER, 77, None));
    check(&mut gw, dhcp::OFFER);
    gw.send(&dhcp_frame(dhcp::REQUEST, 77, Some(GUEST)));
    check(&mut gw, dhcp::ACK);
    assert_eq!(gw.lease_of(&GUEST_MAC), Some(GUEST));
    assert_eq!(gw.recv(), None);
}

#[test]
fn ping_the_gateway() {
    let mut gw = gateway();
    let echo = |dst, ident| {
        let m = echo_message(&Echo {
            kind: ICMP_ECHO_REQUEST,
            ident,
            seq: 3,
            data: b"are you there?",
        });
        ip_frame(GUEST, dst, PROTO_ICMP, &m)
    };
    gw.send(&echo(GATEWAY_ADDR, 1));
    gw.send(&echo(DNS_ADDR, 2));
    // ICMP to the internet is not forwarded.
    gw.send(&echo(Ipv4Addr::new(192, 0, 2, 1), 3));
    let reply = |src, ident| Out::Icmp {
        src,
        echo: (ICMP_ECHO_REPLY, ident, 3, b"are you there?".to_vec()),
    };
    assert_eq!(drain(&mut gw), [reply(GATEWAY_ADDR, 1), reply(DNS_ADDR, 2)]);
    // A bad ICMP checksum: no answer.
    let mut bad = echo(GATEWAY_ADDR, 4);
    let n = bad.len();
    bad[n - 1] ^= 1;
    gw.send(&bad);
    assert_eq!(drain(&mut gw), []);
}

#[test]
fn dns_answers_from_the_resolver() {
    let mut gw = gateway();
    let dns_server = SocketAddrV4::new(DNS_ADDR, 53);
    let answer = |out: &Out| match out {
        Out::Udp { src, dst, payload } if *src == dns_server && *dst == SocketAddrV4::new(GUEST, 5353) => {
            dns::read_answer(payload)
        }
        _ => None,
    };
    gw.send(&udp_frame(
        5353,
        dns_server,
        &dns::query(1, "camelot.test", dns::TYPE_A),
    ));
    let (got, _) = wait(&mut gw, answer);
    assert_eq!(
        got,
        (
            1,
            dns::NOERROR,
            vec![Ipv4Addr::new(192, 0, 2, 7), Ipv4Addr::new(192, 0, 2, 8)]
        )
    );
    gw.send(&udp_frame(
        5353,
        dns_server,
        &dns::query(2, "nowhere.test", dns::TYPE_A),
    ));
    assert_eq!(wait(&mut gw, answer).0, (2, dns::NXDOMAIN, vec![]));
    // AAAA is answered at once, empty.
    gw.send(&udp_frame(
        5353,
        dns_server,
        &dns::query(3, "camelot.test", dns::TYPE_AAAA),
    ));
    assert_eq!(answer(&drain(&mut gw)[0]), Some((3, dns::NOERROR, vec![])));
    // Not port 53, or garbage: nothing.
    gw.send(&udp_frame(
        5353,
        SocketAddrV4::new(DNS_ADDR, 54),
        &dns::query(4, "x", dns::TYPE_A),
    ));
    gw.send(&udp_frame(5353, dns_server, b"\x00\x05"));
    assert_eq!(drain(&mut gw), []);
}

#[test]
fn dns_with_the_host_resolver() {
    // An IP literal resolves without asking any server, so this needs no network.
    let mut gw = gateway_with(Config {
        limits: limits(),
        ..Config::default()
    });
    let dns_server = SocketAddrV4::new(DNS_ADDR, 53);
    gw.send(&udp_frame(
        1000,
        dns_server,
        &dns::query(9, "192.0.2.33", dns::TYPE_A),
    ));
    let (got, _) = wait(&mut gw, |out| match out {
        Out::Udp { payload, .. } => dns::read_answer(payload),
        _ => None,
    });
    assert_eq!(got, (9, dns::NOERROR, vec![Ipv4Addr::new(192, 0, 2, 33)]));
}

#[test]
fn udp_through_nat_to_the_host() {
    let mut gw = gateway();
    let server = UdpSocket::bind("127.0.0.1:0").unwrap();
    server.set_read_timeout(Some(DEADLINE)).unwrap();
    let port = server.local_addr().unwrap().port();
    gw.send(&udp_frame(4000, host_side(port), b"hello host"));
    let mut buf = [0; 64];
    let (n, from) = server.recv_from(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"hello host");
    assert_eq!(gw.flows(), 1);
    server.send_to(b"hello guest", from).unwrap();
    let (got, _) = wait(&mut gw, |out| match out {
        Out::Udp { src, dst, payload } => Some((*src, *dst, payload.clone())),
        _ => None,
    });
    assert_eq!(
        got,
        (
            host_side(port),
            SocketAddrV4::new(GUEST, 4000),
            b"hello guest".to_vec()
        )
    );
    // The same guest port reuses its socket.
    gw.send(&udp_frame(4000, host_side(port), b"again"));
    let (_, again) = server.recv_from(&mut buf).unwrap();
    assert_eq!((again, gw.flows()), (from, 1));
    // Idle for longer than the limit: closed.
    gw.recv_at(Instant::now() + limits().udp_idle + Duration::from_secs(1));
    assert_eq!(gw.flows(), 0);
}

#[test]
fn udp_to_unroutable_addresses_is_dropped() {
    let mut gw = gateway();
    for dst in [
        Ipv4Addr::new(10, 0, 2, 16),
        Ipv4Addr::new(127, 0, 0, 1),
        Ipv4Addr::BROADCAST,
        Ipv4Addr::new(224, 0, 0, 251),
    ] {
        gw.send(&udp_frame(4000, SocketAddrV4::new(dst, 9), b"x"));
    }
    assert_eq!((gw.flows(), drain(&mut gw)), (0, vec![]));
}

/// The guest's side of a connection it opened to `port` on the host (10.0.2.2), up
/// to the handshake. Returns the gateway's SYN-ACK.
fn connect(gw: &mut Gateway, guest_port: u16, port: u16, iss: u32) -> Seg {
    gw.send(&tcp_frame(guest_port, host_side(port), SYN, iss, 0, b""));
    let (syn_ack, _) = wait(gw, tcp_seg);
    assert_eq!(syn_ack.flags, SYN | ACK, "{syn_ack:?}");
    assert_eq!(
        (syn_ack.src, syn_ack.dst),
        (host_side(port), SocketAddrV4::new(GUEST, guest_port))
    );
    assert_eq!((syn_ack.ack, syn_ack.mss), (iss + 1, Some(TCP_MSS as u16)));
    gw.send(&tcp_frame(
        guest_port,
        host_side(port),
        ACK,
        iss + 1,
        syn_ack.seq + 1,
        b"",
    ));
    syn_ack
}

#[test]
fn tcp_to_the_host_handshake_data_and_close() {
    let mut gw = gateway();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let to = host_side(port);
    let syn_ack = connect(&mut gw, 40000, port, 1000);
    let (mut host, _) = listener.accept().unwrap();
    host.set_read_timeout(Some(DEADLINE)).unwrap();
    let mut seq = 1001;
    let ack = syn_ack.seq + 1;

    // Guest to host: the data is acknowledged and arrives.
    gw.send(&tcp_frame(
        40000,
        to,
        ACK | PSH,
        seq,
        ack,
        b"GET / HTTP/1.0\r\n\r\n",
    ));
    seq += 18;
    let out = drain(&mut gw);
    let Out::Tcp(a) = &out[0] else { panic!("{out:?}") };
    assert_eq!((a.flags, a.ack), (ACK, seq));
    let mut buf = [0; 18];
    host.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"GET / HTTP/1.0\r\n\r\n");

    // Host to guest: a segment with the bytes.
    host.write_all(b"HTTP/1.0 200 OK\r\n").unwrap();
    let (data, _) = wait(&mut gw, tcp_seg);
    assert_eq!((data.flags, data.seq, data.ack), (ACK | PSH, ack, seq));
    assert_eq!(data.payload, b"HTTP/1.0 200 OK\r\n");
    let ack = ack + data.payload.len() as u32;
    gw.send(&tcp_frame(40000, to, ACK, seq, ack, b""));

    // The host closes: FIN to the guest, which closes too.
    drop(host);
    let (fin, _) = wait(&mut gw, tcp_seg);
    assert_eq!((fin.flags, fin.seq), (FIN | ACK, ack));
    gw.send(&tcp_frame(40000, to, FIN | ACK, seq, ack + 1, b""));
    let out = drain(&mut gw);
    let Out::Tcp(last) = &out[0] else {
        panic!("{out:?}")
    };
    assert_eq!((last.flags, last.ack), (ACK, seq + 1));
    assert_eq!(gw.flows(), 0, "both sides closed");
}

#[test]
fn tcp_guest_closes_first() {
    let mut gw = gateway();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let to = host_side(port);
    let syn_ack = connect(&mut gw, 40001, port, 5);
    let (mut host, _) = listener.accept().unwrap();
    host.set_read_timeout(Some(DEADLINE)).unwrap();
    let ack = syn_ack.seq + 1;
    // Data and FIN in one segment: the host reads the data, then end of file.
    gw.send(&tcp_frame(40001, to, FIN | ACK | PSH, 6, ack, b"bye"));
    let mut got = Vec::new();
    host.read_to_end(&mut got).unwrap();
    assert_eq!(got, b"bye");
    let out = drain(&mut gw);
    assert!(
        matches!(&out[..], [Out::Tcp(s)] if s.ack == 10 && s.flags == ACK),
        "{out:?}"
    );
    // The host answers and closes its side.
    host.write_all(b"ok").unwrap();
    drop(host);
    let (data, _) = wait(&mut gw, tcp_seg);
    assert_eq!(data.payload, b"ok");
    let fin = if data.flags & FIN != 0 {
        data.clone()
    } else {
        wait(&mut gw, tcp_seg).0
    };
    assert_eq!(fin.flags & FIN, FIN);
    let fin_seq = fin.seq + fin.payload.len() as u32;
    gw.send(&tcp_frame(40001, to, ACK, 10, fin_seq + 1, b""));
    until("the flow to close", || {
        (gw.recv().is_none() && gw.flows() == 0).then_some(())
    });
}

#[test]
fn tcp_refused_or_unroutable_gets_a_reset() {
    let mut gw = gateway();
    // A port nobody listens on: bind one, then close it.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    gw.send(&tcp_frame(40002, host_side(port), SYN, 77, 0, b""));
    let (rst, _) = wait(&mut gw, tcp_seg);
    assert_eq!((rst.flags, rst.ack), (RST | ACK, 78));
    until("the flow to go", || (gw.flows() == 0).then_some(()));

    // Another guest's address and a guest's loopback are not routed; a segment of
    // no known connection is reset.
    for (dst, flags, seq, ack, want) in [
        (Ipv4Addr::new(10, 0, 2, 16), SYN, 1, 0, (RST | ACK, 0, 2)),
        (Ipv4Addr::LOCALHOST, SYN, 1, 0, (RST | ACK, 0, 2)),
        (Ipv4Addr::new(192, 0, 2, 1), ACK, 1, 500, (RST, 500, 0)),
    ] {
        gw.send(&tcp_frame(
            40003,
            SocketAddrV4::new(dst, 80),
            flags,
            seq,
            ack,
            b"",
        ));
        let out = drain(&mut gw);
        let [Out::Tcp(rst)] = &out[..] else {
            panic!("{out:?}")
        };
        assert_eq!((rst.flags, rst.seq, rst.ack), want);
    }
    // A reset of nothing gets no answer.
    gw.send(&tcp_frame(40003, host_side(1), RST, 1, 0, b""));
    assert_eq!((drain(&mut gw), gw.flows()), (vec![], 0));
}

#[test]
fn tcp_retransmits_until_acknowledged() {
    let mut gw = gateway();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let syn_ack = connect(&mut gw, 40004, port, 0);
    let (mut host, _) = listener.accept().unwrap();
    host.write_all(b"lost?").unwrap();
    let (data, _) = wait(&mut gw, tcp_seg);
    assert_eq!(data.payload, b"lost?");
    // The guest never saw it: after the timeout it comes again, then after twice that.
    let t0 = Instant::now();
    let rto = limits().tcp_rto;
    let again = decode(&gw.recv_at(t0 + rto + Duration::from_millis(50)).unwrap());
    assert_eq!(again, Out::Tcp(data.clone()));
    assert_eq!(gw.recv_at(t0 + rto * 2), None, "the timeout doubled");
    let third = decode(&gw.recv_at(t0 + rto * 4).unwrap());
    assert_eq!(third, Out::Tcp(data.clone()));
    // Acknowledged: no more.
    gw.send(&tcp_frame(40004, host_side(port), ACK, 1, syn_ack.seq + 6, b""));
    assert_eq!(gw.recv_at(t0 + rto * 20), None);
    assert_eq!(gw.flows(), 1);
    // Unacknowledged forever: the connection is reset after the retries.
    host.write_all(b"again").unwrap();
    wait(&mut gw, tcp_seg);
    let mut t = Instant::now();
    let mut resets = 0;
    for _ in 0..40 {
        t += tcp::MAX_RTO;
        while let Some(f) = gw.recv_at(t) {
            if let Out::Tcp(s) = decode(&f) {
                resets += (s.flags & RST != 0) as u32;
            }
        }
    }
    assert_eq!((resets, gw.flows()), (1, 0));
}

#[test]
fn tcp_out_of_order_and_duplicate_data() {
    let mut gw = gateway();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let to = host_side(port);
    let syn_ack = connect(&mut gw, 40005, port, 100);
    let (mut host, _) = listener.accept().unwrap();
    host.set_read_timeout(Some(DEADLINE)).unwrap();
    let ack = syn_ack.seq + 1;
    // A segment from the future is not taken: the ACK still asks for 101.
    gw.send(&tcp_frame(40005, to, ACK, 104, ack, b"def"));
    let out = drain(&mut gw);
    assert!(matches!(&out[..], [Out::Tcp(s)] if s.ack == 101), "{out:?}");
    // In order, then a retransmission overlapping it with two new bytes.
    gw.send(&tcp_frame(40005, to, ACK, 101, ack, b"abc"));
    gw.send(&tcp_frame(40005, to, ACK, 102, ack, b"bcde"));
    let acks: Vec<u32> = drain(&mut gw).iter().filter_map(tcp_seg).map(|s| s.ack).collect();
    assert_eq!(acks, [104, 106]);
    let mut buf = [0; 5];
    host.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"abcde");
}

#[test]
fn tcp_guest_window_limits_what_is_sent() {
    let mut gw = gateway();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let to = host_side(port);
    // The guest's SYN says MSS 1000, window 8192 (see tcp_frame); then a window of 1500.
    let syn_ack = connect(&mut gw, 40006, port, 0);
    let ack = syn_ack.seq + 1;
    gw.send(&{
        let h = TcpHeader {
            src_port: 40006,
            dst_port: port,
            seq: 1,
            ack,
            flags: ACK,
            window: 1500,
            mss: None,
        };
        ip_frame(
            GUEST,
            GATEWAY_ADDR,
            PROTO_TCP,
            &tcp_segment(GUEST, GATEWAY_ADDR, &h, b""),
        )
    });
    let (mut host, _) = listener.accept().unwrap();
    host.write_all(&[7; 4000]).unwrap();
    // Wait until all 4000 bytes are buffered; only 1500 go out, in segments of 1000.
    let mut sent = Vec::new();
    until("1500 bytes", || {
        while let Some(f) = gw.recv() {
            sent.extend(tcp_seg(&decode(&f)));
        }
        (sent.iter().map(|s| s.payload.len()).sum::<usize>() >= 1500).then_some(())
    });
    // Segments of at most the guest's MSS, 1500 bytes in all however the host's
    // bytes were read; polling more sends nothing past the window.
    for _ in 0..10 {
        while let Some(f) = gw.recv() {
            sent.extend(tcp_seg(&decode(&f)));
        }
    }
    assert!(sent.iter().all(|s| s.payload.len() <= 1000), "{sent:?}");
    // (A slow run may see a retransmission: count the sequence space covered.)
    let end = sent.iter().map(|s| s.seq + s.payload.len() as u32).max().unwrap();
    assert_eq!(end - ack, 1500);
    // Acknowledging all of it opens the window for the next 1500.
    gw.send(&tcp_frame(40006, to, ACK, 1, ack + 1500, b""));
    let mut more = Vec::new();
    until("1500 more bytes", || {
        while let Some(f) = gw.recv() {
            more.extend(tcp_seg(&decode(&f)));
        }
        (more.iter().map(|s| s.payload.len()).sum::<usize>() >= 1500).then_some(())
    });
    assert_eq!(more[0].seq, ack + 1500);
}

#[test]
fn hostfwd_tcp_reaches_the_guest() {
    let mut gw = gateway_with(Config {
        hostfwd: vec![HostFwd::tcp(0, 7)],
        limits: limits(),
        ..Config::default()
    });
    let rule = gw.hostfwd()[0];
    assert_ne!(rule.host.port(), 0);
    let mut host = TcpStream::connect(rule.host).unwrap();
    host.set_read_timeout(Some(DEADLINE)).unwrap();
    let (syn, _) = wait(&mut gw, tcp_seg);
    assert_eq!(syn.flags, SYN);
    assert_eq!(syn.dst, SocketAddrV4::new(GUEST, 7));
    assert_eq!(*syn.src.ip(), GATEWAY_ADDR);
    assert!(syn.src.port() >= FIRST_PORT);
    let from = syn.src;
    // The guest accepts.
    gw.send(&tcp_frame(7, from, SYN | ACK, 9000, syn.seq + 1, b""));
    let out = drain(&mut gw);
    assert!(
        matches!(&out[..], [Out::Tcp(s)] if s.flags == ACK && s.ack == 9001),
        "{out:?}"
    );
    // Host to guest, and the guest echoes it.
    host.write_all(b"echo me").unwrap();
    let (data, _) = wait(&mut gw, tcp_seg);
    assert_eq!(data.payload, b"echo me");
    let ack = data.seq + 7;
    gw.send(&tcp_frame(7, from, ACK | PSH, 9001, ack, b"echo me"));
    let mut buf = [0; 7];
    host.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"echo me");
    // The host closes; so does the guest.
    host.shutdown(std::net::Shutdown::Write).unwrap();
    let (fin, _) = wait(&mut gw, |o| tcp_seg(o).filter(|s| s.flags & FIN != 0));
    gw.send(&tcp_frame(7, from, FIN | ACK, 9008, fin.seq + 1, b""));
    let mut rest = Vec::new();
    host.read_to_end(&mut rest).unwrap();
    assert_eq!(rest, b"");
    until("the flow to close", || {
        (gw.recv().is_none() && gw.flows() == 0).then_some(())
    });
}

#[test]
fn hostfwd_asks_arp_for_an_unknown_guest() {
    let mut gw = Gateway::new(Config {
        hostfwd: vec![HostFwd::tcp(0, 7)],
        limits: limits(),
        ..Config::default()
    })
    .unwrap();
    let _host = TcpStream::connect(gw.hostfwd()[0].host).unwrap();
    let (who, _) = wait(&mut gw, |o| match o {
        Out::Arp(a) => Some(*a),
        _ => None,
    });
    assert_eq!(
        (who.op, who.target_ip, who.sender_ip),
        (ARP_REQUEST, GUEST, GATEWAY_ADDR)
    );
    // The guest answers; the SYN that waited goes out.
    gw.send(&arp_frame(ARP_REPLY, GATEWAY_ADDR));
    let out = drain(&mut gw);
    assert!(matches!(&out[..], [Out::Tcp(s)] if s.flags == SYN), "{out:?}");
}

#[test]
fn hostfwd_udp_reaches_the_guest_and_back() {
    let mut gw = gateway_with(Config {
        hostfwd: vec![HostFwd::udp(0, 53)],
        limits: limits(),
        ..Config::default()
    });
    let rule = gw.hostfwd()[0];
    let client = UdpSocket::bind("127.0.0.1:0").unwrap();
    client.set_read_timeout(Some(DEADLINE)).unwrap();
    client.send_to(b"question", rule.host).unwrap();
    let (got, _) = wait(&mut gw, |o| match o {
        Out::Udp { src, dst, payload } => Some((*src, *dst, payload.clone())),
        _ => None,
    });
    assert_eq!(*got.0.ip(), GATEWAY_ADDR);
    assert_eq!(
        (got.1, got.2.as_slice()),
        (SocketAddrV4::new(GUEST, 53), &b"question"[..])
    );
    assert_eq!(gw.flows(), 1);
    gw.send(&udp_frame(53, got.0, b"answer"));
    let mut buf = [0; 16];
    let (n, from) = client.recv_from(&mut buf).unwrap();
    assert_eq!((&buf[..n], from), (&b"answer"[..], SocketAddr::V4(rule.host)));
}

#[test]
fn hostfwd_port_in_use_is_an_error() {
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let e = Gateway::new(Config {
        hostfwd: vec![HostFwd::tcp(port, 80)],
        ..Config::default()
    })
    .unwrap_err();
    assert!(
        e.to_string()
            .starts_with(&format!("hostfwd tcp:127.0.0.1:{port}-10.0.2.15:80: ")),
        "{e}"
    );
}

#[test]
fn flows_per_guest_are_limited() {
    let mut gw = gateway_with(Config {
        limits: Limits {
            flows_per_guest: 2,
            ..limits()
        },
        ..Config::default()
    });
    let server = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = server.local_addr().unwrap().port();
    for guest_port in [1, 2, 3] {
        gw.send(&udp_frame(guest_port, host_side(port), b"x"));
    }
    assert_eq!((gw.flows(), gw.stats().refused), (2, 1));
    // A third flow as TCP: reset at once.
    gw.send(&tcp_frame(40007, host_side(port), SYN, 0, 0, b""));
    let out = drain(&mut gw);
    assert!(
        matches!(&out[..], [Out::Tcp(s)] if s.flags == RST | ACK),
        "{out:?}"
    );
    assert_eq!(gw.stats().refused, 2);
}

#[test]
fn queue_toward_the_guest_is_bounded() {
    let mut gw = gateway_with(Config {
        limits: Limits { queue: 3, ..limits() },
        ..Config::default()
    });
    for _ in 0..5 {
        gw.send(&arp_frame(ARP_REQUEST, GATEWAY_ADDR));
    }
    assert_eq!((drain(&mut gw).len(), gw.stats().queue_full), (3, 2));
}

#[test]
fn frames_not_for_the_gateway_are_ignored() {
    let mut gw = gateway();
    let mut other = arp_frame(ARP_REQUEST, GATEWAY_ADDR);
    other[..6].copy_from_slice(&[2, 0, 0, 0, 0, 9]); // to another card
    gw.send(&other);
    let mut ipv6 = arp_frame(ARP_REQUEST, GATEWAY_ADDR);
    ipv6[12..14].copy_from_slice(&0x86ddu16.to_be_bytes());
    gw.send(&ipv6);
    let mut group = arp_frame(ARP_REQUEST, GATEWAY_ADDR);
    group[6] = 1; // a group address as the source
    gw.send(&group);
    assert_eq!((drain(&mut gw), gw.stats().malformed), (vec![], 0));
}

/// Every truncation of every kind of frame, bad checksums, and bytes from a simple
/// generator: dropped (counted), never a panic, never an answer.
#[test]
fn broken_frames_are_dropped() {
    let mut gw = gateway();
    let syn = tcp_frame(40008, host_side(9), SYN, 0, 0, b"");
    let udp = udp_frame(
        4000,
        SocketAddrV4::new(DNS_ADDR, 53),
        &dns::query(1, "camelot.test", dns::TYPE_A),
    );
    let ping = ip_frame(
        GUEST,
        GATEWAY_ADDR,
        PROTO_ICMP,
        &echo_message(&Echo {
            kind: ICMP_ECHO_REQUEST,
            ident: 1,
            seq: 1,
            data: b"x",
        }),
    );
    for frame in [&syn, &udp, &ping] {
        for n in 0..frame.len() {
            gw.send(&frame[..n]);
        }
        // One bit flipped in every byte past the Ethernet header (the IP header's
        // checksum, the payload's, or a length catches each).
        for i in ETH_HEADER..frame.len() {
            let mut bad = frame.clone();
            bad[i] ^= 0x10;
            gw.send(&bad);
        }
    }
    assert_eq!(drain(&mut gw), []);
    assert_eq!(gw.flows(), 0);
    assert!(gw.stats().malformed > 0);

    let mut x: u32 = 0x1234_5678;
    for len in 0..2000 {
        let mut frame = Vec::with_capacity(len);
        for _ in 0..len {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            frame.push(x as u8);
        }
        // Addressed to the gateway, with each EtherType it reads.
        if len >= ETH_HEADER {
            frame[..6].copy_from_slice(&GATEWAY_MAC);
            frame[6] &= !1;
            let ethertype = if len % 2 == 0 {
                ETHERTYPE_IPV4
            } else {
                ETHERTYPE_ARP
            };
            frame[12..14].copy_from_slice(&ethertype.to_be_bytes());
            if len % 4 == 0 && len > ETH_HEADER + 1 {
                frame[ETH_HEADER] = 0x45;
            }
        }
        gw.send(&frame);
    }
    let _ = drain(&mut gw);
}

#[test]
fn guest_addresses() {
    assert!(is_guest_addr(FIRST_LEASE));
    assert!(is_guest_addr(Ipv4Addr::new(10, 0, 2, 1)));
    for not in [
        NETWORK,
        GATEWAY_ADDR,
        DNS_ADDR,
        Ipv4Addr::new(10, 0, 2, 255),
        Ipv4Addr::new(10, 0, 3, 15),
    ] {
        assert!(!is_guest_addr(not), "{not}");
    }
    assert_eq!(host_addr(GATEWAY_ADDR), Some(Ipv4Addr::LOCALHOST));
    assert_eq!(
        host_addr(Ipv4Addr::new(192, 0, 2, 1)),
        Some(Ipv4Addr::new(192, 0, 2, 1))
    );
    assert_eq!(host_addr(DNS_ADDR), None);
    assert_eq!(host_addr(Ipv4Addr::new(127, 0, 0, 1)), None);
}
