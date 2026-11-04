//! An in-process Avalon on an ephemeral port, driven through the protocol: VMs with
//! different hardware profiles, the life cycle (start, pause, resume, stop, destroy),
//! consoles (scrollback, live output, input, several clients), and shutdown.

mod common;

use avalon_proto::{
    AttachKind, DeviceEntry, DeviceSpec, DiskSource, DiskSpec, ErrorCode, HardwareProfile, Message, VmRef,
    VmState, VmSummary,
};
use common::*;
use std::time::{Duration, Instant};

#[test]
fn two_vms_with_their_own_profiles() {
    let server = server(None);
    let mut rt = connect(&server);
    let small = create(&mut rt, "small", profile(64, None), example("sysinfo"));
    let paced = create(&mut rt, "paced", profile(32, Some(5_000_000)), example("sysinfo"));
    assert_eq!((small, paced), (1, 2));
    assert_eq!(state(&mut rt, small), VmState::Created);

    // Attached before the start, so the whole output streams live.
    let mut small_console = Console::attach(&server, VmRef::Name("small".into()));
    let mut paced_console = Console::attach(&server, VmRef::Id(paced));
    assert!(small_console.scrollback.is_empty());
    assert_eq!((small_console.vm, paced_console.vm), (small, paced));

    rt.request_ok(&Message::StartVm { vm: VmRef::Id(small) }).unwrap();
    rt.request_ok(&Message::StartVm {
        vm: VmRef::Name("paced".into()),
    })
    .unwrap();

    let small_out = sysinfo_output(64, 10_000_000, false);
    let paced_out = sysinfo_output(32, 5_000_000, true);
    small_console.wait_for_event(Message::VmStateChanged {
        vm: small,
        state: VmState::Exited(0),
    });
    paced_console.wait_for_event(Message::VmStateChanged {
        vm: paced,
        state: VmState::Exited(0),
    });
    assert_eq!(String::from_utf8_lossy(&small_console.output), small_out);
    assert_eq!(String::from_utf8_lossy(&paced_console.output), paced_out);
    assert_eq!(
        small_console.events,
        [
            Message::VmStateChanged {
                vm: small,
                state: VmState::Running
            },
            Message::VmStateChanged {
                vm: small,
                state: VmState::Exited(0)
            }
        ]
    );

    // The scrollback keeps it for anyone who attaches later.
    let late = Console::attach(&server, VmRef::Id(paced));
    assert_eq!(String::from_utf8_lossy(&late.scrollback), paced_out);
    let late = Console::attach(&server, VmRef::Id(small));
    assert_eq!(String::from_utf8_lossy(&late.scrollback), small_out);

    // Each VM reports its own profile, the device table its guest read, and its steps.
    let details = info(&mut rt, paced);
    assert_eq!(details.vm.profile, profile(32, Some(5_000_000)));
    assert_eq!(details.vm.state, VmState::Exited(0));
    assert_eq!(details.vm.name, "paced");
    let config = avalon::machine_config(&details.vm.profile, &[]).unwrap();
    let expected: Vec<DeviceEntry> = grail::bus::device_table(&config, 0)
        .iter()
        .map(|d| DeviceEntry {
            kind: d.kind as u32,
            base: u32::try_from(d.base).expect("devices are below 4 GiB"),
            size: u32::try_from(d.size).expect("devices are below 4 GiB"),
            irq: d.irq,
        })
        .collect();
    assert_eq!(details.devices, expected);
    assert_eq!(details.devices.len(), 9);
    assert!(details.steps > 1000, "{}", details.steps);
    let small_steps = info(&mut rt, small).steps;
    assert!(small_steps > 1000);

    match rt.request(&Message::ListVms).unwrap() {
        Message::VmList { vms } => assert_eq!(
            vms,
            [
                VmSummary {
                    id: small,
                    name: "small".into(),
                    state: VmState::Exited(0),
                    profile: profile(64, None),
                },
                VmSummary {
                    id: paced,
                    name: "paced".into(),
                    state: VmState::Exited(0),
                    profile: profile(32, Some(5_000_000)),
                },
            ]
        ),
        other => panic!("{other:?}"),
    }

    // An exited VM cannot be restarted.
    for msg in [
        Message::StartVm { vm: VmRef::Id(small) },
        Message::PauseVm { vm: VmRef::Id(small) },
        Message::ResumeVm { vm: VmRef::Id(small) },
        Message::StopVm { vm: VmRef::Id(small) },
    ] {
        assert_eq!(error_of(&mut rt, &msg), ErrorCode::InvalidState);
    }
    server.stop().unwrap();
}

