//! The Camelot kernel's network stack (`kernel/net/`, docs/camelot-net.md) on Grail's
//! network cards, driven through the shell's `ifconfig` and `ping` built-ins:
//!
//! - with user networking (`--net user`) it gets 10.0.2.15 from the gateway by DHCP and
//!   pings 10.0.2.2, itself and 127.0.0.1; its UDP echo service answers a host program
//!   through a forwarded port;
//! - two kernels on a hub, with static addresses (one from the kernel's configuration,
//!   one from `ifconfig` after DHCP found no server), ping each other;
//! - on a hub with this test as the other host, crafted frames (bad checksums,
//!   truncated headers, wrong lengths, fragments, unknown protocols, sources no host can
//!   have) are dropped and counted, never a panic, and the stack still answers
//!   afterwards; ARP gives up on an address nobody has, and forgets answered ones after
//!   a minute;
//! - after each, every packet buffer is back in the pool but the receive ring's.
//!
//! The machines run in this thread, in turns of a few thousand steps, so what they do is
//! deterministic: time is Grail's steps, the forwarded port's gateway included. Only
//! that test goes through real host sockets, and its waits for them are bounded.

mod common;

use common::*;
use grail::net::{Hub, MacAddr, NetBackend};
use grail::NetConfig;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

/// `time=1.234 ms` (which depends on how the machines take turns) as `time=T ms`.
fn mask_times(s: &str) -> String {
    s.lines()
        .map(|l| match (l.find("time="), l.find(" ms")) {
            (Some(a), Some(b)) if a < b => format!("{}time=T{}\n", &l[..a], &l[b..]),
            _ => format!("{l}\n"),
        })
        .collect()
}

/// What `ping ADDR N` prints when every reply comes.
fn ping_ok(addr: &str, n: u32) -> String {
    let mut s = format!("PING {addr}: 56 data bytes\n");
    for seq in 1..=n {
        s += &format!("64 bytes from {addr}: seq={seq} ttl=64 time=T ms\n");
    }
    s + &format!("{n} sent, {n} received\n")
}

/// Sends a frame the machine must drop without a word.
fn ignored(vm: &mut Vm, host: &mut Host, what: &str, frame: Vec<u8>) {
    let back = exchange(vm, host, frame);
    assert!(back.is_empty(), "{what}: the kernel answered {back:x?}");
}

/// An echo request from the host to the machine, and a check of its reply.
fn echo(vm: &mut Vm, host: &mut Host, seq: u16) {
    let data = b"Camelot echo data";
    let req = ether(
        VM_MAC,
        HOST_MAC,
        IP_TYPE,
        &ipv4(HOST_IP, VM_IP, 1, &icmp_echo(8, 0x4242, seq, data)),
    );
    let back = exchange(vm, host, req);
    assert_eq!(back.len(), 1, "{back:x?}");
    assert_eq!(back[0][..12], [HOST_MAC, VM_MAC].concat());
    let p = parse_ipv4(&back[0]).unwrap();
    assert_eq!((p.src, p.dst, p.proto, p.ttl), (VM_IP, HOST_IP, 1, 64));
    assert_eq!(checksum(&p.payload, 0), 0, "bad ICMP checksum");
    assert_eq!(p.payload, icmp_echo(0, 0x4242, seq, data));
}

/// A UDP datagram from the host's port `sport` to the machine's echo service, and a
/// check of what came back.
fn udp_echo(vm: &mut Vm, host: &mut Host, sport: u16, data: &[u8], with_checksum: bool) {
    let mut u = udp(HOST_IP, VM_IP, sport, 7, data);
    if !with_checksum {
        u[6..8].fill(0);
    }
    let back = exchange(
        vm,
        host,
        ether(VM_MAC, HOST_MAC, IP_TYPE, &ipv4(HOST_IP, VM_IP, 17, &u)),
    );
    assert_eq!(back.len(), 1, "{back:x?}");
    let p = parse_ipv4(&back[0]).unwrap();
    assert_eq!((p.src, p.dst, p.proto), (VM_IP, HOST_IP, 17));
    assert_eq!(
        p.payload,
        udp(VM_IP, HOST_IP, 7, sport, data),
        "a checksum or a byte is wrong"
    );
}

