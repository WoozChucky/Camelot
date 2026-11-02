//! Avalon's virtual switches end to end, in process and over the protocol: two VMs on
//! one switch run `examples/net` (ping and pong) and exchange frames, a VM on another
//! switch sees none of them, a capture records the exchange, and the switch requests
//! (create, delete, list, attach, detach, capture) with their errors. A VM that sends
//! as fast as it can is never held up by one that takes nothing.

mod common;

use avalon_proto::{
    DeviceSpec, ErrorCode, HardwareProfile, Message, NetSpec, PortOwner, SwitchInfo, VmRef, VmState,
};
use common::*;
use std::net::TcpStream;
use std::time::{Duration, Instant};

type Conn = avalon_proto::Connection<TcpStream>;

const PING_MAC: [u8; 6] = [0x02, 0xca, 0, 0, 0, 1];
const PONG_MAC: [u8; 6] = [0x02, 0xca, 0, 0, 0, 2];
const SPY_MAC: [u8; 6] = [0x02, 0xca, 0, 0, 0, 3];

/// What ping and pong print (as in grail's tests/net.rs, with the same addresses).
const PING_OUT: &str = "net 0x10009000, irq 12, mac 02:ca:00:00:00:01, link up
sent 60 bytes to ff:ff:ff:ff:ff:ff
received 60 bytes from 02:ca:00:00:00:02, type 0x88b5: Hello back from pong!
";

const PONG_OUT: &str = "net 0x10009000, irq 12, mac 02:ca:00:00:00:02, link up
received 60 bytes from 02:ca:00:00:00:01, type 0x88b5: Hello from ping!
sent 60 bytes to 02:ca:00:00:00:01
";

fn net_example(name: &str) -> Vec<u8> {
    std::fs::read(examples_dir().join(format!("net/{name}.elf"))).unwrap()
}

/// 16 MiB, at `clock_hz` (full speed if `None`), with these cards.
fn with_cards(clock_hz: Option<u32>, cards: &[(Option<&str>, Option<[u8; 6]>)]) -> HardwareProfile {
    HardwareProfile {
        devices: cards
            .iter()
            .map(|(switch, mac)| {
                DeviceSpec::Net(NetSpec {
                    switch: switch.map(str::to_string),
                    mac: *mac,
                })
            })
            .collect(),
        ..profile(16, clock_hz)
    }
}

fn ok(conn: &mut Conn, msg: Message) {
    conn.request_ok(&msg).unwrap_or_else(|e| panic!("{msg:?}: {e}"));
}

fn create_switch(conn: &mut Conn, name: &str) {
    ok(conn, Message::CreateSwitch { name: name.into() });
}

fn switches(conn: &mut Conn) -> Vec<SwitchInfo> {
    match conn.request(&Message::ListSwitches) {
        Ok(Message::SwitchList { switches }) => switches,
        other => panic!("{other:?}"),
    }
}

fn switch(conn: &mut Conn, name: &str) -> SwitchInfo {
    switches(conn)
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no switch {name}"))
}

/// The cards of VM `vm` as `VmInfo` shows them.
fn cards(conn: &mut Conn, vm: u32) -> Vec<NetSpec> {
    info(conn, vm)
        .vm
        .profile
        .devices
        .into_iter()
        .filter_map(|d| match d {
            DeviceSpec::Net(n) => Some(n),
            _ => None,
        })
        .collect()
}

fn card(vm: u32) -> PortOwner {
    PortOwner::Card { vm, card: 0 }
}

/// Starts `vm` and waits until its `net_init` is done: the line it prints, then enough
/// steps for the rings that follow it.
fn start_listening(server: &avalon::ServerThread, conn: &mut Conn, vm: u32) -> Console {
    let mut console = Console::attach(server, VmRef::Id(vm));
    ok(conn, Message::StartVm { vm: VmRef::Id(vm) });
    console.wait_for("link up\n");
    let start = Instant::now();
    while info(conn, vm).steps < 50_000 {
        assert!(start.elapsed() < SLOW, "VM {vm} does not run");
        std::thread::sleep(Duration::from_millis(5));
    }
    console
}