#[test]
fn an_rv64_vm_has_more_ram_than_the_host() {
    // 512 GiB, more than any machine running these tests has: RAM is reserved, not
    // allocated, so the VM is made and runs, its host paying for what it touches. An
    // RV64 VM (its program is ELF64) takes it; an RV32 one is refused as a bad profile.
    let server = server(None);
    let mut conn = connect(&server);
    let vm = create(
        &mut conn,
        "big",
        HardwareProfile::with_ram(512 << 30),
        rv64_spinner(),
    );
    assert_eq!(info(&mut conn, vm).vm.profile.ram_size, 512 << 30);
    match conn.request(&Message::StartVm { vm: VmRef::Id(vm) }) {
        Ok(Message::Ok) => {}
        other => panic!("start: {other:?}"),
    }
    wait_until("the VM to run", Duration::from_secs(10), || {
        slices(&server, vm) > 0
    });
    let refused = error_of(
        &mut conn,
        &Message::CreateVm {
            name: "big32".into(),
            profile: HardwareProfile::with_ram(512 << 30),
            program: example("hello"),
        },
    );
    assert_eq!(refused, avalon_proto::ErrorCode::InvalidProfile);
}

#[test]
fn exit_codes_are_reported() {
    let server = server(None);
    let mut rt = connect(&server);
    let id = create(&mut rt, "seven", profile(1, None), exits_with(7));
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    assert_eq!(
        wait_for_state(&mut rt, id, |s| s != VmState::Running),
        VmState::Exited(7)
    );
    let console = Console::attach(&server, VmRef::Id(id));
    assert_eq!(console.scrollback, b"bye");
    server.stop().unwrap();
}

#[test]
fn pause_resume_stop_and_destroy_a_looping_guest() {
    let server = server(Some(64 * MIB));
    let mut rt = connect(&server);
    let id = create(&mut rt, "spin", profile(48, None), spinner());
    let mut console = Console::attach(&server, VmRef::Name("spin".into()));
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    console.wait_for("L");

    let steps = |rt: &mut _| info(rt, id).steps;
    let wait_for_steps_above = |rt: &mut _, n: u64| {
        let start = Instant::now();
        while steps(rt) <= n {
            assert!(start.elapsed() < SLOW, "the VM is not running");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    wait_for_steps_above(&mut rt, 1_000_000);

    // Paused: not a single step more once PauseVm is answered.
    rt.request_ok(&Message::PauseVm { vm: VmRef::Id(id) }).unwrap();
    assert_eq!(state(&mut rt, id), VmState::Paused);
    let paused_at = steps(&mut rt);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(steps(&mut rt), paused_at);
    assert_eq!(
        error_of(&mut rt, &Message::PauseVm { vm: VmRef::Id(id) }),
        ErrorCode::InvalidState
    );
    assert_eq!(
        error_of(&mut rt, &Message::StartVm { vm: VmRef::Id(id) }),
        ErrorCode::InvalidState
    );

    rt.request_ok(&Message::ResumeVm { vm: VmRef::Id(id) }).unwrap();
    assert_eq!(state(&mut rt, id), VmState::Running);
    wait_for_steps_above(&mut rt, paused_at + 1_000_000);
    assert_eq!(
        error_of(&mut rt, &Message::ResumeVm { vm: VmRef::Id(id) }),
        ErrorCode::InvalidState
    );

    rt.request_ok(&Message::StopVm { vm: VmRef::Id(id) }).unwrap();
    assert_eq!(state(&mut rt, id), VmState::Stopped);
    let stopped_at = steps(&mut rt);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(steps(&mut rt), stopped_at);
    for msg in [
        Message::StartVm { vm: VmRef::Id(id) },
        Message::ResumeVm { vm: VmRef::Id(id) },
        Message::PauseVm { vm: VmRef::Id(id) },
        Message::StopVm { vm: VmRef::Id(id) },
    ] {
        assert_eq!(error_of(&mut rt, &msg), ErrorCode::InvalidState);
    }

    // The console saw every state change, in order.
    let changes = [
        VmState::Running,
        VmState::Paused,
        VmState::Running,
        VmState::Stopped,
    ];
    console.wait_until(|c| c.events.len() == changes.len(), "four state changes");
    let expected: Vec<Message> = changes
        .iter()
        .map(|&state| Message::VmStateChanged { vm: id, state })
        .collect();
    assert_eq!(console.events, expected);
    assert_eq!(console.output, b"L");

    // The VM still holds its RAM until it is destroyed.
    assert_eq!(
        error_of(
            &mut rt,
            &Message::CreateVm {
                name: "other".into(),
                profile: profile(32, None),
                program: spinner(),
            }
        ),
        ErrorCode::OverBudget
    );

    rt.request_ok(&Message::DestroyVm {
        vm: VmRef::Name("spin".into()),
    })
    .unwrap();
    console.wait_for_event(Message::Detached {
        vm: id,
        kind: AttachKind::Console,
    });
    for msg in [
        Message::GetVmInfo { vm: VmRef::Id(id) },
        Message::DestroyVm { vm: VmRef::Id(id) },
        Message::StartVm {
            vm: VmRef::Name("spin".into()),
        },
        Message::ConsoleAttach { vm: VmRef::Id(id) },
    ] {
        assert_eq!(error_of(&mut rt, &msg), ErrorCode::NoSuchVm);
    }
    assert_eq!(
        rt.request(&Message::ListVms).unwrap(),
        Message::VmList { vms: vec![] }
    );

    // Its RAM and its name are free again; ids are not reused.
    let again = create(&mut rt, "spin", profile(48, None), spinner());
    assert_eq!(again, id + 1);
    // Destroying a running VM works too.
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(again) }).unwrap();
    rt.request_ok(&Message::DestroyVm { vm: VmRef::Id(again) })
        .unwrap();
    create(&mut rt, "other", profile(32, None), spinner());
    server.stop().unwrap();
}

