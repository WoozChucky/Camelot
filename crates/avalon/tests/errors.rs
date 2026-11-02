//! Bad requests get protocol errors, and nothing a client sends brings the server down:
//! unknown VMs, invalid names, profiles and programs, the RAM budget, requests for
//! groups Avalon does not serve, and malformed or misdirected frames.

mod common;

use avalon_proto::{
    encode, ty, DeviceSpec, DiskSource, DiskSpec, ErrorCode, Frame, HardwareProfile, Message, VmRef,
};
use common::*;
use std::io::{Read, Write};
use std::net::TcpStream;

fn create_error(
    conn: &mut avalon_proto::Connection<TcpStream>,
    name: &str,
    profile: HardwareProfile,
    program: Vec<u8>,
) -> ErrorCode {
    error_of(
        conn,
        &Message::CreateVm {
            name: name.into(),
            profile,
            program,
        },
    )
}

#[test]
fn unknown_vms() {
    let server = server(None);
    let mut rt = connect(&server);
    create(&mut rt, "real", profile(1, None), spinner());
    for vm in [
        VmRef::Id(0),
        VmRef::Id(2),
        VmRef::Id(u32::MAX),
        VmRef::Name("ghost".into()),
    ] {
        for msg in [
            Message::GetVmInfo { vm: vm.clone() },
            Message::StartVm { vm: vm.clone() },
            Message::PauseVm { vm: vm.clone() },
            Message::ResumeVm { vm: vm.clone() },
            Message::StopVm { vm: vm.clone() },
            Message::DestroyVm { vm: vm.clone() },
            Message::ConsoleAttach { vm: vm.clone() },
        ] {
            assert_eq!(error_of(&mut rt, &msg), ErrorCode::NoSuchVm, "{msg:?}");
        }
    }
    assert_eq!(
        error_of(&mut rt, &Message::ConsoleDetach { vm: 9 }),
        ErrorCode::NoSuchVm
    );
    // Attached nowhere.
    assert_eq!(
        error_of(&mut rt, &Message::ConsoleDetach { vm: 1 }),
        ErrorCode::InvalidState
    );
    server.stop().unwrap();
}

#[test]
fn invalid_names() {
    let server = server(None);
    let mut rt = connect(&server);
    for name in [
        "",
        "Upper",
        "123",
        "con",
        "com1.img",
        "a/b",
        "-x",
        ".x",
        "x.",
        "a b",
        &"x".repeat(65),
    ] {
        assert_eq!(
            create_error(&mut rt, name, profile(1, None), spinner()),
            ErrorCode::InvalidName,
            "{name:?}"
        );
    }
    create(&mut rt, &"x".repeat(64), profile(1, None), spinner());
    create(&mut rt, "web-1.prod_a", profile(1, None), spinner());
    assert_eq!(
        create_error(&mut rt, "web-1.prod_a", profile(1, None), spinner()),
        ErrorCode::AlreadyExists
    );
    server.stop().unwrap();
}

#[test]
fn invalid_profiles() {
    let server = server(None);
    let mut rt = connect(&server);
    let disk = |source| {
        DeviceSpec::Disk(DiskSpec {
            source,
            read_only: false,
        })
    };
    let missing = examples_dir().join("disk/no-such-image.img");
    let bad = [
        HardwareProfile::with_ram(0),
        HardwareProfile::with_ram(4095),
        HardwareProfile::with_ram(4 << 30),
        HardwareProfile::with_ram(u64::MAX),
        HardwareProfile {
            clock_hz: Some(0),
            ..HardwareProfile::with_ram(MIB)
        },
        HardwareProfile {
            nominal_hz: 0,
            ..HardwareProfile::with_ram(MIB)
        },
        HardwareProfile {
            devices: vec![disk(DiskSource::File(missing.to_string_lossy().into_owned())); 9],
            ..HardwareProfile::with_ram(MIB)
        },
        HardwareProfile {
            devices: vec![disk(DiskSource::File(missing.to_string_lossy().into_owned()))],
            ..HardwareProfile::with_ram(MIB)
        },
        HardwareProfile {
            devices: vec![disk(DiskSource::Lake {
                addr: "127.0.0.1:7401".into(),
                name: "root".into(),
            })],
            ..HardwareProfile::with_ram(MIB)
        },
    ];
    for profile in bad {
        assert_eq!(
            create_error(&mut rt, "vm", profile.clone(), spinner()),
            ErrorCode::InvalidProfile,
            "{profile:?}"
        );
    }
    assert_eq!(
        rt.request(&Message::ListVms).unwrap(),
        Message::VmList { vms: vec![] }
    );
    server.stop().unwrap();
}