#[test]
fn two_vms_on_a_switch_exchange_frames_and_another_switch_sees_nothing() {
    let server = server(None);
    let mut rt = connect(&server);
    create_switch(&mut rt, "lan");
    create_switch(&mut rt, "dmz");

    // Pong and the spy wait in wfi for a frame, at full speed: a frame rings pong's
    // bell, which wakes it (the bell count below shows it rang).
    let pong = create(
        &mut rt,
        "pong",
        with_cards(None, &[(Some("lan"), Some(PONG_MAC))]),
        net_example("net-pong"),
    );
    let spy = create(
        &mut rt,
        "spy",
        with_cards(None, &[(Some("dmz"), Some(SPY_MAC))]),
        net_example("net-pong"),
    );
    // Ping polls for its answer a few million instructions, then gives up. At 1 MHz
    // that leaves pong three seconds to answer, however loaded the host is.
    let ping = create(
        &mut rt,
        "ping",
        with_cards(Some(1_000_000), &[(Some("lan"), Some(PING_MAC))]),
        net_example("net-ping"),
    );
    let capture = std::env::temp_dir().join(format!("avalon-net-{}-lan.pcap", std::process::id()));
    let capture_path = capture.to_str().unwrap().to_string();
    ok(
        &mut rt,
        Message::CaptureSwitch {
            switch: "lan".into(),
            path: Some(capture_path.clone()),
        },
    );

    let mut pong_console = start_listening(&server, &mut rt, pong);
    let mut spy_console = start_listening(&server, &mut rt, spy);
    let mut ping_console = Console::attach(&server, VmRef::Id(ping));
    ok(&mut rt, Message::StartVm { vm: VmRef::Id(ping) });

    ping_console.wait_for_event(Message::VmStateChanged {
        vm: ping,
        state: VmState::Exited(0),
    });
    pong_console.wait_for_event(Message::VmStateChanged {
        vm: pong,
        state: VmState::Exited(0),
    });
    assert_eq!(String::from_utf8_lossy(&ping_console.output), PING_OUT);
    assert_eq!(String::from_utf8_lossy(&pong_console.output), PONG_OUT);
    // Ping's frame rang pong's bell (and the reply ping's, if it was waiting); the spy
    // got nothing to ring for.
    assert!(server.debug_vm(pong).unwrap().bells >= 1);
    assert_eq!(server.debug_vm(spy).unwrap().bells, 0);

    // The spy, on the other switch, heard nothing and still waits.
    ok(&mut rt, Message::PauseVm { vm: VmRef::Id(spy) });
    spy_console.idle(Duration::from_millis(50));
    assert_eq!(
        String::from_utf8_lossy(&spy_console.output),
        "net 0x10009000, irq 12, mac 02:ca:00:00:00:03, link up\n"
    );
    let dmz = switch(&mut rt, "dmz");
    assert_eq!(dmz.ports.len(), 1);
    assert_eq!(dmz.ports[0].owner, card(spy));
    assert_eq!(
        (
            dmz.ports[0].in_frames,
            dmz.ports[0].out_frames,
            dmz.ports[0].dropped
        ),
        (0, 0, 0)
    );

    // Ping and pong have exited: their machines are gone, and their cards with them.
    let lan = switch(&mut rt, "lan");
    assert!(lan.ports.is_empty(), "{lan:?}");
    assert_eq!(lan.capture.as_deref(), Some(capture_path.as_str()));
    assert_eq!(
        cards(&mut rt, ping),
        [NetSpec {
            switch: None,
            mac: Some(PING_MAC)
        }]
    );

    // The capture has both frames, in order.
    ok(
        &mut rt,
        Message::CaptureSwitch {
            switch: "lan".into(),
            path: None,
        },
    );
    let bytes = std::fs::read(&capture).unwrap();
    std::fs::remove_file(&capture).unwrap();
    let frames = pcap_frames(&bytes);
    assert_eq!(frames.len(), 2, "{frames:x?}");
    assert_eq!(frames[0][..12], [[0xff; 6], PING_MAC].concat());
    assert_eq!(frames[1][..12], [PING_MAC, PONG_MAC].concat());
    let text = |f: &[u8]| {
        String::from_utf8_lossy(&f[14..])
            .trim_end_matches('\0')
            .to_string()
    };
    assert_eq!(text(&frames[0]), "Hello from ping!");
    assert_eq!(text(&frames[1]), "Hello back from pong!");
}

