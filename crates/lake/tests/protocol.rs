//! An in-process Lake on an ephemeral port, driven through the protocol: disk
//! management (what Round Table's `disk create|list|delete` send), block I/O, the
//! one-writer-or-many-readers rule across connections, and every kind of bad request,
//! each of which must get its error code and leave the server running.

mod common;

use avalon_proto::{encode, ty, DiskInfo, ErrorCode, Frame, HardwareProfile, Message, VmRef};
use common::*;
use lake::{Config, MAX_HANDLES};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

#[test]
fn disk_management() {
    let dir = TempDir::new("manage");
    let server = lake(&dir);
    let mut rt = connect(&server);
    assert_eq!(list(&mut rt), []);
    create_disk(&mut rt, "root", 2048);
    create_disk(&mut rt, "data.img", 8);
    create_disk(&mut rt, "a_1-b", 1);
    let info = |name: &str, sectors| DiskInfo {
        name: name.into(),
        sectors,
        readers: 0,
        writer: false,
    };
    // In name order.
    assert_eq!(
        list(&mut rt),
        [info("a_1-b", 1), info("data.img", 8), info("root", 2048)]
    );
    // Each disk is a zero-filled raw image named after it.
    assert_eq!(
        std::fs::read(dir.path().join("data.img")).unwrap(),
        vec![0; 8 * 512]
    );

    let create = |name: &str, sectors| Message::DiskCreate {
        name: name.into(),
        sectors,
    };
    assert_eq!(error_of(&mut rt, &create("root", 1)), ErrorCode::AlreadyExists);
    for bad in [
        "", "Root", "../root", "a/b", "a\\b", "con", "com1.img", "123", ".hidden", "x.",
    ] {
        assert_eq!(
            error_of(&mut rt, &create(bad, 1)),
            ErrorCode::InvalidName,
            "{bad:?}"
        );
    }
    assert_eq!(error_of(&mut rt, &create("empty", 0)), ErrorCode::OverBudget);
    assert_eq!(
        error_of(&mut rt, &create("huge", u32::MAX as u64 + 1)),
        ErrorCode::OverBudget
    );
    assert_eq!(
        error_of(&mut rt, &create("huge", u64::MAX)),
        ErrorCode::OverBudget
    );
    assert_eq!(list(&mut rt).len(), 3, "nothing else was created");

    rt.request_ok(&Message::DiskDelete { name: "root".into() })
        .unwrap();
    assert!(!dir.path().join("root").exists());
    let delete = |name: &str| Message::DiskDelete { name: name.into() };
    assert_eq!(error_of(&mut rt, &delete("root")), ErrorCode::NoSuchDisk);
    assert_eq!(error_of(&mut rt, &delete("../x")), ErrorCode::InvalidName);
    assert_eq!(list(&mut rt), [info("a_1-b", 1), info("data.img", 8)]);
    // A name is free again once its disk is deleted.
    create_disk(&mut rt, "root", 4);
    assert_eq!(disk_info(&mut rt, "root").sectors, 4);
    server.stop().unwrap();

    // The images outlive the server: a new one serves the same directory.
    let server = lake(&dir);
    let mut rt = connect(&server);
    assert_eq!(list(&mut rt).len(), 3);
    server.stop().unwrap();
}

#[test]
fn a_size_limit_of_its_own() {
    let dir = TempDir::new("limit");
    let server = lake_with(Config {
        max_sectors: 16,
        ..quiet(&dir)
    });
    let mut rt = connect(&server);
    create_disk(&mut rt, "max", 16);
    let code = error_of(
        &mut rt,
        &Message::DiskCreate {
            name: "over".into(),
            sectors: 17,
        },
    );
    assert_eq!(code, ErrorCode::OverBudget);
    server.stop().unwrap();
}