#[test]
fn stop_and_destroy_from_every_state() {
    let server = server(None);
    let mut rt = connect(&server);
    // Created → stopped.
    let a = create(&mut rt, "a", profile(1, None), spinner());
    rt.request_ok(&Message::StopVm { vm: VmRef::Id(a) }).unwrap();
    assert_eq!(state(&mut rt, a), VmState::Stopped);
    assert_eq!(info(&mut rt, a).steps, 0);
    assert_eq!(
        error_of(&mut rt, &Message::StartVm { vm: VmRef::Id(a) }),
        ErrorCode::InvalidState
    );
    // Paused → stopped.
    let b = create(&mut rt, "b", profile(1, None), spinner());
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(b) }).unwrap();
    rt.request_ok(&Message::PauseVm { vm: VmRef::Id(b) }).unwrap();
    rt.request_ok(&Message::StopVm { vm: VmRef::Id(b) }).unwrap();
    assert_eq!(state(&mut rt, b), VmState::Stopped);
    // Destroy: created, paused, running, stopped and exited VMs.
    let c = create(&mut rt, "c", profile(1, None), spinner());
    let d = create(&mut rt, "d", profile(1, None), spinner());
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(d) }).unwrap();
    rt.request_ok(&Message::PauseVm { vm: VmRef::Id(d) }).unwrap();
    let e = create(&mut rt, "e", profile(1, Some(1000)), spinner());
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(e) }).unwrap();
    let f = create(&mut rt, "f", profile(1, None), exits_with(0));
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(f) }).unwrap();
    wait_for_state(&mut rt, f, |s| s == VmState::Exited(0));
    for id in [a, b, c, d, e, f] {
        rt.request_ok(&Message::DestroyVm { vm: VmRef::Id(id) }).unwrap();
    }
    assert_eq!(
        rt.request(&Message::ListVms).unwrap(),
        Message::VmList { vms: vec![] }
    );
    server.stop().unwrap();
}