/// The frames of a pcap file, after its 24-byte header.
fn pcap_frames(bytes: &[u8]) -> Vec<Vec<u8>> {
    assert_eq!(bytes[..4], 0xa1b2_c3d4u32.to_le_bytes());
    let mut rest = &bytes[24..];
    let mut frames = Vec::new();
    while !rest.is_empty() {
        let len = u32::from_le_bytes(rest[8..12].try_into().unwrap()) as usize;
        frames.push(rest[16..16 + len].to_vec());
        rest = &rest[16 + len..];
    }
    frames
}

/// Sends 1000 broadcast frames as fast as its card takes them, then exits: card 0's
/// transmit ring is one descriptor, which the card sends during each store to
/// `NET_TX_TAIL`.
fn flooder() -> Vec<u8> {
    guest(
        "
    .globl _start
_start:
    li    s0, 0x10009000
    la    t0, desc
    sw    t0, 0x20(s0)          # NET_TX_BASE
    li    t1, 1
    sw    t1, 0x24(s0)          # NET_TX_SIZE
    la    t2, frame
    sw    t2, 0(t0)             # the descriptor: the frame's address
    li    t3, 60
    sh    t3, 4(t0)             # and length
    li    s1, 0
    li    s2, 1000
1:  addi  s1, s1, 1
    sw    s1, 0x2c(s0)          # NET_TX_TAIL: sends it
    bne   s1, s2, 1b
    li    a0, 0
    xcal.halt a0

    .data
    .balign 4
desc:
    .space 8
frame:
    .byte 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02, 0, 0, 0, 0, 0x10, 0x88, 0xb5
    .space 46
",
    )
}

#[test]
fn a_vm_that_takes_nothing_holds_nobody_up() {
    let server = server(None);
    let mut rt = connect(&server);
    create_switch(&mut rt, "lan");
    // Created, never started: its card is plugged in, and nothing takes its frames.
    let idle = create(
        &mut rt,
        "idle",
        with_cards(None, &[(Some("lan"), None)]),
        spinner(),
    );
    let flood = create(
        &mut rt,
        "flood",
        with_cards(None, &[(Some("lan"), None)]),
        flooder(),
    );
    ok(&mut rt, Message::StartVm { vm: VmRef::Id(flood) });
    wait_for_state(&mut rt, flood, |s| s == VmState::Exited(0));
    // Only the idle VM's port lost frames: those past its queue.
    let lan = switch(&mut rt, "lan");
    assert_eq!(lan.ports.len(), 1, "the flooder's card went with its machine");
    let port = lan.ports[0];
    assert_eq!(port.owner, card(idle));
    assert_eq!(port.out_frames, avalon::switch::PORT_QUEUE as u64);
    assert_eq!(port.out_bytes, 60 * avalon::switch::PORT_QUEUE as u64);
    assert_eq!(port.dropped, 1000 - avalon::switch::PORT_QUEUE as u64);
}