// ---- the tests ----

#[test]
fn user_networking_gets_a_lease_by_dhcp_and_pings_the_gateway() {
    // Issue #83's first criterion.
    let mut vm = Vm::new(vec![NetConfig::user(Vec::new())], Vec::new(), &[]);
    boot(&mut [&mut vm], None);
    let out = vm.out();
    assert!(
        out.contains("disk: none\nnet 0: mac 52:54:00:12:34:56, irq 12, link up\ntick 1\n"),
        "{out}"
    );
    assert!(
        out.contains(&format!(
            "tick 10\nnet: 10.0.2.15/24, gateway 10.0.2.2, dns 10.0.2.3 (dhcp)\n{SHELL_BANNER}"
        )),
        "{out}"
    );
    let vms = &mut [&mut vm];
    assert_eq!(
        mask_times(&command(vms, 0, None, "ping 10.0.2.2 3")),
        ping_ok("10.0.2.2", 3)
    );
    // Loopback, by name and by the interface's own address.
    assert_eq!(
        mask_times(&command(vms, 0, None, "ping 127.0.0.1 2")),
        ping_ok("127.0.0.1", 2)
    );
    assert_eq!(
        mask_times(&command(vms, 0, None, "ping 10.0.2.15 1")),
        ping_ok("10.0.2.15", 1)
    );
    let ifconfig = command(vms, 0, None, "ifconfig");
    assert!(
        ifconfig.starts_with(
            "net0: mac 52:54:00:12:34:56, link up\n  \
             inet 10.0.2.15 netmask 255.255.255.0 gateway 10.0.2.2 dns 10.0.2.3 (dhcp)\n"
        ),
        "{ifconfig}"
    );
    assert!(
        ifconfig.ends_with("lo: inet 127.0.0.1 netmask 255.0.0.0\n"),
        "{ifconfig}"
    );
    // Nothing was dropped. Sent: two DHCP messages, an ARP request for the gateway and
    // three echo requests; received their answers. Loopback never reaches the card.
    assert!(counters(&ifconfig).values().all(|&n| n == 0), "{ifconfig}");
    assert_eq!(traffic(&ifconfig), (6, 6), "{ifconfig}");
    assert_buffers_back(&mut vm);
    exit(&mut vm);
}

#[test]
fn grail_net_user_boots_camelot_onto_the_network() {
    // The same through the command line, as a user would run it.
    let kernel = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../kernel/kernel.elf");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_grail"))
        .args(["--max-steps", "400000000", "--ram", "16", "--net", "user"])
        .arg(kernel)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"ping 10.0.2.2 2\nexit\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let shell = &stdout[stdout.find(SHELL_BANNER).unwrap()..];
    assert_eq!(
        mask_times(shell),
        format!(
            "{SHELL_BANNER}{PROMPT}ping 10.0.2.2 2\n{}{PROMPT}exit\n",
            ping_ok("10.0.2.2", 2)
        )
    );
}