/// Input from attached clients reaches the UART; output goes to every attached
/// client. `examples/uart-echo` echoes each byte from its interrupt handler and halts
/// on EOT.
fn echo_session(clock_hz: Option<u32>) {
    let server = server(None);
    let mut rt = connect(&server);
    let id = create(&mut rt, "echo", profile(1, clock_hz), example("uart-echo"));
    let mut first = Console::attach(&server, VmRef::Name("echo".into()));
    let mut second = Console::attach(&server, VmRef::Id(id));
    // Input before the start waits for the guest.
    first.send(b"early ");
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    first.wait_for("early ");
    second.send(b"hi\r\n");
    first.wait_for("early hi\r\n");
    second.wait_for("early hi\r\n");

    // Input from a client that is not attached is dropped.
    let mut stranger = connect(&server);
    stranger
        .send(&Message::ConsoleInput {
            vm: id,
            data: b"stranger".to_vec(),
        })
        .unwrap();
    assert_eq!(
        stranger.request(&Message::Ping { token: 1 }).unwrap(),
        Message::Pong { token: 1 }
    );

    // Paused, the guest sees nothing; resumed, it gets the input.
    rt.request_ok(&Message::PauseVm { vm: VmRef::Id(id) }).unwrap();
    first.send(b"x");
    first.idle(Duration::from_millis(150));
    assert_eq!(first.output, b"early hi\r\n");
    rt.request_ok(&Message::ResumeVm { vm: VmRef::Id(id) }).unwrap();
    first.wait_for("early hi\r\nx");

    // Detached, the second client gets no more output; attaching again shows the
    // scrollback.
    assert_eq!(second.request(&Message::ConsoleDetach { vm: id }), Message::Ok);
    assert!(matches!(
        second.request(&Message::ConsoleDetach { vm: id }),
        Message::Error {
            code: ErrorCode::InvalidState,
            ..
        }
    ));
    // Input from a detached client is dropped.
    second.send(b"lost");
    first.send(b"y");
    first.wait_for("xy");
    second.idle(Duration::from_millis(100));
    assert_eq!(String::from_utf8_lossy(&second.output), "early hi\r\nx");
    let third = Console::attach(&server, VmRef::Id(id));
    assert_eq!(third.scrollback, b"early hi\r\nxy");

    // EOT halts the guest.
    first.send(&[4]);
    first.wait_for_event(Message::VmStateChanged {
        vm: id,
        state: VmState::Exited(0),
    });
    assert_eq!(first.output, b"early hi\r\nxy");
    assert!(!String::from_utf8_lossy(&first.output).contains("stranger"));
    server.stop().unwrap();
}

#[test]
fn console_at_full_speed() {
    echo_session(None);
}

#[test]
fn console_in_real_time() {
    echo_session(Some(1_000_000));
}

#[test]
fn a_full_speed_vm_waiting_for_input_sleeps() {
    let server = server(None);
    let mut rt = connect(&server);
    let id = create(&mut rt, "echo", profile(1, None), example("uart-echo"));
    let mut console = Console::attach(&server, VmRef::Id(id));
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    console.send(b"a");
    console.wait_for("a");
    // Parked in wfi with only the UART interrupt enabled, it runs one slice every
    // 50 ms rather than spinning (which is tens of millions of steps a second).
    let before = info(&mut rt, id).steps;
    let slices_before = slices(&server, id);
    std::thread::sleep(Duration::from_millis(300));
    let idle_steps = info(&mut rt, id).steps - before;
    let idle_slices = slices(&server, id) - slices_before;
    assert!(idle_steps < 5_000_000, "{idle_steps} steps while idle");
    assert!(idle_slices < 100, "{idle_slices} slices while idle");
    // And input still wakes it at once.
    let start = Instant::now();
    console.send(b"b");
    console.wait_for("ab");
    assert!(start.elapsed() < Duration::from_secs(2));
    server.stop().unwrap();
}

#[test]
fn a_real_time_vm_keeps_to_its_clock() {
    let server = server(None);
    let mut rt = connect(&server);
    // 1 MHz, waiting in wfi: idle ticks still count as steps, at the clock's rate.
    let id = create(
        &mut rt,
        "clock",
        profile(1, Some(1_000_000)),
        example("uart-echo"),
    );
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    let start = Instant::now();
    std::thread::sleep(Duration::from_millis(300));
    let steps = info(&mut rt, id).steps;
    let slices = slices(&server, id);
    let elapsed = start.elapsed().as_secs_f64();
    // Never ahead of the clock (give or take a 1 ms batch); behind only on a slow host.
    assert!(
        steps as f64 <= (elapsed + 0.002) * 1_000_000.0,
        "{steps} steps in {elapsed} s"
    );
    assert!(steps > 50_000, "{steps} steps in {elapsed} s");
    // And the thread sleeps between its ~1 ms batches instead of asking the machine
    // for more steps over and over (millions of empty slices a second).
    assert!(slices < 3_000, "{slices} slices in {elapsed} s");
    server.stop().unwrap();
}