/// Once a client has heard that a VM's machine has ended (its `StopVm` was answered, or
/// it saw the exit), the cards are unplugged: no port on the switch, no switch in
/// `VmInfo`, and plugging one in is refused. The machine itself goes a moment later, on
/// the VM's thread; the requests come at once, and the VMs are big (freeing their RAM
/// takes a while), so that a card left to go with its machine would be seen.
#[test]
fn a_vm_whose_machine_ended_has_unplugged_its_cards_already() {
    let server = server(None);
    let mut rt = connect(&server);
    create_switch(&mut rt, "lan");
    create_switch(&mut rt, "dmz");
    let attach = |vm, switch: &str| Message::AttachCard {
        vm: VmRef::Id(vm),
        card: 0,
        switch: switch.into(),
    };
    let big = |switch| HardwareProfile {
        ram_size: 256 * MIB,
        ..with_cards(None, &[(switch, None)])
    };
    for i in 0..10 {
        // Stopped while running.
        let vm = create(&mut rt, &format!("stopped-{i}"), big(None), spinner());
        ok(&mut rt, Message::StartVm { vm: VmRef::Id(vm) });
        ok(&mut rt, attach(vm, "dmz"));
        ok(&mut rt, Message::StopVm { vm: VmRef::Id(vm) });
        assert_eq!(error_of(&mut rt, &attach(vm, "lan")), ErrorCode::InvalidState);
        assert!(switch(&mut rt, "dmz").ports.is_empty(), "{i}");
        assert_eq!(cards(&mut rt, vm)[0].switch, None);
        ok(&mut rt, Message::DestroyVm { vm: VmRef::Id(vm) });

        // Exited: the first `VmInfo` that says so has the card off its switch.
        let vm = create(&mut rt, &format!("exits-{i}"), big(Some("lan")), exits_with(0));
        ok(&mut rt, Message::StartVm { vm: VmRef::Id(vm) });
        let start = Instant::now();
        let details = loop {
            let details = info(&mut rt, vm);
            if details.vm.state != VmState::Running {
                break details;
            }
            assert!(start.elapsed() < SLOW, "VM {vm} does not exit");
        };
        assert_eq!(details.vm.state, VmState::Exited(0));
        let DeviceSpec::Net(card) = &details.vm.profile.devices[0] else {
            panic!("{details:?}")
        };
        assert_eq!(card.switch, None, "{i}");
        assert!(switch(&mut rt, "lan").ports.is_empty(), "{i}");
        assert_eq!(error_of(&mut rt, &attach(vm, "lan")), ErrorCode::InvalidState);
        ok(&mut rt, Message::DestroyVm { vm: VmRef::Id(vm) });
    }
}