#[test]
fn the_udp_echo_service_answers_through_a_forwarded_port() {
    // UDP end to end through the gateway, which checks every checksum: a host program
    // sends to a forwarded port, the gateway passes the datagram to the kernel's echo
    // service on port 7, and the answer comes back the same way.
    //
    // The gateway runs on the machine's steps (SteppedGateway), so the machine's side
    // is deterministic. The two hops through host sockets are bounded instead: the
    // datagram is in the gateway's frame queue before the machine runs again, and the
    // machine's answer is handed to the host (a synchronous send) before this waits
    // for it, with a read timeout.
    let gw = gateway::Gateway::new(gateway::Config {
        hostfwd: vec![gateway::HostFwd::udp(0, 7)],
        // Host sockets serviced on every poll, so the gateway's clock need only follow
        // the machine's steps (SteppedGateway::wait_for_host).
        limits: gateway::Limits {
            service_interval: Duration::ZERO,
            ..gateway::Limits::default()
        },
        ..gateway::Config::default()
    })
    .unwrap();
    let forwarded = gw.hostfwd()[0].host;
    let gw = SteppedGateway::new(gw);
    let cards: Vec<(MacAddr, Box<dyn NetBackend>)> =
        vec![(MacAddr(gateway::DEFAULT_GUEST_MAC), Box::new(gw.clone()))];
    let mut vm = Vm::new(Vec::new(), cards, &[]);
    boot(&mut [&mut vm], None);
    assert!(vm.out().contains("net: 10.0.2.15/24"), "{}", vm.out());

    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.send_to(b"Hello, echo service!", forwarded).unwrap();
    gw.wait_for_host();

    // The echo, from 10.0.2.15 port 7, is what the machine sends next.
    let is_echo = |f: &Vec<u8>| {
        parse_ipv4(f).is_some_and(|p| p.src == [10, 0, 2, 15] && p.proto == 17 && p.payload[..2] == [0, 7])
    };
    let sent = gw.guest_frames().len();
    let mut steps = 0;
    while !gw.guest_frames()[sent..].iter().any(is_echo) {
        vm.m.run_for(SLICE, &mut |_| {});
        assert_eq!(vm.m.exit_code(), None, "the kernel stopped:\n{}", vm.out());
        steps += SLICE;
        assert!(steps < 10_000_000, "no echo from the kernel:\n{}", vm.out());
    }
    sock.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut buf = [0u8; 64];
    let (n, from) = sock.recv_from(&mut buf).unwrap();
    assert_eq!(
        (&buf[..n], from),
        (&b"Hello, echo service!"[..], forwarded.into())
    );

    let ifconfig = command(&mut [&mut vm], 0, None, "ifconfig");
    assert!(counters(&ifconfig).values().all(|&n| n == 0), "{ifconfig}");
    assert_buffers_back(&mut vm);
    exit(&mut vm);
}

#[test]
fn two_kernels_on_a_hub_ping_each_other() {
    // Issue #83's second criterion. `a` boots with a static address from the kernel's
    // configuration; `b` tries DHCP, finds no server on the hub, falls back to the
    // default static address, and gets its own with ifconfig.
    let hub = Hub::new();
    let mut a = Vm::new(
        vec![NetConfig::hub(&hub)],
        Vec::new(),
        &static_config([192, 168, 7, 1], [255, 255, 255, 0]),
    );
    let mut b = Vm::new(vec![NetConfig::hub(&hub)], Vec::new(), &[]);
    let vms = &mut [&mut a, &mut b];
    boot(vms, None);
    assert!(
        vms[0].out().contains("tick 10\nnet: 192.168.7.1/24 (static)\n"),
        "{}",
        vms[0].out()
    );
    assert!(
        vms[1].out().contains(
            "net 0: mac 02:ca:00:00:00:02, irq 12, link up\n\
             tick 1\n"
        ),
        "{}",
        vms[1].out()
    );
    assert!(
        vms[1].out().contains(
            "tick 10\nnet: no answer from DHCP\n\
             net: 10.0.2.15/24, gateway 10.0.2.2 (static)\n"
        ),
        "{}",
        vms[1].out()
    );
    assert_eq!(command(vms, 1, None, "ifconfig 192.168.7.2"), "");
    // What netconf refuses, and what the shell can't parse, change nothing: the subnet's
    // network and broadcast addresses, loopback, multicast, 0.0.0.0/8, masks that aren't
    // ones then zeros, a gateway off the subnet or the address itself.
    for (line, error) in [
        ("ifconfig 192.168.7.255", "invalid argument"),
        ("ifconfig 192.168.7.0", "invalid argument"),
        ("ifconfig 127.0.0.5", "invalid argument"),
        ("ifconfig 224.0.0.1", "invalid argument"),
        ("ifconfig 0.1.2.3", "invalid argument"),
        ("ifconfig 10.0.0.1 255.0.255.0", "invalid argument"),
        ("ifconfig 10.0.0.1 0.0.0.0", "invalid argument"),
        ("ifconfig 10.0.0.1 255.255.255.0 10.0.1.1", "invalid argument"),
        ("ifconfig 10.0.0.1 255.255.255.0 10.0.0.1", "invalid argument"),
        ("ifconfig foo", "foo: not an address"),
        ("ifconfig 10.0.0.256", "10.0.0.256: not an address"),
        ("ifconfig 10.0.0.1 255.255.255.0 x", "x: not an address"),
    ] {
        assert_eq!(
            command(vms, 1, None, line),
            format!("sh: ifconfig: {error}\n"),
            "{line}"
        );
    }
    assert_eq!(command(vms, 1, None, "ping x"), "sh: ping: x: not an address\n");
    let ifconfig = command(vms, 1, None, "ifconfig");
    assert!(
        ifconfig.contains("\n  inet 192.168.7.2 netmask 255.255.255.0 (static)\n"),
        "{ifconfig}"
    );

    assert_eq!(
        mask_times(&command(vms, 0, None, "ping 192.168.7.2 3")),
        ping_ok("192.168.7.2", 3)
    );
    assert_eq!(
        mask_times(&command(vms, 1, None, "ping 192.168.7.1 3")),
        ping_ok("192.168.7.1", 3)
    );
    // Off the subnet with no gateway: no route.
    assert_eq!(
        command(vms, 0, None, "ping 10.1.2.3"),
        "PING 10.1.2.3: 56 data bytes\nsh: ping: network unreachable\n"
    );

    for i in 0..2 {
        let ifconfig = command(vms, i, None, "ifconfig");
        let mut c = counters(&ifconfig);
        // `a` got b's four DHCP discovers, for a port it has no socket on; ARP requests
        // for the other were filtered; nothing else went wrong.
        assert_eq!(
            c.remove("no-port"),
            Some(if i == 0 { 4 } else { 0 }),
            "{ifconfig}"
        );
        c.remove("filtered");
        assert!(c.values().all(|&n| n == 0), "{ifconfig}");
    }
    assert_buffers_back(&mut a);
    assert_buffers_back(&mut b);
    exit(&mut a);
    exit(&mut b);
}