#[test]
fn a_guest_that_presents_frames_runs_at_full_speed() {
    let server = server(None);
    let mut rt = connect(&server);
    // Presents a frame once, then prints `P` and counts forever.
    let presenter = guest(
        "
    .globl _start
_start:
    li    t0, 0x30000014
    sw    zero, 0(t0)
    li    t0, 0x10000000
    li    t1, 80
    sb    t1, 0(t0)
1:  addi  t2, t2, 1
    j     1b
",
    );
    let id = create(&mut rt, "presenter", profile(4, None), presenter);
    let mut console = Console::attach(&server, VmRef::Id(id));
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    console.wait_for("P");
    let (steps_before, slices_before) = (info(&mut rt, id).steps, slices(&server, id));
    std::thread::sleep(Duration::from_millis(300));
    let steps = info(&mut rt, id).steps - steps_before;
    let slices = slices(&server, id) - slices_before;
    // The frame is taken, so slices are full ones again rather than a step each.
    assert!(slices > 0);
    assert!(steps / slices > 10_000, "{steps} steps in {slices} slices");

    // examples/colorbars draws, presents once and halts.
    let bars = create(&mut rt, "colorbars", profile(4, None), example("colorbars"));
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(bars) }).unwrap();
    assert_eq!(
        wait_for_state(&mut rt, bars, |s| s != VmState::Running),
        VmState::Exited(0)
    );
    server.stop().unwrap();
}

#[test]
fn shutdown_stops_vms_and_closes_connections() {
    let server = server(None);
    let addr = server.addr();
    let mut rt = connect(&server);
    let id = create(&mut rt, "spin", profile(1, None), spinner());
    let idle = create(&mut rt, "idle", profile(1, None), spinner());
    let mut console = Console::attach(&server, VmRef::Id(id));
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    console.wait_for("L");
    // A connection that never says Hello does not hold shutdown up.
    let silent = std::net::TcpStream::connect(addr).unwrap();

    let start = Instant::now();
    server.stop().unwrap();
    assert!(start.elapsed() < SLOW);
    console.wait_closed();
    assert_eq!(
        console.events.last(),
        Some(&Message::VmStateChanged {
            vm: id,
            state: VmState::Stopped
        })
    );
    // The control connection is closed too.
    assert!(rt.request(&Message::GetVmInfo { vm: VmRef::Id(idle) }).is_err());
    drop(silent);
    // Nothing listens any more.
    assert!(avalon_proto::Connection::connect(addr, "late").is_err());
}

#[test]
fn many_clients_at_once() {
    let server = server(None);
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let mut conn = connect(&server);
            std::thread::spawn(move || {
                let name = format!("vm-{i}");
                let id = create(&mut conn, &name, profile(1 + i as u64, None), exits_with(i));
                conn.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
                assert_eq!(
                    wait_for_state(&mut conn, id, |s| matches!(s, VmState::Exited(_))),
                    VmState::Exited(i)
                );
                for token in 0..100 {
                    assert_eq!(
                        conn.request(&Message::Ping { token }).unwrap(),
                        Message::Pong { token }
                    );
                }
                id
            })
        })
        .collect();
    let mut ids: Vec<u32> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    ids.sort();
    assert_eq!(ids, (1..=8).collect::<Vec<_>>());
    let mut rt = connect(&server);
    match rt.request(&Message::ListVms).unwrap() {
        Message::VmList { vms } => {
            assert_eq!(vms.len(), 8);
            assert!(vms.windows(2).all(|w| w[0].id < w[1].id), "in id order");
        }
        other => panic!("{other:?}"),
    }
    server.stop().unwrap();
}

/// The README's client example is the test below, but for the server's address, where
/// the program is read from, and the live output the README leaves in a comment: each
/// of its statements must be there, word for word (spaces and commas aside, so
/// rustfmt's line breaks do not count).
#[test]
fn the_readme_client_example_is_the_test() {
    // A Windows checkout may have CRLF line endings.
    let readme = include_str!("../../../README.md").replace("\r\n", "\n");
    let section = &readme[readme.find("## The Avalon platform").unwrap()..];
    let start = section.find("```rust\n").unwrap() + "```rust\n".len();
    let block = &section[start..start + section[start..].find("```").unwrap()];
    let squash = |s: &str| {
        s.chars()
            .filter(|c| !c.is_whitespace() && *c != ',')
            .collect::<String>()
    };
    let test = squash(include_str!("server.rs"));
    let mut checked = 0;
    for line in block.lines() {
        if line.is_empty() || line.starts_with("use ") || line.starts_with("print!") {
            continue;
        }
        let line = line.replace("\"127.0.0.1:7400\"", "addr.as_str()").replace(
            "std::fs::read(\"examples/sysinfo/sysinfo.elf\")",
            "std::fs::read(examples_dir().join(\"sysinfo/sysinfo.elf\"))",
        );
        assert!(test.contains(&squash(&line)), "not in the test: {line}");
        checked += 1;
    }
    assert_eq!(checked, 6);
}