#[test]
fn block_io() {
    let dir = TempDir::new("io");
    let server = lake(&dir);
    let mut c = connect(&server);
    create_disk(&mut c, "d", 4096);
    let (h, sectors) = open(&mut c, "d", false);
    assert_eq!(sectors, 4096);
    assert_eq!(
        c.request(&Message::BlockSize { handle: h }).unwrap(),
        Message::BlockCapacity {
            handle: h,
            sectors: 4096
        }
    );

    // The largest request each way: 2048 sectors, 1 MiB.
    let big: Vec<u8> = (0..2048 * 512u32).map(|i| (i * 7 + i / 512) as u8).collect();
    write(&mut c, h, 1000, &big);
    assert_eq!(read(&mut c, h, 1000, 2048), big);
    assert_eq!(read(&mut c, h, 999, 1), vec![0; 512], "before it");
    assert_eq!(read(&mut c, h, 3048, 1), vec![0; 512], "after it");
    write(&mut c, h, 4095, &[0xab; 512]);
    c.request_ok(&Message::BlockFlush { handle: h }).unwrap();
    // It is in the image file, at sector * 512.
    let file = std::fs::read(dir.path().join("d")).unwrap();
    assert_eq!(file.len(), 4096 * 512);
    assert_eq!(file[1000 * 512..3048 * 512], big[..]);
    assert_eq!(file[4095 * 512..], [0xab; 512]);

    // Out of range: an error, nothing moves, nothing wraps around.
    for (sector, count) in [
        (4096, 1),
        (4095, 2),
        (2049, 2048),
        (u64::MAX, 1),
        (u64::MAX - 10, 2048),
    ] {
        let code = error_of(
            &mut c,
            &Message::BlockRead {
                handle: h,
                sector,
                count,
            },
        );
        assert_eq!(code, ErrorCode::OutOfRange, "read {sector} {count}");
        let data = vec![0xff; count as usize * 512];
        let code = error_of(
            &mut c,
            &Message::BlockWrite {
                handle: h,
                sector,
                data,
            },
        );
        assert_eq!(code, ErrorCode::OutOfRange, "write {sector} {count}");
    }
    assert_eq!(std::fs::read(dir.path().join("d")).unwrap(), file);

    // Unknown handles.
    for handle in [0, h + 1, u32::MAX] {
        for msg in [
            Message::BlockRead {
                handle,
                sector: 0,
                count: 1,
            },
            Message::BlockWrite {
                handle,
                sector: 0,
                data: vec![0; 512],
            },
            Message::BlockFlush { handle },
            Message::BlockSize { handle },
            Message::BlockClose { handle },
        ] {
            assert_eq!(error_of(&mut c, &msg), ErrorCode::NoSuchHandle, "{msg:?}");
        }
    }

    // Closed handles are gone, and never come back as another attachment.
    c.request_ok(&Message::BlockClose { handle: h }).unwrap();
    assert_eq!(
        error_of(&mut c, &Message::BlockSize { handle: h }),
        ErrorCode::NoSuchHandle
    );
    let (h2, _) = open(&mut c, "d", true);
    assert_ne!(h2, h);
    assert_eq!(
        error_of(&mut c, &Message::BlockSize { handle: h }),
        ErrorCode::NoSuchHandle
    );
    // A read-only attachment reads, flushes, and refuses writes.
    assert_eq!(read(&mut c, h2, 1000, 2048), big);
    c.request_ok(&Message::BlockFlush { handle: h2 }).unwrap();
    let code = error_of(
        &mut c,
        &Message::BlockWrite {
            handle: h2,
            sector: 0,
            data: vec![1; 512],
        },
    );
    assert_eq!(code, ErrorCode::ReadOnly);
    assert_eq!(read(&mut c, h2, 0, 1), vec![0; 512]);

    let open_msg = |name: &str| Message::BlockOpen {
        name: name.into(),
        read_only: true,
    };
    assert_eq!(error_of(&mut c, &open_msg("nope")), ErrorCode::NoSuchDisk);
    assert_eq!(error_of(&mut c, &open_msg("../d")), ErrorCode::InvalidName);
    server.stop().unwrap();
}

#[test]
fn one_writer_or_many_readers_across_connections() {
    let dir = TempDir::new("rule");
    let server = lake(&dir);
    let mut rt = connect(&server);
    create_disk(&mut rt, "img", 8);
    let open_msg = |read_only| Message::BlockOpen {
        name: "img".into(),
        read_only,
    };

    let mut writer = connect(&server);
    let (w, _) = open(&mut writer, "img", false);
    let info = disk_info(&mut rt, "img");
    assert!(info.writer && info.readers == 0);
    // A second writer, or a reader while there is a writer: Busy, on any connection
    // (the writer's own included).
    for conn in [&mut rt, &mut writer] {
        assert_eq!(error_of(conn, &open_msg(false)), ErrorCode::Busy);
        assert_eq!(error_of(conn, &open_msg(true)), ErrorCode::Busy);
    }
    // An attached disk cannot be deleted.
    assert_eq!(
        error_of(&mut rt, &Message::DiskDelete { name: "img".into() }),
        ErrorCode::Busy
    );
    write(&mut writer, w, 3, &[7; 512]);

    // Closing the handle frees it at once.
    writer.request_ok(&Message::BlockClose { handle: w }).unwrap();
    assert!(!disk_info(&mut rt, "img").writer);

    // Readers share it, and keep writers out.
    let mut r1 = connect(&server);
    let mut r2 = connect(&server);
    let (h1, _) = open(&mut r1, "img", true);
    let (h2, _) = open(&mut r2, "img", true);
    let (h3, _) = open(&mut r2, "img", true);
    assert_eq!(disk_info(&mut rt, "img").readers, 3);
    assert_eq!(error_of(&mut writer, &open_msg(false)), ErrorCode::Busy);
    assert_eq!(read(&mut r1, h1, 3, 1), vec![7; 512]);
    assert_eq!(read(&mut r2, h2, 3, 1), vec![7; 512]);
    assert_eq!(read(&mut r2, h3, 3, 1), vec![7; 512]);
    r2.request_ok(&Message::BlockClose { handle: h2 }).unwrap();
    assert_eq!(disk_info(&mut rt, "img").readers, 2);

    // Ending a connection detaches everything it had attached.
    drop(r2);
    wait_until("r2's reader to be detached", || {
        disk_info(&mut rt, "img").readers == 1
    });
    drop(r1);
    wait_until("every reader to be detached", || {
        disk_info(&mut rt, "img").readers == 0
    });
    let (w, _) = open(&mut writer, "img", false);
    drop(writer);
    wait_until("the writer to be detached", || !disk_info(&mut rt, "img").writer);
    let _ = w;
    rt.request_ok(&Message::DiskDelete { name: "img".into() })
        .unwrap();
    server.stop().unwrap();
}