#[test]
fn cards_get_stable_addresses_and_plug_in_and_out() {
    let server = server(None);
    let mut rt = connect(&server);
    create_switch(&mut rt, "lan");
    create_switch(&mut rt, "dmz");
    let web = create(
        &mut rt,
        "web",
        with_cards(None, &[(Some("lan"), None), (None, None)]),
        spinner(),
    );
    let generated = |i| Some(avalon::switch::generated_mac("web", i).0);
    assert_eq!(
        cards(&mut rt, web),
        [
            NetSpec {
                switch: Some("lan".into()),
                mac: generated(0)
            },
            NetSpec {
                switch: None,
                mac: generated(1)
            },
        ]
    );
    // The guest sees two cards.
    let nets = info(&mut rt, web).devices.iter().filter(|d| d.kind == 11).count();
    assert_eq!(nets, 2);

    // Card 1 goes into dmz; card 0 moves from lan to dmz.
    ok(
        &mut rt,
        Message::AttachCard {
            vm: VmRef::Name("web".into()),
            card: 1,
            switch: "dmz".into(),
        },
    );
    ok(
        &mut rt,
        Message::DetachCard {
            vm: VmRef::Id(web),
            card: 0,
        },
    );
    assert!(switch(&mut rt, "lan").ports.is_empty());
    ok(
        &mut rt,
        Message::AttachCard {
            vm: VmRef::Id(web),
            card: 0,
            switch: "dmz".into(),
        },
    );
    let dmz = switch(&mut rt, "dmz");
    assert_eq!(
        dmz.ports.iter().map(|p| (p.owner, p.mac)).collect::<Vec<_>>(),
        [
            (PortOwner::Card { vm: web, card: 1 }, generated(1).unwrap()),
            (PortOwner::Card { vm: web, card: 0 }, generated(0).unwrap()),
        ]
    );
    assert_eq!(
        cards(&mut rt, web)
            .iter()
            .map(|c| c.switch.as_deref())
            .collect::<Vec<_>>(),
        [Some("dmz"), Some("dmz")]
    );

    // Deleting the switch unplugs both; the VM keeps its cards.
    ok(&mut rt, Message::DeleteSwitch { name: "dmz".into() });
    assert_eq!(
        cards(&mut rt, web),
        [
            NetSpec {
                switch: None,
                mac: generated(0)
            },
            NetSpec {
                switch: None,
                mac: generated(1)
            },
        ]
    );
    assert_eq!(
        switches(&mut rt)
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["lan"]
    );

    // The same name gets the same addresses again, after a destroy.
    ok(&mut rt, Message::DestroyVm { vm: VmRef::Id(web) });
    let again = create(
        &mut rt,
        "web",
        with_cards(None, &[(Some("lan"), None)]),
        spinner(),
    );
    assert_eq!(cards(&mut rt, again)[0].mac, generated(0));
    assert_eq!(switch(&mut rt, "lan").ports[0].mac, generated(0).unwrap());
    // Stopping a VM ends its machine, and unplugs its cards for good.
    ok(&mut rt, Message::StopVm { vm: VmRef::Id(again) });
    assert!(switch(&mut rt, "lan").ports.is_empty());
    assert_eq!(cards(&mut rt, again)[0].switch, None);
    assert_eq!(
        error_of(
            &mut rt,
            &Message::AttachCard {
                vm: VmRef::Id(again),
                card: 0,
                switch: "lan".into()
            }
        ),
        ErrorCode::InvalidState
    );
}

