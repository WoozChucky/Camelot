//! A switch's NAT gateway end to end, in process and over the protocol: two VMs on a
//! switch with a gateway run the bare-metal `examples/net/net-user`, get different
//! DHCP leases, one connects through 10.0.2.2 to a listener of this test on the host's
//! 127.0.0.1, and the test reaches the other through a forwarded host port
//! (`hostfwd`). The gateway's port is counted and captured like any other, deleting
//! the switch (or taking its gateway away) frees its host ports, and the requests'
//! errors.
//!
//! Nothing here needs the internet, and nothing sleeps for a fixed time: every wait
//! polls with a deadline.

mod common;

use avalon_proto::{
    DeviceSpec, ErrorCode, FwdProto, HardwareProfile, Lease, Message, NatConfig, NetSpec, PortForward,
    PortOwner, SwitchInfo, VmRef, VmState,
};
use common::*;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
use std::time::{Duration, Instant};

type Conn = avalon_proto::Connection<TcpStream>;

const ALPHA_MAC: [u8; 6] = [0x02, 0xca, 0, 0, 0, 0x0a];
const BETA_MAC: [u8; 6] = [0x02, 0xca, 0, 0, 0, 0x0b];
const GATEWAY_MAC: [u8; 6] = [0x52, 0x55, 0x0a, 0x00, 0x02, 0x02];

fn net_user() -> Vec<u8> {
    example_in("net", "net-user")
}

/// 16 MiB at full speed, with one card plugged into `switch`.
fn on_switch(switch: &str, mac: [u8; 6]) -> HardwareProfile {
    HardwareProfile {
        devices: vec![DeviceSpec::Net(NetSpec {
            switch: Some(switch.into()),
            mac: Some(mac),
        })],
        ..profile(16, None)
    }
}

fn ok(conn: &mut Conn, msg: Message) {
    conn.request_ok(&msg).unwrap_or_else(|e| panic!("{msg:?}: {e}"));
}

fn switch(conn: &mut Conn, name: &str) -> SwitchInfo {
    match conn.request(&Message::ListSwitches) {
        Ok(Message::SwitchList { switches }) => switches
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("no switch {name}")),
        other => panic!("{other:?}"),
    }
}

fn tcp(host: SocketAddrV4, guest: SocketAddrV4) -> PortForward {
    PortForward {
        proto: FwdProto::Tcp,
        host,
        guest,
    }
}

fn loopback(port: u16) -> SocketAddrV4 {
    SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)
}

fn guest(last: u8, port: u16) -> SocketAddrV4 {
    SocketAddrV4::new(Ipv4Addr::new(10, 0, 2, last), port)
}

fn nat(hostfwd: Vec<PortForward>) -> Option<NatConfig> {
    Some(NatConfig { hostfwd })
}