/// Sets the memory size of an ELF's first program header.
fn with_memsz(mut elf: Vec<u8>, memsz: u32) -> Vec<u8> {
    let phoff = u32::from_le_bytes(elf[0x1c..0x20].try_into().unwrap()) as usize;
    elf[phoff + 0x14..phoff + 0x18].copy_from_slice(&memsz.to_le_bytes());
    elf
}

#[test]
fn invalid_programs() {
    let server = server(None);
    let mut rt = connect(&server);
    let mut truncated = spinner();
    truncated.truncate(40);
    let mut not_riscv = spinner();
    not_riscv[0x12] = 0x3e; // e_machine = x86-64
    for program in [
        Vec::new(),
        b"not an ELF file".to_vec(),
        truncated,
        not_riscv,
        // A segment far larger than RAM: refused before anything is allocated for it.
        with_memsz(spinner(), 0xffff_f000),
        with_memsz(spinner(), 2 * MIB as u32),
    ] {
        assert_eq!(
            create_error(&mut rt, "vm", profile(1, None), program),
            ErrorCode::InvalidProgram
        );
    }
    // The same segment fits in a bigger machine.
    create(
        &mut rt,
        "vm",
        profile(4, None),
        with_memsz(spinner(), 2 * MIB as u32),
    );
    server.stop().unwrap();
}

#[test]
fn the_ram_budget() {
    let server = server(Some(100 * MIB));
    let mut rt = connect(&server);
    assert_eq!(
        create_error(&mut rt, "huge", profile(101, None), spinner()),
        ErrorCode::OverBudget
    );
    let a = create(&mut rt, "a", profile(60, None), spinner());
    create(&mut rt, "b", profile(40, None), spinner());
    assert_eq!(
        create_error(&mut rt, "c", profile(1, None), spinner()),
        ErrorCode::OverBudget
    );
    // Stopping keeps the RAM; destroying frees it.
    rt.request_ok(&Message::StopVm { vm: VmRef::Id(a) }).unwrap();
    assert_eq!(
        create_error(&mut rt, "c", profile(1, None), spinner()),
        ErrorCode::OverBudget
    );
    rt.request_ok(&Message::DestroyVm { vm: VmRef::Id(a) }).unwrap();
    create(&mut rt, "c", profile(60, None), spinner());
    server.stop().unwrap();
}

#[test]
fn state_errors() {
    let server = server(None);
    let mut rt = connect(&server);
    let id = create(&mut rt, "vm", profile(1, None), spinner());
    for msg in [
        Message::PauseVm { vm: VmRef::Id(id) },
        Message::ResumeVm { vm: VmRef::Id(id) },
    ] {
        assert_eq!(error_of(&mut rt, &msg), ErrorCode::InvalidState);
    }
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    for msg in [
        Message::StartVm { vm: VmRef::Id(id) },
        Message::ResumeVm { vm: VmRef::Id(id) },
    ] {
        assert_eq!(error_of(&mut rt, &msg), ErrorCode::InvalidState);
    }
    server.stop().unwrap();
}

