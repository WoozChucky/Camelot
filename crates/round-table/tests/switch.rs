//! Round Table's switch commands, and `create --net`, against an Avalon server running
//! in process: `examples/net` (ping and pong) on a switch made with Round Table, its
//! capture, plugging cards in and out, and the errors the server gives back.

mod common;

use avalon_proto::VmState;
use common::*;
use std::path::Path;
use std::time::{Duration, Instant};

fn net_example(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../examples/net/{name}.elf"));
    path.to_str().unwrap().to_string()
}

#[test]
fn ping_and_pong_on_a_switch() {
    let server = avalon(512);
    let addr = server.addr();
    assert_eq!(ok(addr, &["switch", "list"]), "no switches\n");
    assert_eq!(ok(addr, &["switch", "create", "lan"]), "created switch lan\n");
    assert!(refused(addr, &["switch", "create", "lan"]).starts_with("already exists: "));

    let pong = net_example("net-pong");
    assert_eq!(
        ok(
            addr,
            &[
                "create",
                "--name",
                "pong",
                "--ram",
                "16",
                "--net",
                "lan,mac=02:ca:00:00:00:02",
                "--start",
                &pong
            ]
        ),
        "created VM 1 (pong)\nstarted VM 1 (pong)\n"
    );
    // Pong is listening once it has run its driver's set-up.
    let start = Instant::now();
    while steps(addr, "pong") < 50_000 {
        assert!(start.elapsed() < SLOW, "pong does not run");
        std::thread::sleep(Duration::from_millis(5));
    }
    let info = ok(addr, &["info", "pong"]);
    assert!(
        info.contains("\ndisks    none\nnets     02:ca:00:00:00:02 on switch lan\ndevices  10\n"),
        "{info}"
    );
    assert!(
        info.contains("\n  net       0x10009000 + 0x00001000  irq 12\n"),
        "{info}"
    );
    assert_eq!(
        ok(addr, &["switch", "show", "lan"]),
        "\
switch   lan
capture  no
nat      no
ports    1
VM  CARD  MAC                IN  IN-BYTES  OUT  OUT-BYTES  DROPPED
1   0     02:ca:00:00:00:02  0   0         0    0          0
"
    );

    let capture = std::env::temp_dir().join(format!("round-table-{}-lan.pcap", std::process::id()));
    let capture = capture.to_str().unwrap();
    assert_eq!(
        ok(addr, &["switch", "capture", "lan", capture]),
        format!("capturing switch lan into {capture}\n")
    );
    assert_eq!(
        ok(addr, &["switch", "list"]),
        format!("NAME  PORTS  NAT  CAPTURE\nlan   1      no   {capture}\n")
    );
    // Ping at 1 MHz: its few million polls for the answer leave pong three seconds.
    assert_eq!(
        ok(
            addr,
            &[
                "create",
                &net_example("net-ping"),
                "--name=ping",
                "--ram=16",
                "--clock=1MHz",
                "--net=lan,mac=02:ca:00:00:00:01",
                "--start"
            ]
        ),
        "created VM 2 (ping)\nstarted VM 2 (ping)\n"
    );
    for vm in ["ping", "pong"] {
        assert_eq!(
            wait_for_state(addr, vm, |s| s != VmState::Running),
            VmState::Exited(0),
            "{vm}"
        );
    }
    assert_eq!(
        ok(addr, &["switch", "capture", "lan", "--stop"]),
        "stopped capturing switch lan\n"
    );
    // Header, then two 60-byte frames with their record headers.
    let bytes = std::fs::read(capture).unwrap();
    std::fs::remove_file(capture).unwrap();
    assert_eq!(bytes.len(), 24 + 2 * (16 + 60));
    // Both machines have ended: their cards are unplugged, but still listed.
    assert_eq!(
        ok(addr, &["switch", "list"]),
        "NAME  PORTS  NAT  CAPTURE\nlan   0      no   no\n"
    );
    assert!(ok(addr, &["info", "ping"]).contains("\nnets     02:ca:00:00:00:01 unplugged\n"));
    let list = ok(addr, &["list"]);
    assert!(
        list.contains("\n2   ping  exited(0)  16 MiB  1 MHz       0      1\n"),
        "{list}"
    );
    server.stop().unwrap();
}

