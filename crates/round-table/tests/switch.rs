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
        format!("NAME  PORTS  CAPTURE\nlan   1      {capture}\n")
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
        "NAME  PORTS  CAPTURE\nlan   0      no\n"
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
        "NAME  PORTS  CAPTURE\ndmz   0      no\nlan   2      no\n"
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
        "NAME  PORTS  CAPTURE\ndmz   0      no\n"
    );
    server.stop().unwrap();
}