#[test]
fn attachments_per_connection_are_limited() {
    let dir = TempDir::new("handles");
    let server = lake(&dir);
    let mut c = connect(&server);
    create_disk(&mut c, "shared", 1);
    let handles: Vec<u32> = (0..MAX_HANDLES).map(|_| open(&mut c, "shared", true).0).collect();
    let code = error_of(
        &mut c,
        &Message::BlockOpen {
            name: "shared".into(),
            read_only: true,
        },
    );
    assert_eq!(code, ErrorCode::OverBudget);
    // Another connection is not affected, and closing one makes room.
    let mut other = connect(&server);
    open(&mut other, "shared", true);
    c.request_ok(&Message::BlockClose { handle: handles[5] }).unwrap();
    open(&mut c, "shared", true);
    assert_eq!(disk_info(&mut c, "shared").readers, MAX_HANDLES as u32 + 1);
    server.stop().unwrap();
}

#[test]
fn requests_lake_does_not_serve() {
    let dir = TempDir::new("unserved");
    let server = lake(&dir);
    let mut c = connect(&server);
    for msg in [
        Message::CreateVm {
            name: "vm".into(),
            profile: HardwareProfile::default(),
            program: vec![],
        },
        Message::ListVms,
        Message::StartVm { vm: VmRef::Id(1) },
        Message::ConsoleAttach { vm: VmRef::Id(1) },
        Message::ScreenAttach { vm: VmRef::Id(1) },
        Message::ScreenDetach { vm: 1 },
    ] {
        assert_eq!(error_of(&mut c, &msg), ErrorCode::Unsupported, "{msg:?}");
    }
    assert_eq!(
        error_of(
            &mut c,
            &Message::Hello {
                version: 1,
                agent: "again".into()
            }
        ),
        ErrorCode::Unexpected
    );
    assert_eq!(
        c.request(&Message::Ping { token: 9 }).unwrap(),
        Message::Pong { token: 9 }
    );
    server.stop().unwrap();
}