#[test]
fn cards_plug_in_and_out() {
    let server = avalon(512);
    let addr = server.addr();
    for name in ["lan", "dmz"] {
        ok(addr, &["switch", "create", name]);
    }
    let echo = example_str("uart-echo");
    ok(
        addr,
        &[
            "create",
            "--name",
            "web",
            "--net",
            "-,mac=02:00:00:00:00:0a",
            "--net",
            "dmz,mac=02:00:00:00:00:0b",
            &echo,
        ],
    );
    assert!(ok(addr, &["info", "web"])
        .contains("\nnets     02:00:00:00:00:0a unplugged\n         02:00:00:00:00:0b on switch dmz\n"));

    assert_eq!(
        ok(addr, &["switch", "attach", "lan", "web"]),
        "plugged card 0 of VM web into switch lan\n"
    );
    assert_eq!(
        ok(addr, &["switch", "detach", "1", "--card", "1"]),
        "unplugged card 1 of VM 1\n"
    );
    assert_eq!(
        ok(addr, &["switch", "attach", "--card=1", "lan", "web"]),
        "plugged card 1 of VM web into switch lan\n"
    );
    assert!(ok(addr, &["info", "web"])
        .contains("\nnets     02:00:00:00:00:0a on switch lan\n         02:00:00:00:00:0b on switch lan\n"));
    assert_eq!(
        ok(addr, &["switch", "list"]),
        "NAME  PORTS  NAT  CAPTURE\ndmz   0      no   no\nlan   2      no   no\n"
    );

    // What the server refuses.
    for (args, why) in [
        (
            &["switch", "attach", "dmz", "web"][..],
            "busy: card 0 of VM 1 is plugged into switch 'lan'; detach it first",
        ),
        (
            &["switch", "attach", "wan", "web"],
            "no such switch: no switch named 'wan'",
        ),
        (&["switch", "attach", "lan", "db"], "no such VM: no VM named 'db'"),
        (
            &["switch", "attach", "lan", "web", "--card", "4"],
            "no such network card: VM 1 ('web') has 2 network cards, 0 to 1",
        ),
        (
            &["switch", "show", "wan"],
            "no such switch: no switch named 'wan'",
        ),
        (
            &["switch", "delete", "wan"],
            "no such switch: no switch named 'wan'",
        ),
        (
            &["switch", "capture", "lan", "--stop"],
            "invalid state: switch 'lan' is not being captured",
        ),
        (
            &["create", "--name", "db", "--net", "wan", &echo],
            "no such switch: no switch named 'wan'",
        ),
    ] {
        assert_eq!(refused(addr, args), why, "{args:?}");
    }
    assert_eq!(
        ok(addr, &["switch", "detach", "web"]),
        "unplugged card 0 of VM web\n"
    );
    assert!(refused(addr, &["switch", "detach", "web"]).starts_with("invalid state: "));

    // Deleting a switch unplugs what is left on it.
    assert_eq!(ok(addr, &["switch", "delete", "lan"]), "deleted switch lan\n");
    assert!(ok(addr, &["info", "web"])
        .contains("\nnets     02:00:00:00:00:0a unplugged\n         02:00:00:00:00:0b unplugged\n"));
    assert_eq!(
        ok(addr, &["switch", "list"]),
        "NAME  PORTS  NAT  CAPTURE\ndmz   0      no   no\n"
    );
    server.stop().unwrap();
}

/// The port `rule` (as Round Table prints it, `forwarding tcp:127.0.0.1:N-...`)
/// forwards from.
fn forwarded_port(line: &str) -> u16 {
    let host = line
        .strip_prefix("forwarding tcp:127.0.0.1:")
        .unwrap_or_else(|| panic!("{line}"));
    host.split('-').next().unwrap().parse().unwrap()
}