#[test]
fn malformed_frames_are_dropped_and_counted() {
    // Issue #83's third criterion: this test is the other host on the hub and sends
    // the kernel every kind of broken frame; each is dropped and counted under its
    // reason, nothing panics, and the stack still answers afterwards.
    let hub = Hub::new();
    let mut vm = Vm::new(
        vec![NetConfig::hub(&hub)],
        Vec::new(),
        &static_config(VM_IP, MASK),
    );
    let mut host = Host::new(&hub);
    boot(&mut [&mut vm], Some(&mut host));
    assert!(vm.out().contains("net: 10.0.9.2/24 (static)\n"), "{}", vm.out());
    assert!(host.take().is_empty(), "a static configuration sends nothing");

    // The kernel pings the host: it asks for the host's MAC by ARP, the host answers
    // that and the echo requests.
    let out = command(&mut [&mut vm], 0, Some(&mut host), "ping 10.0.9.1 2");
    assert_eq!(mask_times(&out), ping_ok("10.0.9.1", 2));
    let request = ether(
        [0xff; 6],
        VM_MAC,
        ARP_TYPE,
        &arp(1, VM_MAC, VM_IP, [0; 6], HOST_IP),
    );
    assert_eq!(host.asked, [request], "the ARP request");
    let sent = host.take();
    assert_eq!(sent.len(), 2, "two echo requests: {sent:x?}");

    // ARP from the host: answered, from the address asked about.
    let back = exchange(
        &mut vm,
        &mut host,
        ether(
            [0xff; 6],
            HOST_MAC,
            ARP_TYPE,
            &arp(1, HOST_MAC, HOST_IP, [0; 6], VM_IP),
        ),
    );
    assert_eq!(
        back,
        [ether(
            HOST_MAC,
            VM_MAC,
            ARP_TYPE,
            &arp(2, VM_MAC, VM_IP, HOST_MAC, HOST_IP)
        )]
    );
    echo(&mut vm, &mut host, 1);
    udp_echo(&mut vm, &mut host, 40000, b"UDP echo, with a checksum", true);
    udp_echo(&mut vm, &mut host, 40001, b"UDP echo, without one", false);

    let to_vm = |payload: &[u8]| ether(VM_MAC, HOST_MAC, IP_TYPE, payload);
    let good_icmp = icmp_echo(8, 7, 7, b"x");
    let good_ip = ipv4(HOST_IP, VM_IP, 1, &good_icmp);
    let mut want: HashMap<&str, u32> = HashMap::new();
    let mut drop = |vm: &mut Vm, host: &mut Host, counter: &'static str, what: &str, frame: Vec<u8>| {
        ignored(vm, host, what, frame);
        *want.entry(counter).or_default() += 1;
    };

    // Ethernet: for another MAC, an unknown EtherType.
    drop(
        &mut vm,
        &mut host,
        "filtered",
        "another MAC",
        ether([2, 0, 0, 0, 0, 0x55], HOST_MAC, IP_TYPE, &good_ip),
    );
    drop(
        &mut vm,
        &mut host,
        "ether",
        "IPv6",
        ether(VM_MAC, HOST_MAC, 0x86dd, &[0; 40]),
    );
    // ARP: truncated, not Ethernet, an unknown operation.
    let request = arp(1, HOST_MAC, HOST_IP, [0; 6], VM_IP);
    drop(
        &mut vm,
        &mut host,
        "arp",
        "short ARP",
        ether(VM_MAC, HOST_MAC, ARP_TYPE, &request[..20]),
    );
    let mut bad = request.clone();
    bad[1] = 6;
    drop(
        &mut vm,
        &mut host,
        "arp",
        "ARP for IEEE 802",
        ether(VM_MAC, HOST_MAC, ARP_TYPE, &bad),
    );
    let mut bad = request.clone();
    bad[7] = 9;
    drop(
        &mut vm,
        &mut host,
        "arp",
        "ARP operation 9",
        ether(VM_MAC, HOST_MAC, ARP_TYPE, &bad),
    );
    // IPv4 headers: nothing at all, truncated, version 6, a header length of 4 words,
    // a total length past the frame and one shorter than the header.
    drop(&mut vm, &mut host, "ip", "empty IPv4", to_vm(&[]));
    drop(
        &mut vm,
        &mut host,
        "ip",
        "10 bytes of IPv4",
        to_vm(&good_ip[..10]),
    );
    let mut bad = good_ip.clone();
    bad[0] = 0x65;
    fix_ip_checksum(&mut bad);
    drop(&mut vm, &mut host, "ip", "version 6", to_vm(&bad));
    let mut bad = good_ip.clone();
    bad[0] = 0x44;
    drop(&mut vm, &mut host, "ip", "IHL 4", to_vm(&bad));
    let mut bad = good_ip.clone();
    bad[2..4].copy_from_slice(&(good_ip.len() as u16 + 1).to_be_bytes());
    fix_ip_checksum(&mut bad);
    drop(
        &mut vm,
        &mut host,
        "ip",
        "total length past the frame",
        to_vm(&bad),
    );
    let mut bad = good_ip.clone();
    bad[2..4].copy_from_slice(&19u16.to_be_bytes());
    fix_ip_checksum(&mut bad);
    drop(&mut vm, &mut host, "ip", "total length 19", to_vm(&bad));
    let mut bad = good_ip.clone();
    bad[10] ^= 0x10;
    drop(&mut vm, &mut host, "ip-checksum", "header checksum", to_vm(&bad));
    // Fragments: the first (MF), and a later one (an offset).
    let mut bad = good_ip.clone();
    bad[6] = 0x20;
    fix_ip_checksum(&mut bad);
    drop(&mut vm, &mut host, "fragment", "first fragment", to_vm(&bad));
    let mut bad = good_ip.clone();
    bad[6..8].copy_from_slice(&[0, 0x10]);
    fix_ip_checksum(&mut bad);
    drop(&mut vm, &mut host, "fragment", "last fragment", to_vm(&bad));
    // For another address on the subnet; GRE (47). (kernel_tcp.rs has TCP's own.)
    drop(
        &mut vm,
        &mut host,
        "filtered",
        "another address",
        to_vm(&ipv4(HOST_IP, [10, 0, 9, 77], 1, &good_icmp)),
    );
    drop(
        &mut vm,
        &mut host,
        "protocol",
        "GRE",
        to_vm(&ipv4(HOST_IP, VM_IP, 47, &[0; 20])),
    );
    // ICMP: a bad checksum, truncated.
    let mut bad = good_icmp.clone();
    bad[8] ^= 1;
    drop(
        &mut vm,
        &mut host,
        "icmp",
        "ICMP checksum",
        to_vm(&ipv4(HOST_IP, VM_IP, 1, &bad)),
    );
    drop(
        &mut vm,
        &mut host,
        "icmp",
        "4 bytes of ICMP",
        to_vm(&ipv4(HOST_IP, VM_IP, 1, &good_icmp[..4])),
    );
    // UDP: truncated, a length past the packet, one shorter than the header, a bad
    // checksum; then good ones to a port nobody has, and from a port below 1024 to the
    // echo service (which must not answer it).
    let good_udp = udp(HOST_IP, VM_IP, 40000, 7, b"hello");
    drop(
        &mut vm,
        &mut host,
        "udp",
        "4 bytes of UDP",
        to_vm(&ipv4(HOST_IP, VM_IP, 17, &good_udp[..4])),
    );
    let mut bad = good_udp.clone();
    bad[4..6].copy_from_slice(&(good_udp.len() as u16 + 1).to_be_bytes());
    drop(
        &mut vm,
        &mut host,
        "udp",
        "UDP length past the packet",
        to_vm(&ipv4(HOST_IP, VM_IP, 17, &bad)),
    );
    let mut bad = good_udp.clone();
    bad[4..6].copy_from_slice(&7u16.to_be_bytes());
    drop(
        &mut vm,
        &mut host,
        "udp",
        "UDP length 7",
        to_vm(&ipv4(HOST_IP, VM_IP, 17, &bad)),
    );
    let mut bad = good_udp.clone();
    bad[9] ^= 1;
    drop(
        &mut vm,
        &mut host,
        "udp-checksum",
        "UDP checksum",
        to_vm(&ipv4(HOST_IP, VM_IP, 17, &bad)),
    );
    let to_port = udp(HOST_IP, VM_IP, 40000, 9999, b"anyone?");
    drop(
        &mut vm,
        &mut host,
        "no-port",
        "port 9999",
        to_vm(&ipv4(HOST_IP, VM_IP, 17, &to_port)),
    );
    let from_7 = udp(HOST_IP, VM_IP, 7, 7, b"loop?");
    drop(
        &mut vm,
        &mut host,
        "no-port",
        "echo from port 7",
        to_vm(&ipv4(HOST_IP, VM_IP, 17, &from_7)),
    );
    // Sources no host can have (RFC 1122 3.2.1.3), to the echo services: answering would
    // send the answer to everyone on the segment (a broadcast source), to a group, or to
    // this host itself.
    for src in [
        [10, 0, 9, 255],
        [255, 255, 255, 255],
        [224, 0, 0, 1],
        [240, 0, 0, 1],
        [127, 0, 0, 1],
        VM_IP,
    ] {
        drop(
            &mut vm,
            &mut host,
            "ip",
            &format!("echo request from {src:?}"),
            to_vm(&ipv4(src, VM_IP, 1, &good_icmp)),
        );
        let u = udp(src, VM_IP, 40000, 7, b"reflect me");
        drop(
            &mut vm,
            &mut host,
            "ip",
            &format!("UDP echo from {src:?}"),
            to_vm(&ipv4(src, VM_IP, 17, &u)),
        );
    }
    // From 0.0.0.0, a host with no address yet: taken, but nothing answers it.
    ignored(
        &mut vm,
        &mut host,
        "echo request from 0.0.0.0",
        to_vm(&ipv4([0; 4], VM_IP, 1, &good_icmp)),
    );
    let u = udp([0; 4], VM_IP, 40000, 7, b"reflect me");
    drop(
        &mut vm,
        &mut host,
        "no-port",
        "UDP echo from 0.0.0.0",
        to_vm(&ipv4([0; 4], VM_IP, 17, &u)),
    );
    // Lengths the card itself refuses: shorter than a header, longer than 1514.
    drop(&mut vm, &mut host, "card", "10-byte frame", vec![0xff; 10]);
    drop(
        &mut vm,
        &mut host,
        "card",
        "1600-byte frame",
        ether(VM_MAC, HOST_MAC, IP_TYPE, &[0; 1586]),
    );

    // Still alive: it answers as before.
    echo(&mut vm, &mut host, 2);
    udp_echo(&mut vm, &mut host, 40002, b"still here", true);

    let ifconfig = command(&mut [&mut vm], 0, Some(&mut host), "ifconfig");
    let got = counters(&ifconfig);
    for (name, n) in &got {
        assert_eq!(
            *n,
            want.get(name.as_str()).copied().unwrap_or(0),
            "{name}:\n{ifconfig}"
        );
    }
    assert_eq!(got.len(), 23, "{ifconfig}");
    assert!(want.keys().all(|name| got.contains_key(*name)), "{want:?}");
    // Every frame the card took counts as received; what it sent: the two echo requests
    // of its ping, the replies (ARP, two echoes, three UDP) and its ARP requests.
    assert_eq!(
        traffic(&ifconfig),
        (host.sent, 8 + host.asked.len() as u32),
        "{ifconfig}"
    );
    assert_buffers_back(&mut vm);
    exit(&mut vm);
}