#[test]
fn groups_avalon_does_not_serve() {
    let server = server(None);
    let mut rt = connect(&server);
    create(&mut rt, "vm", profile(1, None), spinner());
    for msg in [
        Message::ScreenAttach { vm: VmRef::Id(1) },
        Message::ScreenDetach { vm: 1 },
        Message::BlockOpen {
            name: "disk".into(),
            read_only: true,
        },
        Message::BlockRead {
            handle: 1,
            sector: 0,
            count: 1,
        },
        Message::BlockWrite {
            handle: 1,
            sector: 0,
            data: vec![0; 512],
        },
        Message::BlockFlush { handle: 1 },
        Message::BlockSize { handle: 1 },
        Message::BlockClose { handle: 1 },
        Message::DiskCreate {
            name: "disk".into(),
            sectors: 8,
        },
        Message::ListDisks,
        Message::DiskDelete { name: "disk".into() },
    ] {
        assert_eq!(error_of(&mut rt, &msg), ErrorCode::Unsupported, "{msg:?}");
    }
    assert_eq!(
        error_of(
            &mut rt,
            &Message::Hello {
                version: 1,
                agent: "again".into()
            }
        ),
        ErrorCode::Unexpected
    );
    server.stop().unwrap();
}

/// Raw frames, each answered (or not) as section 4 of the protocol doc says: the next
/// `Pong` must be the very next reply, so nothing was answered twice or skipped.
#[test]
fn misdirected_and_malformed_frames() {
    let server = server(None);
    let mut conn = connect(&server);
    let expect_reply =
        |conn: &mut avalon_proto::Connection<TcpStream>, frame: Frame, code: Option<ErrorCode>| {
            conn.send_frame(&frame).unwrap();
            conn.send(&Message::Ping { token: 77 }).unwrap();
            if let Some(code) = code {
                match conn.recv().unwrap() {
                    Message::Error { code: got, .. } => assert_eq!(got, code, "{frame:?}"),
                    other => panic!("{frame:?}: {other:?}"),
                }
            }
            assert_eq!(conn.recv().unwrap(), Message::Pong { token: 77 }, "{frame:?}");
        };
    let frame = |ty: u16, payload: &[u8]| Frame {
        ty,
        payload: payload.to_vec(),
    };
    // Unknown type.
    expect_reply(&mut conn, frame(0x7777, b"??"), Some(ErrorCode::UnknownMessage));
    // Replies are not for clients to send, valid or not.
    expect_reply(&mut conn, frame(ty::OK, &[]), Some(ErrorCode::Unexpected));
    expect_reply(
        &mut conn,
        frame(ty::VM_CREATED, &[1]),
        Some(ErrorCode::Unexpected),
    );
    // Requests that do not decode.
    expect_reply(
        &mut conn,
        frame(ty::START_VM, &[0, 1]),
        Some(ErrorCode::Malformed),
    );
    expect_reply(
        &mut conn,
        frame(ty::START_VM, &[9, 1, 0, 0, 0]),
        Some(ErrorCode::Malformed),
    );
    expect_reply(&mut conn, frame(ty::LIST_VMS, &[0]), Some(ErrorCode::Malformed));
    expect_reply(
        &mut conn,
        frame(ty::CREATE_VM, &[0xff; 30]),
        Some(ErrorCode::Malformed),
    );
    // Events are never answered: bad ones, ones that only servers send, input for
    // VMs that do not exist.
    expect_reply(&mut conn, frame(ty::CONSOLE_INPUT, &[1]), None);
    let output = Message::ConsoleOutput {
        vm: 1,
        data: b"x".to_vec(),
    };
    let encoded = Frame::from_message(&output).unwrap();
    expect_reply(&mut conn, encoded, None);
    let input = Frame::from_message(&Message::ConsoleInput {
        vm: 42,
        data: b"x".to_vec(),
    })
    .unwrap();
    expect_reply(&mut conn, input, None);
    let key = Frame::from_message(&Message::KeyInput {
        vm: 1,
        key: avalon_proto::KeyEvent {
            code: b'a',
            release: false,
            shift: false,
            ctrl: false,
        },
    })
    .unwrap();
    expect_reply(&mut conn, key, None);
    // A frame too short to have a type is consumed and answered.
    conn.get_mut().write_all(&[1, 0, 0, 0, 0xee]).unwrap();
    match conn.recv().unwrap() {
        Message::Error { code, .. } => assert_eq!(code, ErrorCode::Malformed),
        other => panic!("{other:?}"),
    }
    expect_reply(&mut conn, frame(ty::CONSOLE_OUTPUT, &[]), None);
    // Still a working session.
    assert_eq!(
        conn.request(&Message::ListVms).unwrap(),
        Message::VmList { vms: vec![] }
    );
    server.stop().unwrap();
}