#[test]
fn switch_requests_refuse_what_cannot_be_done() {
    let server = server(None);
    let mut rt = connect(&server);
    assert!(switches(&mut rt).is_empty());
    create_switch(&mut rt, "lan");
    let e = |rt: &mut Conn, msg: Message| error_of(rt, &msg);

    // Names.
    for bad in ["", "LAN", "con", "42", "a/b"] {
        assert_eq!(
            e(&mut rt, Message::CreateSwitch { name: bad.into() }),
            ErrorCode::InvalidName,
            "{bad:?}"
        );
    }
    assert_eq!(
        e(&mut rt, Message::CreateSwitch { name: "lan".into() }),
        ErrorCode::AlreadyExists
    );
    for missing in [
        Message::DeleteSwitch { name: "wan".into() },
        Message::CaptureSwitch {
            switch: "wan".into(),
            path: None,
        },
    ] {
        assert_eq!(e(&mut rt, missing), ErrorCode::NoSuchSwitch);
    }

    // A profile naming a switch that does not exist creates nothing.
    assert_eq!(
        error_of(
            &mut rt,
            &Message::CreateVm {
                name: "web".into(),
                profile: with_cards(None, &[(Some("wan"), None)]),
                program: spinner(),
            }
        ),
        ErrorCode::NoSuchSwitch
    );
    // Five cards, or an address that is not a card's, are not a machine.
    for profile in [
        with_cards(None, &[(None, None); 5]),
        with_cards(None, &[(None, Some([0x01, 0, 0x5e, 0, 0, 1]))]),
        with_cards(None, &[(None, Some([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]))]),
    ] {
        assert_eq!(
            error_of(
                &mut rt,
                &Message::CreateVm {
                    name: "web".into(),
                    profile,
                    program: spinner(),
                }
            ),
            ErrorCode::InvalidProfile
        );
    }
    // Two cards with one address on one switch: the second VM is not created.
    let mac = Some([0x02, 0, 0, 0, 0, 0x99]);
    let first = create(
        &mut rt,
        "first",
        with_cards(None, &[(Some("lan"), mac)]),
        spinner(),
    );
    assert_eq!(
        error_of(
            &mut rt,
            &Message::CreateVm {
                name: "twin".into(),
                profile: with_cards(None, &[(Some("lan"), mac)]),
                program: spinner(),
            }
        ),
        ErrorCode::AlreadyExists
    );
    assert_eq!(
        error_of(
            &mut rt,
            &Message::GetVmInfo {
                vm: VmRef::Name("twin".into())
            }
        ),
        ErrorCode::NoSuchVm
    );
    assert_eq!(switch(&mut rt, "lan").ports.len(), 1);
    // The name and the RAM are free again.
    let twin = create(&mut rt, "twin", with_cards(None, &[(None, mac)]), spinner());

    // Cards.
    let plug = |vm: VmRef, card: u8, switch: &str| Message::AttachCard {
        vm,
        card,
        switch: switch.into(),
    };
    assert_eq!(e(&mut rt, plug(VmRef::Id(99), 0, "lan")), ErrorCode::NoSuchVm);
    assert_eq!(e(&mut rt, plug(VmRef::Id(twin), 1, "lan")), ErrorCode::NoSuchCard);
    assert_eq!(
        e(&mut rt, plug(VmRef::Id(twin), 0, "wan")),
        ErrorCode::NoSuchSwitch
    );
    assert_eq!(
        e(&mut rt, plug(VmRef::Id(twin), 0, "lan")),
        ErrorCode::AlreadyExists
    );
    assert_eq!(e(&mut rt, plug(VmRef::Id(first), 0, "lan")), ErrorCode::Busy);
    assert_eq!(
        e(
            &mut rt,
            Message::DetachCard {
                vm: VmRef::Id(twin),
                card: 0
            }
        ),
        ErrorCode::InvalidState
    );
    assert_eq!(
        e(
            &mut rt,
            Message::DetachCard {
                vm: VmRef::Id(twin),
                card: 4
            }
        ),
        ErrorCode::NoSuchCard
    );
    let plain = create(&mut rt, "plain", profile(16, None), spinner());
    match rt.request(&plug(VmRef::Id(plain), 0, "lan")) {
        Err(avalon_proto::Error::Remote { code, message }) => {
            assert_eq!(code, ErrorCode::NoSuchCard);
            assert_eq!(message, format!("VM {plain} ('plain') has no network cards"));
        }
        other => panic!("{other:?}"),
    }

    // Captures.
    let path = std::env::temp_dir().join(format!("avalon-net-{}-errors.pcap", std::process::id()));
    let capture = |path: Option<&std::path::Path>| Message::CaptureSwitch {
        switch: "lan".into(),
        path: path.map(|p| p.to_str().unwrap().to_string()),
    };
    assert_eq!(e(&mut rt, capture(None)), ErrorCode::InvalidState);
    ok(&mut rt, capture(Some(&path)));
    assert_eq!(e(&mut rt, capture(Some(&path))), ErrorCode::Busy);
    ok(&mut rt, capture(None));
    std::fs::remove_file(&path).unwrap();
    let nowhere = path.join("no").join("such.pcap");
    assert_eq!(e(&mut rt, capture(Some(&nowhere))), ErrorCode::Io);

    // A server has at most 64 switches.
    for i in 1..avalon::switch::MAX_SWITCHES {
        create_switch(&mut rt, &format!("s{i}"));
    }
    assert_eq!(
        e(
            &mut rt,
            Message::CreateSwitch {
                name: "one-more".into()
            }
        ),
        ErrorCode::OverBudget
    );
    ok(&mut rt, Message::DeleteSwitch { name: "s1".into() });
    create_switch(&mut rt, "one-more");
    assert_eq!(switches(&mut rt).len(), avalon::switch::MAX_SWITCHES);
}