#[test]
fn a_switch_with_a_nat_gateway() {
    let server = avalon(512);
    let addr = server.addr();
    // Port 0: Avalon's host picks one, and Round Table says which.
    let created = ok(
        addr,
        &["switch", "create", "wan", "--nat", "--hostfwd", "tcp::0-:7"],
    );
    let mut lines = created.lines();
    assert_eq!(lines.next(), Some("created switch wan with a NAT gateway"));
    let port = forwarded_port(lines.next().unwrap());
    assert!(created.ends_with(&format!("{port}-10.0.2.15:7\n")), "{created}");
    assert_eq!(
        ok(addr, &["switch", "list"]),
        "NAME  PORTS  NAT  CAPTURE\nwan   1      yes  no\n"
    );

    // A VM on it gets the first lease (its address is made up from its name: VM
    // web's card 0 is 02:66:b7:0f:2e:09).
    ok(
        addr,
        &[
            "create",
            "--name",
            "web",
            "--ram",
            "16",
            "--net",
            "wan",
            "--start",
            &net_example("net-user"),
        ],
    );
    let start = Instant::now();
    let show = loop {
        let show = ok(addr, &["switch", "show", "wan"]);
        if show.contains("lease ") {
            break show;
        }
        assert!(start.elapsed() < SLOW, "no lease: {show}");
        std::thread::sleep(Duration::from_millis(5));
    };
    let mut lines = show.lines();
    assert_eq!(
        lines.by_ref().take(6).collect::<Vec<_>>(),
        [
            "switch   wan",
            "capture  no",
            "nat      10.0.2.0/24, gateway and host 10.0.2.2, DNS 10.0.2.3",
            &format!("forward  tcp:127.0.0.1:{port}-10.0.2.15:7"),
            "lease    10.0.2.15 02:66:b7:0f:2e:09",
            "ports    2",
        ]
    );
    let rows: Vec<&str> = lines.skip(1).collect();
    assert!(
        rows[0].starts_with("gateway  -     52:55:0a:00:02:02  "),
        "{show}"
    );
    assert!(
        rows[1].starts_with("1        0     02:66:b7:0f:2e:09  "),
        "{show}"
    );

    // One gateway per switch; the port is the gateway's while it lives.
    assert_eq!(
        refused(addr, &["switch", "nat", "wan"]),
        "already exists: switch 'wan' has a NAT gateway already; take it away first"
    );
    let rule = format!("tcp::{port}-:22");
    let taken = refused(addr, &["switch", "create", "lan", "--nat", "--hostfwd", &rule]);
    assert!(
        taken.starts_with(&format!(
            "I/O error: switch 'lan': cannot forward the port: hostfwd tcp:127.0.0.1:{port}-10.0.2.15:22: "
        )),
        "{taken}"
    );
    assert_eq!(
        refused(addr, &["switch", "nat", "lan", "--off"]),
        "no such switch: no switch named 'lan'"
    );

    // Taken away, the gateway lets go of its port; given back, it binds it again.
    assert_eq!(
        ok(addr, &["switch", "nat", "wan", "--off"]),
        "took the NAT gateway of switch wan away\n"
    );
    assert_eq!(
        refused(addr, &["switch", "nat", "wan", "--off"]),
        "invalid state: switch 'wan' has no NAT gateway"
    );
    assert!(ok(addr, &["switch", "show", "wan"]).contains("\nnat      no\nports    1\n"));
    assert_eq!(
        ok(addr, &["switch", "nat", "wan", "--hostfwd", &rule]),
        format!("gave switch wan a NAT gateway\nforwarding tcp:127.0.0.1:{port}-10.0.2.15:22\n")
    );

    // Deleting the switch frees the port for a new one.
    assert_eq!(ok(addr, &["switch", "delete", "wan"]), "deleted switch wan\n");
    assert_eq!(
        ok(addr, &["switch", "create", "lan", "--nat", "--hostfwd", &rule]),
        format!("created switch lan with a NAT gateway\nforwarding tcp:127.0.0.1:{port}-10.0.2.15:22\n")
    );
    server.stop().unwrap();
}