#[test]
fn arp_gives_up_on_an_address_nobody_has() {
    // A ping to an address with no host behind it: ARP asks three times, a second
    // apart, then gives up; the pings time out, the packets waiting for an answer are
    // dropped and counted, and for a while the address is unreachable at once.
    let hub = Hub::new();
    let mut vm = Vm::new(
        vec![NetConfig::hub(&hub)],
        Vec::new(),
        &static_config(VM_IP, MASK),
    );
    let mut host = Host::new(&hub);
    boot(&mut [&mut vm], Some(&mut host));
    let out = command(&mut [&mut vm], 0, Some(&mut host), "ping 10.0.9.50 3");
    assert_eq!(
        out,
        "PING 10.0.9.50: 56 data bytes\nseq=1: timed out\nseq=2: timed out\nseq=3: timed out\n3 sent, 0 received\n"
    );
    let request = ether(
        [0xff; 6],
        VM_MAC,
        ARP_TYPE,
        &arp(1, VM_MAC, VM_IP, [0; 6], [10, 0, 9, 50]),
    );
    assert_eq!(
        host.take(),
        [request.clone(), request.clone(), request],
        "three ARP requests"
    );
    // ARP has given up by now: the next ping fails without waiting.
    let out = command(&mut [&mut vm], 0, Some(&mut host), "ping 10.0.9.50 1");
    assert_eq!(
        out,
        "PING 10.0.9.50: 56 data bytes\nseq=1: host unreachable\n1 sent, 0 received\n"
    );
    assert!(host.take().is_empty());
    let ifconfig = command(&mut [&mut vm], 0, Some(&mut host), "ifconfig");
    // Each ping's packet but the last was replaced in the ARP entry by the next one;
    // the last was dropped when ARP gave up; the fourth was refused.
    assert_eq!(counters(&ifconfig)["arp-failed"], 4, "{ifconfig}");
    assert_buffers_back(&mut vm);
    exit(&mut vm);
}