/// The client example in the README, against an in-process server.
#[test]
fn the_readme_client_example() -> Result<(), Box<dyn std::error::Error>> {
    use avalon_proto::{Connection, HardwareProfile};
    let server = server(None);
    let addr = server.addr().to_string();

    let mut avalon = Connection::connect(addr.as_str(), "example")?;
    let program = std::fs::read(examples_dir().join("sysinfo/sysinfo.elf"))?;
    let profile = HardwareProfile {
        clock_hz: Some(2_000_000),
        ..HardwareProfile::with_ram(64 << 20)
    };
    let Message::VmCreated { id } = avalon.request(&Message::CreateVm {
        name: "demo".into(),
        profile,
        program,
    })?
    else {
        panic!()
    };
    avalon.request_ok(&Message::StartVm { vm: VmRef::Id(id) })?;
    let Message::ConsoleAttached { scrollback, .. } =
        avalon.request(&Message::ConsoleAttach { vm: VmRef::Id(id) })?
    else {
        panic!()
    };

    // Then the rest of the output, live.
    let mut output = scrollback;
    let expected = sysinfo_output(64, 2_000_000, true);
    avalon.get_ref().set_read_timeout(Some(SLOW))?;
    while output.len() < expected.len() {
        match avalon.recv()? {
            Message::ConsoleOutput { data, .. } => output.extend(data),
            Message::VmStateChanged { .. } => {}
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(String::from_utf8(output)?, expected);
    server.stop()?;
    Ok(())
}

/// A VM created with no program boots from its disk 0 (spec section 7, "Boot from
/// disk"): here a boot block that says `bye` and halts with exit code 3. A VM whose
/// disk has no boot block stops with the boot ROM's message and exit code.
#[test]
fn a_vm_with_no_program_boots_from_its_disk() {
    let dir = std::env::temp_dir().join(format!("avalon-disk-boot-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = grail::elf::parse(&exits_with(3)).unwrap();
    let mut block = [0u8; grail::boot::BOOT_BLOCK_SIZE];
    block[..file.segments[0].data.len()].copy_from_slice(&file.segments[0].data);
    grail::boot::seal(&mut block);
    let bootable = dir.join("bootable.img");
    std::fs::write(&bootable, [&block[..], &[0; 3072]].concat()).unwrap();
    let empty = dir.join("empty.img");
    std::fs::write(&empty, [0u8; 4096]).unwrap();

    let server = server(None);
    let mut rt = connect(&server);
    let disk = |path: &std::path::Path| HardwareProfile {
        devices: vec![DeviceSpec::Disk(DiskSpec {
            source: DiskSource::File(path.to_string_lossy().into_owned()),
            read_only: true,
        })],
        ..profile(4, None)
    };
    for (path, code, out) in [
        (&bootable, 3, "bye".to_string()),
        (
            &empty,
            grail::boot::EXIT_BOOT_FAILED,
            "boot ROM: disk 0 is not bootable (no boot signature)\n".to_string(),
        ),
    ] {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let vm = create(&mut rt, &name, disk(path), Vec::new());
        let mut console = Console::attach(&server, VmRef::Id(vm));
        rt.request_ok(&Message::StartVm { vm: VmRef::Id(vm) }).unwrap();
        console.wait_for_event(Message::VmStateChanged {
            vm,
            state: VmState::Exited(code),
        });
        assert_eq!(String::from_utf8_lossy(&console.output), out);
    }
    // No program and no disk: nothing to run.
    assert_eq!(
        error_of(
            &mut rt,
            &Message::CreateVm {
                name: "nothing".into(),
                profile: profile(4, None),
                program: Vec::new(),
            }
        ),
        ErrorCode::InvalidProgram
    );
    server.stop().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