/// Accepts one connection on `listener`, giving up at the deadline.
fn accept(listener: &TcpListener) -> TcpStream {
    listener.set_nonblocking(true).unwrap();
    let start = Instant::now();
    loop {
        match listener.accept() {
            Ok((s, _)) => {
                s.set_nonblocking(false).unwrap();
                s.set_read_timeout(Some(SLOW)).unwrap();
                return s;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                assert!(start.elapsed() < SLOW, "the guest never connected");
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(e) => panic!("accept: {e}"),
        }
    }
}

#[test]
fn two_vms_share_a_gateway_reach_the_host_and_are_reached_from_it() {
    let server = server(None);
    let mut rt = connect(&server);
    // The host service the first guest connects to, through 10.0.2.2.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let service = listener.local_addr().unwrap().port();
    // The host port the test reaches the second guest by: port 0, the system picks.
    ok(
        &mut rt,
        Message::CreateSwitch {
            name: "wan".into(),
            nat: nat(vec![tcp(loopback(0), guest(16, 7))]),
        },
    );
    let wan = switch(&mut rt, "wan");
    let forwarded = wan.nat.as_ref().unwrap().hostfwd[0].host;
    assert_eq!(*forwarded.ip(), Ipv4Addr::LOCALHOST);
    assert_ne!(forwarded.port(), 0, "the port the system picked");
    assert_eq!(wan.nat.as_ref().unwrap().hostfwd[0].guest, guest(16, 7));
    assert_eq!(wan.nat.as_ref().unwrap().leases, []);
    assert_eq!(wan.ports.len(), 1);
    assert_eq!(
        (wan.ports[0].owner, wan.ports[0].mac),
        (PortOwner::Gateway, GATEWAY_MAC)
    );

    let capture = std::env::temp_dir().join(format!("avalon-nat-{}-wan.pcap", std::process::id()));
    let capture_path = capture.to_str().unwrap().to_string();
    ok(
        &mut rt,
        Message::CaptureSwitch {
            switch: "wan".into(),
            path: Some(capture_path.clone()),
        },
    );

    // Alpha asks first, so it leases 10.0.2.15; beta then gets 10.0.2.16, the address
    // the forwarding rule names.
    let alpha = create(&mut rt, "alpha", on_switch("wan", ALPHA_MAC), net_user());
    let beta = create(&mut rt, "beta", on_switch("wan", BETA_MAC), net_user());
    let mut a = Console::attach(&server, VmRef::Id(alpha));
    ok(&mut rt, Message::StartVm { vm: VmRef::Id(alpha) });
    a.wait_for("dhcp: address 10.0.2.15, server 10.0.2.2\n");
    let mut b = Console::attach(&server, VmRef::Id(beta));
    ok(&mut rt, Message::StartVm { vm: VmRef::Id(beta) });
    b.wait_for("dhcp: address 10.0.2.16, server 10.0.2.2\n");
    assert_eq!(
        switch(&mut rt, "wan").nat.unwrap().leases,
        [
            Lease {
                mac: ALPHA_MAC,
                addr: Ipv4Addr::new(10, 0, 2, 15),
            },
            Lease {
                mac: BETA_MAC,
                addr: Ipv4Addr::new(10, 0, 2, 16),
            },
        ]
    );

    // Alpha to the host, through 10.0.2.2.
    a.send(format!("c {service}\n").as_bytes());
    let mut s = accept(&listener);
    let mut greeting = [0; 22];
    s.read_exact(&mut greeting).unwrap();
    assert_eq!(&greeting, b"Hello from the guest!\n");
    s.write_all(b"Hello from the host!\n").unwrap();
    drop(s);
    a.wait_for("closed\n");

    // The host to beta, through the forwarded port, once beta listens.
    b.send(b"l 7\n");
    b.wait_for("listen 7\n");
    let mut s = TcpStream::connect(forwarded).unwrap();
    s.set_read_timeout(Some(SLOW)).unwrap();
    s.write_all(b"ping over hostfwd\n").unwrap();
    let mut echo = [0; 18];
    s.read_exact(&mut echo).unwrap();
    assert_eq!(&echo, b"ping over hostfwd\n");
    s.shutdown(std::net::Shutdown::Write).unwrap();
    let mut rest = Vec::new();
    s.read_to_end(&mut rest).unwrap();
    assert_eq!(rest, b"", "the guest closes after the host");
    b.wait_for("echoed 18 bytes\nclosed\n");

    a.send(b"q\n");
    b.send(b"q\n");
    for (console, vm) in [(&mut a, alpha), (&mut b, beta)] {
        console.wait_for_event(Message::VmStateChanged {
            vm,
            state: VmState::Exited(0),
        });
    }
    let net = addr_text(0x1000_9000);
    assert_eq!(
        String::from_utf8_lossy(&a.output),
        format!(
            "net {net}, irq 12, mac 02:ca:00:00:00:0a, link up\n\
         dhcp: address 10.0.2.15, server 10.0.2.2\n\
         connect 10.0.2.2:{service}\n\
         received: Hello from the host!\n\
         closed\n"
        )
    );
    assert_eq!(
        String::from_utf8_lossy(&b.output),
        format!(
            "net {net}, irq 12, mac 02:ca:00:00:00:0b, link up\n\
         dhcp: address 10.0.2.16, server 10.0.2.2\n\
         listen 7\n\
         accepted from 10.0.2.2:49152\n\
         echoed 18 bytes\n\
         closed\n"
        )
    );

    // Both machines have ended and their cards are off the switch; the gateway stays,
    // and so do the leases, for when the VMs come back. Its port counted the traffic
    // both ways.
    let wan = switch(&mut rt, "wan");
    assert_eq!(wan.nat.as_ref().unwrap().leases.len(), 2);
    assert_eq!(wan.ports.len(), 1, "{wan:?}");
    let port = wan.ports[0];
    assert_eq!(port.owner, PortOwner::Gateway);
    assert!(port.in_frames > 10 && port.out_frames > 10, "{port:?}");
    assert_eq!(port.dropped, 0);

    // The capture has the gateway's frames as well as the guests'.
    ok(
        &mut rt,
        Message::CaptureSwitch {
            switch: "wan".into(),
            path: None,
        },
    );
    let bytes = std::fs::read(&capture).unwrap();
    std::fs::remove_file(&capture).unwrap();
    let frames = pcap_frames(&bytes);
    let from = |mac: [u8; 6]| frames.iter().filter(|f| f[6..12] == mac).count() as u64;
    assert_eq!(from(GATEWAY_MAC), port.in_frames);
    assert!(from(ALPHA_MAC) > 0 && from(BETA_MAC) > 0);
    assert_eq!(
        frames.len() as u64,
        from(GATEWAY_MAC) + from(ALPHA_MAC) + from(BETA_MAC)
    );

    // While the switch holds the port, nobody else can have it: the request fails,
    // naming the rule, and creates nothing.
    let taken = Message::CreateSwitch {
        name: "lan".into(),
        nat: nat(vec![tcp(forwarded, guest(15, 22))]),
    };
    match rt.request(&taken) {
        Err(avalon_proto::Error::Remote { code, message }) => {
            assert_eq!(code, ErrorCode::Io);
            assert!(
                message.contains(&format!("tcp:{forwarded}-10.0.2.15:22")),
                "{message}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(!switches(&mut rt).iter().any(|s| s.name == "lan"));

    // Deleting the switch frees its host ports at once: a new switch binds the same
    // one, and so does another once the gateway is taken away.
    ok(&mut rt, Message::DeleteSwitch { name: "wan".into() });
    ok(&mut rt, taken.clone());
    ok(
        &mut rt,
        Message::SetSwitchNat {
            switch: "lan".into(),
            nat: None,
        },
    );
    assert_eq!(switch(&mut rt, "lan").nat, None);
    assert!(switch(&mut rt, "lan").ports.is_empty());
    ok(
        &mut rt,
        Message::SetSwitchNat {
            switch: "lan".into(),
            nat: nat(vec![tcp(forwarded, guest(15, 22))]),
        },
    );
    assert_eq!(
        switch(&mut rt, "lan").nat.unwrap().hostfwd,
        [tcp(forwarded, guest(15, 22))]
    );
    server.stop().unwrap();
    // The server's end closed the gateway: the port is free again.
    drop(TcpListener::bind(forwarded).unwrap());
}

fn switches(conn: &mut Conn) -> Vec<SwitchInfo> {
    match conn.request(&Message::ListSwitches) {
        Ok(Message::SwitchList { switches }) => switches,
        other => panic!("{other:?}"),
    }
}

#[test]
fn gateway_requests_and_their_errors() {
    let server = server(None);
    let mut rt = connect(&server);
    ok(
        &mut rt,
        Message::CreateSwitch {
            name: "wan".into(),
            nat: nat(Vec::new()),
        },
    );
    let wan = switch(&mut rt, "wan");
    assert_eq!(wan.nat.as_ref().map(|n| n.hostfwd.len()), Some(0));
    assert_eq!(wan.ports[0].owner, PortOwner::Gateway);

    let set = |switch: &str, nat: Option<NatConfig>| Message::SetSwitchNat {
        switch: switch.into(),
        nat,
    };
    // One gateway per switch.
    assert_eq!(
        error_of(&mut rt, &set("wan", nat(Vec::new()))),
        ErrorCode::AlreadyExists
    );
    assert_eq!(error_of(&mut rt, &set("nowhere", None)), ErrorCode::NoSuchSwitch);
    assert_eq!(
        error_of(&mut rt, &set("nowhere", nat(Vec::new()))),
        ErrorCode::NoSuchSwitch
    );
    ok(&mut rt, set("wan", None));
    assert_eq!(error_of(&mut rt, &set("wan", None)), ErrorCode::InvalidState);
    assert!(switch(&mut rt, "wan").ports.is_empty());

    // A card with the gateway's address keeps a gateway off its switch, and the other
    // way round.
    let vm = create(&mut rt, "imposter", on_switch("wan", GATEWAY_MAC), net_user());
    assert_eq!(
        error_of(&mut rt, &set("wan", nat(Vec::new()))),
        ErrorCode::AlreadyExists
    );
    ok(
        &mut rt,
        Message::DetachCard {
            vm: VmRef::Id(vm),
            card: 0,
        },
    );
    ok(&mut rt, set("wan", nat(Vec::new())));
    assert_eq!(
        error_of(
            &mut rt,
            &Message::AttachCard {
                vm: VmRef::Id(vm),
                card: 0,
                switch: "wan".into(),
            }
        ),
        ErrorCode::AlreadyExists
    );

    // A rule that cannot be bound fails the request, which changes nothing: a UDP
    // port in use, and an address the host does not have.
    let busy = UdpSocket::bind("127.0.0.1:0").unwrap();
    let busy_port = busy.local_addr().unwrap().port();
    let udp = PortForward {
        proto: FwdProto::Udp,
        host: loopback(busy_port),
        guest: guest(15, 53),
    };
    ok(&mut rt, set("wan", None));
    assert_eq!(error_of(&mut rt, &set("wan", nat(vec![udp]))), ErrorCode::Io);
    assert_eq!(switch(&mut rt, "wan").nat, None);
    let elsewhere = tcp(SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, 1), 0), guest(15, 80));
    assert_eq!(
        error_of(
            &mut rt,
            &Message::CreateSwitch {
                name: "lan".into(),
                nat: nat(vec![elsewhere]),
            }
        ),
        ErrorCode::Io
    );
    // Two rules for one port: the second cannot bind, and the first is let go. The
    // port is one the system just picked and let go of: another test running at the
    // same time could take it in between, but only by being handed this very port
    // within microseconds, which systems rarely do (and the test would then fail, not
    // pass wrongly). Holding it instead would defeat the check: that the gateway
    // binds it, then lets go.
    let free = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let same = tcp(loopback(free), guest(15, 80));
    assert_eq!(
        error_of(
            &mut rt,
            &Message::CreateSwitch {
                name: "lan".into(),
                nat: nat(vec![same, same]),
            }
        ),
        ErrorCode::Io
    );
    drop(TcpListener::bind(loopback(free)).unwrap());
    assert_eq!(switches(&mut rt).len(), 1);

    // The usual switch errors still come first.
    assert_eq!(
        error_of(
            &mut rt,
            &Message::CreateSwitch {
                name: "wan".into(),
                nat: nat(Vec::new()),
            }
        ),
        ErrorCode::AlreadyExists
    );
    assert_eq!(
        error_of(
            &mut rt,
            &Message::CreateSwitch {
                name: "Bad".into(),
                nat: nat(Vec::new()),
            }
        ),
        ErrorCode::InvalidName
    );
    // A rule the gateway cannot have does not even decode.
    let bad = Message::SetSwitchNat {
        switch: "wan".into(),
        nat: nat(vec![tcp(loopback(0), guest(2, 80))]),
    };
    assert!(matches!(
        avalon_proto::encode(&bad),
        Err(avalon_proto::Error::Invalid(_))
    ));
    drop(busy);
    server.stop().unwrap();
}