#[test]
fn arp_entries_expire() {
    // An answered address is good for ARP_TIMEOUT ticks (60 s), then asked again.
    let hub = Hub::new();
    let mut vm = Vm::new(
        vec![NetConfig::hub(&hub)],
        Vec::new(),
        &static_config(VM_IP, MASK),
    );
    let mut host = Host::new(&hub);
    boot(&mut [&mut vm], Some(&mut host));
    let vms = &mut [&mut vm];
    assert_eq!(
        mask_times(&command(vms, 0, Some(&mut host), "ping 10.0.9.1 1")),
        ping_ok("10.0.9.1", 1)
    );
    assert_eq!(host.asked.len(), 1, "the first ping asks");
    assert_eq!(
        mask_times(&command(vms, 0, Some(&mut host), "ping 10.0.9.1 1")),
        ping_ok("10.0.9.1", 1)
    );
    assert_eq!(
        host.asked.len(),
        1,
        "the second one finds the address in the cache"
    );
    // An idle machine skips ahead to its next timer interrupt, so a minute is cheap.
    assert_eq!(command(vms, 0, Some(&mut host), "sleep 61"), "");
    assert_eq!(
        mask_times(&command(vms, 0, Some(&mut host), "ping 10.0.9.1 1")),
        ping_ok("10.0.9.1", 1)
    );
    assert_eq!(host.asked.len(), 2, "after a minute the entry has expired");
    assert_buffers_back(&mut vm);
    exit(&mut vm);
}
