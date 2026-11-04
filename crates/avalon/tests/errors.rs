//! Bad requests get protocol errors, and nothing a client sends brings the server down:
//! unknown VMs, invalid names, profiles and programs, the RAM budget, requests for
//! groups Avalon does not serve, and malformed or misdirected frames.

mod common;

use avalon::Config;
use avalon_proto::{
    encode, ty, DeviceSpec, DiskSource, DiskSpec, ErrorCode, Frame, HardwareProfile, Message, VmRef, VmState,
};
use common::*;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
            Message::ScreenAttach { vm: vm.clone() },
        ] {
            assert_eq!(error_of(&mut rt, &msg), ErrorCode::NoSuchVm, "{msg:?}");
        }
    }
    for msg in [Message::ConsoleDetach { vm: 9 }, Message::ScreenDetach { vm: 9 }] {
        assert_eq!(error_of(&mut rt, &msg), ErrorCode::NoSuchVm, "{msg:?}");
    }
    // Attached nowhere.
    for msg in [Message::ConsoleDetach { vm: 1 }, Message::ScreenDetach { vm: 1 }] {
        assert_eq!(error_of(&mut rt, &msg), ErrorCode::InvalidState, "{msg:?}");
    }
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
    // A port nobody listens on: a Lake disk there cannot be attached.
    let no_lake = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().to_string()
    };
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
                addr: no_lake.clone(),
                name: "root".into(),
            })],
            ..HardwareProfile::with_ram(MIB)
        },
        HardwareProfile {
            devices: vec![disk(DiskSource::Lake {
                addr: no_lake,
                name: "not/a/name".into(),
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
            source: None,
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
            repeat: false,
            usage: 0x04,
        },
    })
    .unwrap();
    expect_reply(&mut conn, key, None);
    let pointer = Frame::from_message(&Message::PointerInput {
        vm: 1,
        pointer: avalon_proto::PointerChange::default(),
    })
    .unwrap();
    expect_reply(&mut conn, pointer, None);
    expect_reply(&mut conn, frame(ty::POINTER_INPUT, &[1, 0, 0, 0, 7]), None);
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
    // Everything the server sends before it closes the connection, which ends cleanly
    // even when the server did not read all the client sent.
    let read_all = |mut s: TcpStream| read_until_closed(&mut s, SLOW);

    // A stray HTTP client: "GET " reads as a 540 MB frame, refused unread.
    let mut http = TcpStream::connect(server.addr()).unwrap();
    http.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    assert!(read_all(http).is_empty());

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

/// The server's last frame before it closes a connection arrives whole, though the
/// client had sent more that the server never read. Closing a socket with input unread
/// resets the connection, and a reset can throw away what was sent just before it;
/// the server closes gracefully instead (`avalon_proto::close_gracefully`).
#[test]
fn the_last_frame_survives_unread_input() {
    let server = server(None);
    let hello = |version| {
        encode(&Message::Hello {
            version,
            agent: "pipelined".into(),
        })
        .unwrap()
    };
    let ours = avalon_proto::PROTOCOL_VERSION;
    let request = |msg: Message| encode(&msg).unwrap();
    // A frame as the cases below name it: its message, and an Error's code.
    let name = |m: &Message| match m {
        Message::Error { code, .. } => format!("Error {code:?}"),
        other => other.name().to_string(),
    };
    // What the client sends, all at once; then the frames it must get before the close.
    let cases = [
        (
            "another version, then a request",
            [hello(99), request(Message::ListVms)].concat(),
            &["Welcome"][..],
        ),
        (
            "a request before Hello, then another",
            [request(Message::ListVms), request(Message::Ping { token: 1 })].concat(),
            &["Error Unexpected"][..],
        ),
        (
            "an oversize frame in a session, with some of its body",
            [
                hello(ours),
                (64u32 << 20).to_le_bytes().to_vec(),
                vec![0xee; 4096],
            ]
            .concat(),
            &["Welcome", "Error Malformed"][..],
        ),
    ];
    for (what, input, expected) in cases {
        let finished = server.debug_connections_finished();
        let mut s = TcpStream::connect(server.addr()).unwrap();
        s.write_all(&input).unwrap();
        // Done sending: a server that lingers sees the end at once.
        s.shutdown(std::net::Shutdown::Write).unwrap();
        // Nothing is read until the server has closed the socket (the count of closed
        // connections grows only then, while the count of open ones is also 0 before
        // the server accepts), so that a reset, if it sent one, is on its way before
        // the frames are read.
        wait_until("the connection to be closed", SLOW, || {
            server.debug_connections_finished() > finished
        });
        let reply = read_until_closed(&mut s, SLOW);
        let mut got = Vec::new();
        let mut rest = &reply[..];
        while !rest.is_empty() {
            let (msg, n) = avalon_proto::decode(rest).unwrap_or_else(|e| panic!("{what}: {e}"));
            got.push(name(&msg));
            rest = &rest[n..];
        }
        assert_eq!(got, expected, "{what}");
    }
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
    for _ in 0..2 {
        drop(Viewer::attach(&server, VmRef::Id(id)));
    }
    // Detached as they left, not only once the VM next writes to them.
    wait_until("the clients to be detached", SLOW, || {
        let vm = server.debug_vm(id).unwrap();
        vm.consoles == 0 && vm.viewers == 0
    });
    let mut last = Console::attach(&server, VmRef::Id(id));
    assert_eq!(last.scrollback, b"xxx");
    last.send(b"y");
    last.wait_for("y");
    server.stop().unwrap();
}

/// Reads what the server sends on `s` until it closes the connection; panics if it
/// is still open after `limit`, or if it ends with an error (a reset) instead of a
/// clean close, since a reset can discard what the server sent last. Returns the bytes
/// read.
fn read_until_closed(s: &mut TcpStream, limit: Duration) -> Vec<u8> {
    let start = Instant::now();
    let mut got = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let left = limit.saturating_sub(start.elapsed());
        assert!(!left.is_zero(), "the server did not close the connection");
        s.set_read_timeout(Some(left)).unwrap();
        match s.read(&mut buf) {
            Ok(0) => return got,
            Ok(n) => got.extend_from_slice(&buf[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(e) => panic!(
                "the connection ended with an error, not a clean close ({e}), after {} bytes",
                got.len()
            ),
        }
    }
}

#[test]
fn a_client_gets_a_limited_time_to_say_hello() {
    let timeout = Duration::from_millis(300);
    let server = server_with(Config {
        handshake_timeout: timeout,
        ..quiet()
    });

    // Silent: closed once the time is up, with nothing said.
    let start = Instant::now();
    let mut silent = TcpStream::connect(server.addr()).unwrap();
    assert!(read_until_closed(&mut silent, SLOW).is_empty());
    assert!(start.elapsed() >= timeout - Duration::from_millis(50));

    // Trickling a long Hello in byte by byte, each byte well within the time: still
    // closed once the time is up, long before the Hello is complete.
    let hello = encode(&Message::Hello {
        version: avalon_proto::PROTOCOL_VERSION,
        agent: "a".repeat(300),
    })
    .unwrap();
    let mut slow = TcpStream::connect(server.addr()).unwrap();
    slow.set_nodelay(true).unwrap();
    let mut writer = slow.try_clone().unwrap();
    let trickle = std::thread::spawn(move || {
        for &b in &hello {
            if writer.write_all(&[b]).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    let start = Instant::now();
    assert!(read_until_closed(&mut slow, SLOW).is_empty(), "no Welcome");
    assert!(start.elapsed() < Duration::from_secs(3), "{:?}", start.elapsed());
    trickle.join().unwrap();
    wait_until("the connections to end", SLOW, || server.debug_connections() == 0);

    // Once the session is open, the time limit is gone.
    let mut conn = connect(&server);
    std::thread::sleep(timeout * 2);
    assert_eq!(
        conn.request(&Message::Ping { token: 5 }).unwrap(),
        Message::Pong { token: 5 }
    );
    server.stop().unwrap();
}

/// A client that attaches to a chatty console and stops reading is cut off once
/// [`avalon::MAX_QUEUED`] bytes wait for it; the VM and the other clients carry on.
#[test]
fn a_client_that_stops_reading_is_cut_off() {
    let server = server(None);
    let mut rt = connect(&server);
    // Prints `x` forever, a few instructions apart.
    let chatty = guest(
        "
    .globl _start
_start:
    li    t0, 0x10000000
    li    t1, 120
1:  sb    t1, 0(t0)
    li    t2, 4
2:  addi  t2, t2, -1
    bnez  t2, 2b
    j     1b
",
    );
    let id = create(&mut rt, "chatty", profile(1, None), chatty);

    // A client that reads everything, counting the console's bytes.
    let received = Arc::new(AtomicU64::new(0));
    let mut reader = connect(&server);
    reader.get_ref().set_read_timeout(None).unwrap();
    reader
        .request(&Message::ConsoleAttach { vm: VmRef::Id(id) })
        .unwrap();
    let reading = {
        let received = Arc::clone(&received);
        std::thread::spawn(move || {
            while let Ok(msg) = reader.recv() {
                if let Message::ConsoleOutput { data, .. } = msg {
                    received.fetch_add(data.len() as u64, Ordering::SeqCst);
                }
            }
        })
    };
    // And one that attaches, then reads nothing more.
    let mut stuck = connect(&server);
    stuck
        .request(&Message::ConsoleAttach { vm: VmRef::Id(id) })
        .unwrap();
    assert_eq!(server.debug_connections(), 3);
    assert_eq!(server.debug_vm(id).unwrap().consoles, 2);

    rt.request_ok(&Message::StartVm { vm: VmRef::Id(id) }).unwrap();
    let limit = Duration::from_secs(60);
    wait_until("the stuck client to be cut off", limit, || {
        server.debug_vm(id).unwrap().consoles == 1
    });
    // Its connection ends: the server's threads for it are gone, and what reaches it is
    // what was in flight, then the end.
    wait_until("its connection to end", SLOW, || server.debug_connections() == 2);
    let rest = read_until_closed(stuck.get_mut(), SLOW);
    assert!(rest.len() < 2 * avalon::MAX_QUEUED, "{} bytes", rest.len());

    // The VM runs on, the reading client still gets its output, requests are answered.
    let at = received.load(Ordering::SeqCst);
    let steps = info(&mut rt, id).steps;
    wait_until("more output", SLOW, || {
        received.load(Ordering::SeqCst) > at + 100_000
    });
    assert!(info(&mut rt, id).steps > steps);
    assert_eq!(state(&mut rt, id), VmState::Running);
    assert_eq!(
        rt.request(&Message::Ping { token: 8 }).unwrap(),
        Message::Pong { token: 8 }
    );
    server.stop().unwrap();
    reading.join().unwrap();
}

/// A create whose disk image takes a long time to open holds up no other request:
/// here a FIFO, whose open blocks until someone opens it for writing.
#[cfg(target_os = "linux")]
#[test]
fn a_slow_create_holds_up_no_one_else() {
    use std::os::unix::ffi::OsStrExt;

    extern "C" {
        fn mkfifo(path: *const std::os::raw::c_char, mode: u32) -> i32;
    }
    let dir = std::env::temp_dir().join(format!("avalon-fifo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("disk.img");
    let _ = std::fs::remove_file(&fifo);
    let path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: a NUL-terminated path and a mode.
    assert_eq!(unsafe { mkfifo(path.as_ptr(), 0o600) }, 0);

    let server = server(Some(64 * MIB));
    let profile_with_fifo = HardwareProfile {
        devices: vec![DeviceSpec::Disk(DiskSpec {
            source: DiskSource::File(fifo.to_string_lossy().into_owned()),
            read_only: true,
        })],
        ..HardwareProfile::with_ram(40 * MIB)
    };
    let slow = {
        let mut conn = connect(&server);
        conn.get_ref().set_read_timeout(None).unwrap();
        std::thread::spawn(move || {
            conn.request(&Message::CreateVm {
                name: "slow".into(),
                profile: profile_with_fifo,
                program: spinner(),
            })
        })
    };
    std::thread::sleep(Duration::from_millis(200));
    assert!(!slow.is_finished(), "the open of a FIFO blocks");

    // Everyone else is served meanwhile; the slow VM's name and RAM are taken.
    let mut rt = connect(&server);
    let start = Instant::now();
    assert_eq!(
        rt.request(&Message::ListVms).unwrap(),
        Message::VmList { vms: vec![] }
    );
    assert_eq!(
        create_error(&mut rt, "slow", profile(1, None), spinner()),
        ErrorCode::AlreadyExists
    );
    assert_eq!(
        create_error(&mut rt, "big", profile(30, None), spinner()),
        ErrorCode::OverBudget
    );
    let other = create(&mut rt, "other", profile(20, None), spinner());
    rt.request_ok(&Message::StartVm { vm: VmRef::Id(other) }).unwrap();
    assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
    assert!(!slow.is_finished());

    // A writer lets the open finish. A FIFO reads as an empty image: a disk of no
    // sectors, or one Grail refuses; either way the create ends.
    let writer = std::fs::OpenOptions::new().write(true).open(&fifo).unwrap();
    match slow.join().unwrap() {
        Ok(Message::VmCreated { id }) => {
            assert_ne!(id, other);
            rt.request_ok(&Message::DestroyVm {
                vm: VmRef::Name("slow".into()),
            })
            .unwrap();
        }
        Err(avalon_proto::Error::Remote {
            code: ErrorCode::InvalidProfile,
            ..
        }) => {}
        other => panic!("{other:?}"),
    }
    drop(writer);
    // Its name and RAM are free again.
    create(&mut rt, "slow", profile(40, None), spinner());
    server.stop().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