/// Raw frames, each answered (or not) as section 4 of the protocol doc says: the next
/// `Pong` must be the very next reply, so nothing was answered twice or skipped.
#[test]
fn misdirected_and_malformed_frames() {
    let dir = TempDir::new("frames");
    let server = lake(&dir);
    let mut conn = connect(&server);
    create_disk(&mut conn, "d", 4);
    let (h, _) = open(&mut conn, "d", false);
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
    let block_read = |count: u32| {
        let mut p = h.to_le_bytes().to_vec();
        p.extend(0u64.to_le_bytes());
        p.extend(count.to_le_bytes());
        frame(ty::BLOCK_READ, &p)
    };
    expect_reply(&mut conn, frame(0x7777, b"??"), Some(ErrorCode::UnknownMessage));
    expect_reply(&mut conn, frame(ty::OK, &[]), Some(ErrorCode::Unexpected));
    expect_reply(
        &mut conn,
        frame(ty::BLOCK_DATA, &[1]),
        Some(ErrorCode::Unexpected),
    );
    // Requests that do not decode: a block of 0 or too many sectors, a write that is
    // not whole sectors, short payloads, trailing bytes.
    expect_reply(&mut conn, block_read(0), Some(ErrorCode::Malformed));
    expect_reply(&mut conn, block_read(2049), Some(ErrorCode::Malformed));
    let mut partial = h.to_le_bytes().to_vec();
    partial.extend(0u64.to_le_bytes());
    partial.extend(100u32.to_le_bytes());
    partial.extend([1; 100]);
    expect_reply(
        &mut conn,
        frame(ty::BLOCK_WRITE, &partial),
        Some(ErrorCode::Malformed),
    );
    expect_reply(
        &mut conn,
        frame(ty::BLOCK_OPEN, &[0xff; 3]),
        Some(ErrorCode::Malformed),
    );
    expect_reply(&mut conn, frame(ty::LIST_DISKS, &[0]), Some(ErrorCode::Malformed));
    expect_reply(&mut conn, frame(ty::DISK_CREATE, &[]), Some(ErrorCode::Malformed));
    // Events are never answered, good or bad.
    expect_reply(&mut conn, frame(ty::CONSOLE_INPUT, &[1]), None);
    let input = Frame::from_message(&Message::ConsoleInput {
        vm: 1,
        data: b"x".to_vec(),
    })
    .unwrap();
    expect_reply(&mut conn, input, None);
    // A frame too short to have a type is consumed and answered.
    conn.get_mut().write_all(&[1, 0, 0, 0, 0xee]).unwrap();
    match conn.recv().unwrap() {
        Message::Error { code, .. } => assert_eq!(code, ErrorCode::Malformed),
        other => panic!("{other:?}"),
    }
    // Still a working session, with its attachment.
    assert_eq!(read(&mut conn, h, 0, 1), vec![0; 512]);
    server.stop().unwrap();
}

#[test]
fn an_oversize_frame_ends_the_connection_only() {
    let dir = TempDir::new("oversize");
    let server = lake(&dir);
    let mut rt = connect(&server);
    create_disk(&mut rt, "d", 1);
    let mut conn = connect(&server);
    open(&mut conn, "d", false);
    conn.get_mut().write_all(&(64u32 << 20).to_le_bytes()).unwrap();
    match conn.recv() {
        Ok(Message::Error { code, .. }) => assert_eq!(code, ErrorCode::Malformed),
        other => panic!("{other:?}"),
    }
    assert!(conn.recv().is_err(), "closed");
    // Its attachment went with it.
    wait_until("the writer to be detached", || !disk_info(&mut rt, "d").writer);
    assert_eq!(
        rt.request(&Message::Ping { token: 3 }).unwrap(),
        Message::Pong { token: 3 }
    );
    server.stop().unwrap();
}

#[test]
fn bad_handshakes() {
    let dir = TempDir::new("handshake");
    let server = lake_with(Config {
        handshake_timeout: Duration::from_millis(300),
        ..quiet(&dir)
    });
    // Everything the server sends before it closes the connection.
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

    // A stray HTTP client.
    let mut http = TcpStream::connect(server.addr()).unwrap();
    http.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    read_all(http);

    // A request before Hello: an error, then the connection is closed.
    let mut early = TcpStream::connect(server.addr()).unwrap();
    early.write_all(&encode(&Message::ListDisks).unwrap()).unwrap();
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
    let (msg, _) = avalon_proto::decode(&reply).unwrap();
    assert!(matches!(msg, Message::Welcome { version: 1, .. }), "{msg:?}");

    // Silence: closed once the handshake time is up.
    let silent = TcpStream::connect(server.addr()).unwrap();
    assert!(read_all(silent).is_empty());
    wait_until("the connections to be forgotten", || {
        server.debug_connections() == 0
    });

    // The server carries on.
    let mut ok = connect(&server);
    assert_eq!(list(&mut ok), []);
    server.stop().unwrap();
}

#[test]
fn shutdown_closes_every_connection() {
    let dir = TempDir::new("shutdown");
    let server = lake(&dir);
    let mut a = connect(&server);
    create_disk(&mut a, "d", 1);
    open(&mut a, "d", false);
    let mut b = connect(&server);
    wait_until("both connections", || server.debug_connections() == 2);
    server.stop().unwrap();
    assert!(a.request(&Message::ListDisks).is_err());
    assert!(b.request(&Message::Ping { token: 1 }).is_err());
    // The image is still there for the next server, and free.
    let server = lake(&dir);
    let mut c = connect(&server);
    open(&mut c, "d", false);
    server.stop().unwrap();
}

#[test]
fn bind_errors() {
    let dir = TempDir::new("bind");
    std::fs::write(dir.path(), b"a file, not a directory").unwrap();
    assert!(lake::Server::bind(&quiet(&dir)).is_err());
    std::fs::remove_file(dir.path()).unwrap();
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let config = Config {
        listen: taken.local_addr().unwrap(),
        ..quiet(&dir)
    };
    assert!(lake::Server::bind(&config).is_err());
}