#[test]
fn an_oversize_frame_ends_the_connection_only() {
    let server = server(None);
    let mut conn = connect(&server);
    conn.get_mut().write_all(&(64u32 << 20).to_le_bytes()).unwrap();
    match conn.recv() {
        Ok(Message::Error { code, .. }) => assert_eq!(code, ErrorCode::Malformed),
        other => panic!("{other:?}"),
    }
    assert!(conn.recv().is_err(), "closed");
    let mut other = connect(&server);
    assert_eq!(
        other.request(&Message::Ping { token: 3 }).unwrap(),
        Message::Pong { token: 3 }
    );
    server.stop().unwrap();
}

#[test]
fn bad_handshakes() {
    let server = server(None);
    // Everything the server sends before it closes the connection. (Closing with
    // unread input may reset the connection instead of ending it cleanly.)
    let read_all = |mut s: TcpStream| {
        s.set_read_timeout(Some(SLOW)).unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 256];
        loop {
            match s.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    panic!("the server did not close the connection")
                }
                Err(_) => break,
            }
        }
        got
    };

    // A stray HTTP client: "GET " reads as a 540 MB frame, refused unread.
    let mut http = TcpStream::connect(server.addr()).unwrap();
    http.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    read_all(http);

    // Another request before Hello: an error, then the connection is closed.
    let mut early = TcpStream::connect(server.addr()).unwrap();
    early.write_all(&encode(&Message::ListVms).unwrap()).unwrap();
    let reply = read_all(early);
    let (msg, _) = avalon_proto::decode(&reply).unwrap();
    assert!(matches!(
        msg,
        Message::Error {
            code: ErrorCode::Unexpected,
            ..
        }
    ));

    // Another version: Welcome with ours, then closed.
    let mut future = TcpStream::connect(server.addr()).unwrap();
    future
        .write_all(
            &encode(&Message::Hello {
                version: 99,
                agent: "future".into(),
            })
            .unwrap(),
        )
        .unwrap();
    let reply = read_all(future);
    let (msg, n) = avalon_proto::decode(&reply).unwrap();
    assert!(matches!(
        msg,
        Message::Welcome {
            version: avalon_proto::PROTOCOL_VERSION,
            ..
        }
    ));
    assert_eq!(n, reply.len());

    // A client that connects and leaves at once.
    drop(TcpStream::connect(server.addr()).unwrap());

    let mut good = connect(&server);
    assert_eq!(
        good.request(&Message::Ping { token: 1 }).unwrap(),
        Message::Pong { token: 1 }
    );
    server.stop().unwrap();
}

#[test]
fn a_client_that_leaves_is_detached() {
    let server = server(None);
    let mut rt = connect(&server);
    let id = create(&mut rt, "echo", profile(1, None), example("uart-echo"));
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    for _ in 0..3 {
        let mut console = Console::attach(&server, VmRef::Id(id));
        console.send(b"x");
        console.wait_for("x");
        drop(console);
    }
    let mut last = Console::attach(&server, VmRef::Id(id));
    assert_eq!(last.scrollback, b"xxx");
    last.send(b"y");
    last.wait_for("y");
    server.stop().unwrap();
}
