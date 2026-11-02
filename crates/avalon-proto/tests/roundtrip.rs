//! Every message survives encode/decode and a trip through a `Connection`, over memory
//! and over real TCP; the handshake agrees on versions and refuses mismatches.

mod common;

use avalon_proto::*;
use common::{decode_all, samples, Duplex};
use std::collections::BTreeSet;
use std::net::{TcpListener, TcpStream};
use std::thread;

#[test]
fn samples_cover_the_whole_catalogue() {
    let covered: BTreeSet<u16> = samples().iter().map(Message::type_code).collect();
    let catalogue: BTreeSet<u16> = CATALOGUE.iter().map(|(code, _, _)| *code).collect();
    assert_eq!(covered, catalogue);
    assert_eq!(catalogue.len(), CATALOGUE.len(), "duplicate type code");
    let names: BTreeSet<&str> = CATALOGUE.iter().map(|(_, name, _)| *name).collect();
    assert_eq!(names.len(), CATALOGUE.len(), "duplicate type name");
}

#[test]
fn catalogue_is_in_code_order_and_grouped() {
    let codes: Vec<u16> = CATALOGUE.iter().map(|(code, _, _)| *code).collect();
    assert!(codes.windows(2).all(|w| w[0] < w[1]), "{codes:x?}");
    assert!(codes.iter().all(|c| c >> 8 <= 6));
}

#[test]
fn every_message_round_trips() {
    for msg in samples() {
        let bytes = encode(&msg).unwrap_or_else(|e| panic!("{msg:?}: {e}"));
        // Header: length of type + payload, then the type.
        let len = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        assert_eq!(len, bytes.len() - 4, "{}", msg.name());
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), msg.type_code());
        let (back, used) = decode(&bytes).unwrap_or_else(|e| panic!("{}: {e}", msg.name()));
        assert_eq!(used, bytes.len());
        assert_eq!(back, msg);
        // Same through the frame layer.
        let frame = read_frame(&mut &bytes[..]).unwrap();
        assert_eq!(frame.ty, msg.type_code());
        assert_eq!(frame.message().unwrap(), msg);
        assert_eq!(frame.to_bytes(), bytes);
    }
}

#[test]
fn kinds_and_names_match_the_catalogue() {
    for msg in samples() {
        let entry = CATALOGUE.iter().find(|(c, _, _)| *c == msg.type_code()).unwrap();
        assert_eq!(msg.name(), entry.1);
        assert_eq!(msg.kind(), entry.2);
        assert_eq!(type_name(msg.type_code()), entry.1);
    }
    assert_eq!(type_name(0x7777), "0x7777");
    assert_eq!(
        Message::ConsoleOutput { vm: 0, data: vec![] }.kind(),
        MessageKind::Event
    );
    assert_eq!(Message::ListVms.kind(), MessageKind::Request);
    assert_eq!(Message::Ok.kind(), MessageKind::Reply);
}

#[test]
fn field_layouts_match_the_protocol_doc() {
    // A few byte-exact layouts, so the doc and the code cannot drift apart.
    let hello = encode(&Message::Hello {
        version: 1,
        agent: "rt".into(),
    })
    .unwrap();
    assert_eq!(
        hello,
        [14, 0, 0, 0, 1, 0, b'A', b'V', b'L', b'N', 1, 0, 2, 0, 0, 0, b'r', b't']
    );

    let start = encode(&Message::StartVm { vm: VmRef::Id(5) }).unwrap();
    assert_eq!(start, [7, 0, 0, 0, 0x07, 0x01, 0, 5, 0, 0, 0]);

    let state = encode(&Message::VmStateChanged {
        vm: 2,
        state: VmState::Exited(3),
    })
    .unwrap();
    assert_eq!(state, [11, 0, 0, 0, 0x0c, 0x01, 2, 0, 0, 0, 4, 3, 0, 0, 0]);

    let profile = encode(&Message::CreateVm {
        name: "a".into(),
        profile: HardwareProfile {
            ram_size: 0x1000,
            clock_hz: Some(5),
            nominal_hz: 7,
            devices: vec![DeviceSpec::Disk(DiskSpec {
                source: DiskSource::File("d".into()),
                read_only: true,
            })],
        },
        program: vec![0xee],
    })
    .unwrap();
    #[rustfmt::skip]
    let want = [
        41, 0, 0, 0, 0x01, 0x01,
        1, 0, 0, 0, b'a',                       // name
        0, 0x10, 0, 0, 0, 0, 0, 0,              // ram_size u64
        1, 5, 0, 0, 0,                          // clock_hz Some(5)
        7, 0, 0, 0,                             // nominal_hz
        1, 0, 0, 0,                             // 1 device
        1, 1, 1, 0, 0, 0, b'd', 1,              // disk, file "d", read-only
        1, 0, 0, 0, 0xee,                       // program
    ];
    assert_eq!(profile, want);

    let key = encode(&Message::KeyInput {
        vm: 1,
        key: KeyEvent {
            code: b'c',
            release: true,
            shift: false,
            ctrl: true,
        },
    })
    .unwrap();
    assert_eq!(key, [8, 0, 0, 0, 0x04, 0x03, 1, 0, 0, 0, b'c', 0b101]);
}

#[test]
fn error_codes_round_trip() {
    let mut seen = BTreeSet::new();
    for code in ErrorCode::all() {
        let n = code.to_u16();
        assert!(seen.insert(n), "{code:?} reuses {n}");
        assert_eq!(ErrorCode::from_u16(n), code);
        assert_ne!(code.to_string(), format!("error {n}"));
    }
    assert_eq!(seen, (1..=20).collect());
    assert_eq!(ErrorCode::from_u16(0), ErrorCode::Other(0));
    assert_eq!(ErrorCode::from_u16(500).to_u16(), 500);
    assert_eq!(ErrorCode::Other(500).to_string(), "error 500");

    // `Other` with a named code's number would come back as the named code, so the
    // encoder refuses it: every message that encodes decodes to itself.
    for n in 1..=20 {
        let msg = Message::Error {
            code: ErrorCode::Other(n),
            message: "x".into(),
        };
        assert!(matches!(encode(&msg), Err(Error::Invalid(_))), "{n}");
    }
    for n in [0, 21, u16::MAX] {
        let msg = Message::Error {
            code: ErrorCode::Other(n),
            message: "x".into(),
        };
        assert_eq!(decode(&encode(&msg).unwrap()).unwrap().0, msg);
    }
}

#[test]
fn connection_round_trips_every_message_in_memory() {
    let msgs = samples();
    let mut conn = Connection::new(Duplex::with_messages(&msgs));
    for msg in &msgs {
        assert_eq!(&conn.recv().unwrap(), msg);
        conn.send(msg).unwrap();
    }
    assert!(matches!(conn.recv(), Err(Error::Closed)));
    assert_eq!(decode_all(&conn.get_ref().output), msgs);
}

#[test]
fn request_queues_events_that_arrive_before_the_reply() {
    let out1 = Message::ConsoleOutput {
        vm: 1,
        data: b"boot".to_vec(),
    };
    let changed = Message::VmStateChanged {
        vm: 1,
        state: VmState::Paused,
    };
    let out2 = Message::ConsoleOutput {
        vm: 1,
        data: b"more".to_vec(),
    };
    let mut conn = Connection::new(Duplex::with_messages(&[
        out1.clone(),
        changed.clone(),
        Message::Ok,
        out2.clone(),
    ]));
    conn.request_ok(&Message::PauseVm { vm: VmRef::Id(1) }).unwrap();
    assert_eq!(conn.queued_events(), 2);
    assert_eq!(conn.recv().unwrap(), out1);
    assert_eq!(conn.recv().unwrap(), changed);
    assert_eq!(conn.recv().unwrap(), out2);
    assert_eq!(
        decode_all(&conn.get_ref().output),
        [Message::PauseVm { vm: VmRef::Id(1) }]
    );
}

#[test]
fn request_turns_error_replies_into_remote_errors() {
    let mut conn = Connection::new(Duplex::with_messages(&[
        Message::Error {
            code: ErrorCode::NoSuchVm,
            message: "no VM named 'x'".into(),
        },
        Message::Pong { token: 9 },
        Message::Ok,
    ]));
    match conn.request(&Message::StartVm {
        vm: VmRef::Name("x".into()),
    }) {
        Err(Error::Remote { code, message }) => {
            assert_eq!(code, ErrorCode::NoSuchVm);
            assert_eq!(message, "no VM named 'x'");
        }
        other => panic!("{other:?}"),
    }
    // The connection is still in step.
    assert_eq!(
        conn.request(&Message::Ping { token: 9 }).unwrap(),
        Message::Pong { token: 9 }
    );
    // `request_ok` refuses a reply that is not `Ok`... and accepts `Ok`.
    conn.request_ok(&Message::ListVms).unwrap();
}

#[test]
fn request_refuses_non_requests_and_unexpected_replies() {
    let mut conn = Connection::new(Duplex::with_messages(&[
        Message::Pong { token: 1 },
        Message::Pong { token: 2 },
    ]));
    assert!(matches!(conn.request(&Message::Ok), Err(Error::Invalid(_))));
    assert!(conn.get_ref().output.is_empty(), "nothing is sent");
    // A reply of the wrong type is still the reply: the next request gets its own.
    match conn.request_ok(&Message::Ping { token: 1 }) {
        Err(e @ Error::Unexpected(ty::PONG)) => assert!(!e.is_fatal()),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        conn.request(&Message::Ping { token: 2 }).unwrap(),
        Message::Pong { token: 2 }
    );
}

/// Frames as a server would send them: `bad` (raw bytes), then the given messages.
fn bad_then(bad: Vec<u8>, msgs: &[Message]) -> Duplex {
    let mut input = bad;
    input.extend(msgs.iter().flat_map(|m| encode(m).unwrap()));
    Duplex::new(input)
}

fn raw_frame(ty: u16, payload: &[u8]) -> Vec<u8> {
    Frame {
        ty,
        payload: payload.to_vec(),
    }
    .to_bytes()
}

#[test]
fn a_bad_event_before_the_reply_does_not_shift_replies() {
    // A KeyInput with a reserved flag bit and a VmStateChanged with an unknown state
    // tag arrive before the replies. Both are set aside as errors for `recv`; each
    // request still gets its own reply.
    let mut input = raw_frame(ty::KEY_INPUT, &[1, 0, 0, 0, b'a', 0x80]);
    input.extend(raw_frame(ty::VM_STATE_CHANGED, &[1, 0, 0, 0, 9]));
    let mut conn = Connection::new(bad_then(
        input,
        &[
            Message::Pong { token: 1 },
            Message::Ok,
            Message::Pong { token: 2 },
        ],
    ));
    assert_eq!(
        conn.request(&Message::Ping { token: 1 }).unwrap(),
        Message::Pong { token: 1 }
    );
    conn.request_ok(&Message::PauseVm { vm: VmRef::Id(1) }).unwrap();
    assert_eq!(
        conn.request(&Message::Ping { token: 2 }).unwrap(),
        Message::Pong { token: 2 }
    );
    assert_eq!(conn.queued_events(), 2);
    for _ in 0..2 {
        match conn.recv() {
            Err(e @ Error::Invalid(_)) => assert!(!e.is_fatal()),
            other => panic!("{other:?}"),
        }
    }
    assert!(matches!(conn.recv(), Err(Error::Closed)));
}

#[test]
fn a_bad_reply_is_the_reply() {
    // A Pong too short to decode is still the Ping's one reply.
    let mut conn = Connection::new(bad_then(
        raw_frame(ty::PONG, &[1, 0]),
        &[Message::Pong { token: 2 }],
    ));
    match conn.request(&Message::Ping { token: 1 }) {
        Err(e @ Error::Truncated) => assert!(!e.is_fatal()),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        conn.request(&Message::Ping { token: 2 }).unwrap(),
        Message::Pong { token: 2 }
    );
}

#[test]
fn a_frame_that_might_be_the_reply_puts_the_connection_out_of_step() {
    let ping = Message::Ping { token: 1 };
    let cases = [
        // A request where a reply belongs, well-formed or not.
        encode(&Message::ListVms).unwrap(),
        raw_frame(ty::START_VM, &[9]),
        // A type this version does not know.
        raw_frame(0x0777, &[]),
        // A frame too short to hold a type.
        vec![1, 0, 0, 0, 0xee],
    ];
    for bad in cases {
        let mut conn = Connection::new(bad_then(
            bad.clone(),
            &[Message::Pong { token: 1 }, Message::Pong { token: 2 }],
        ));
        match conn.request(&ping) {
            Err(e @ Error::OutOfStep(_)) => {
                assert!(e.is_fatal());
                assert!(e.to_string().contains("out of step"), "{e}");
            }
            other => panic!("{bad:02x?}: {other:?}"),
        }
        // Never hands out a reply that may belong to the earlier request.
        assert!(matches!(
            conn.request(&Message::Ping { token: 2 }),
            Err(Error::OutOfStep(_))
        ));
        assert_eq!(decode_all(&conn.get_ref().output), std::slice::from_ref(&ping));
    }
}

fn tcp_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    (client, server)
}

#[test]
fn every_message_crosses_tcp_both_ways() {
    let (client, server) = tcp_pair();
    let echo = thread::spawn(move || {
        let mut conn = Connection::new(server);
        let mut n = 0;
        loop {
            match conn.recv() {
                Ok(msg) => {
                    conn.send(&msg).unwrap();
                    n += 1;
                }
                Err(Error::Closed) => return n,
                Err(e) => panic!("{e}"),
            }
        }
    });
    let msgs = samples();
    let mut conn = Connection::new(client);
    for msg in &msgs {
        conn.send(msg).unwrap();
        assert_eq!(&conn.recv().unwrap(), msg);
    }
    drop(conn);
    assert_eq!(echo.join().unwrap(), msgs.len());
}

#[test]
fn handshake_over_tcp() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut conn = Connection::new(stream);
        let agent = conn.server_handshake("avalon-test").unwrap();
        let req = conn.recv().unwrap();
        assert_eq!(req, Message::ListVms);
        conn.send(&Message::VmList { vms: Vec::new() }).unwrap();
        agent
    });
    let mut conn = Connection::connect(addr, "round-table").unwrap();
    assert_eq!(
        conn.request(&Message::ListVms).unwrap(),
        Message::VmList { vms: Vec::new() }
    );
    assert_eq!(server.join().unwrap(), "round-table");
    // A cloned handle shares the socket.
    let clone = conn.try_clone().unwrap();
    assert_eq!(clone.get_ref().peer_addr().unwrap(), addr);
}

#[test]
fn client_sees_server_version_mismatch() {
    let mut conn = Connection::new(Duplex::with_messages(&[Message::Welcome {
        version: PROTOCOL_VERSION + 1,
        server: "future avalon".into(),
    }]));
    match conn.client_handshake("herald") {
        Err(e @ Error::VersionMismatch { ours, theirs }) => {
            assert_eq!((ours, theirs), (PROTOCOL_VERSION, PROTOCOL_VERSION + 1));
            assert!(e.is_fatal());
            assert!(e.to_string().contains("version mismatch"), "{e}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        decode_all(&conn.get_ref().output),
        [Message::Hello {
            version: PROTOCOL_VERSION,
            agent: "herald".into()
        }]
    );
}

#[test]
fn server_answers_a_mismatched_client_with_its_version() {
    let mut conn = Connection::new(Duplex::with_messages(&[Message::Hello {
        version: 0,
        agent: "ancient".into(),
    }]));
    assert!(matches!(
        conn.server_handshake("avalon"),
        Err(Error::VersionMismatch {
            ours: PROTOCOL_VERSION,
            theirs: 0
        })
    ));
    assert_eq!(
        decode_all(&conn.get_ref().output),
        [Message::Welcome {
            version: PROTOCOL_VERSION,
            server: "avalon".into()
        }]
    );
}

#[test]
fn version_mismatch_over_tcp() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        Connection::new(stream).server_handshake("avalon")
    });
    let mut conn = Connection::new(TcpStream::connect(addr).unwrap());
    conn.send(&Message::Hello {
        version: PROTOCOL_VERSION + 7,
        agent: "future".into(),
    })
    .unwrap();
    assert_eq!(
        conn.recv().unwrap(),
        Message::Welcome {
            version: PROTOCOL_VERSION,
            server: "avalon".into()
        }
    );
    assert!(matches!(
        server.join().unwrap(),
        Err(Error::VersionMismatch { theirs, .. }) if theirs == PROTOCOL_VERSION + 7
    ));
    // The server dropped the connection.
    assert!(matches!(conn.recv(), Err(Error::Closed | Error::Io(_))));
}

#[test]
fn server_handshake_rejects_anything_but_hello() {
    let mut conn = Connection::new(Duplex::with_messages(&[Message::ListVms]));
    assert!(matches!(
        conn.server_handshake("avalon"),
        Err(Error::Unexpected(ty::LIST_VMS))
    ));
    match &decode_all(&conn.get_ref().output)[..] {
        [Message::Error { code, .. }] => assert_eq!(*code, ErrorCode::Unexpected),
        other => panic!("{other:?}"),
    }

    // Hello with the wrong magic.
    let mut bytes = encode(&Message::Hello {
        version: PROTOCOL_VERSION,
        agent: String::new(),
    })
    .unwrap();
    bytes[6..10].copy_from_slice(b"HTTP");
    let mut conn = Connection::new(Duplex::new(bytes));
    assert!(matches!(conn.server_handshake("avalon"), Err(Error::BadMagic(m)) if &m == b"HTTP"));
    match &decode_all(&conn.get_ref().output)[..] {
        [Message::Error { code, .. }] => assert_eq!(*code, ErrorCode::Malformed),
        other => panic!("{other:?}"),
    }

    // A client that hangs up before saying anything.
    let mut conn = Connection::new(Duplex::new(Vec::new()));
    assert!(matches!(conn.server_handshake("avalon"), Err(Error::Closed)));
    assert!(conn.get_ref().output.is_empty());
}

#[test]
fn client_handshake_reports_server_errors() {
    let mut conn = Connection::new(Duplex::with_messages(&[Message::Error {
        code: ErrorCode::Malformed,
        message: "bad hello".into(),
    }]));
    assert!(matches!(
        conn.client_handshake("x"),
        Err(Error::Remote {
            code: ErrorCode::Malformed,
            ..
        })
    ));
    let mut conn = Connection::new(Duplex::with_messages(&[Message::Ok]));
    assert!(matches!(
        conn.client_handshake("x"),
        Err(Error::Unexpected(ty::OK))
    ));
}

#[test]
fn vm_ref_parses_ids_and_names() {
    assert_eq!(VmRef::parse("12"), VmRef::Id(12));
    assert_eq!(VmRef::parse("web-1"), VmRef::Name("web-1".into()));
    assert_eq!(VmRef::parse("+1"), VmRef::Name("+1".into()));
    assert_eq!(VmRef::parse("99999999999"), VmRef::Name("99999999999".into()));
    // A valid name never parses as an id.
    for name in ["web-1", "1a", "1.5", "0x10", "_7"] {
        assert_eq!(validate_name(name), Ok(()), "{name}");
        assert_eq!(VmRef::parse(name), VmRef::Name(name.into()));
    }
    assert_eq!(VmRef::Id(3).to_string(), "3");
    assert_eq!(VmState::Exited(2).to_string(), "exited(2)");
}

#[test]
fn names_are_validated() {
    for ok in [
        "a",
        "web-1",
        "root.img",
        "a_b.c-9",
        "a.b.c",
        "x-",
        "x_",
        "7z",
        // Device names only count as the whole stem.
        "cons",
        "nul_",
        "com10",
        "lpt",
        "my.con",
        "aux-1.img",
        &"x".repeat(MAX_NAME_LEN),
    ] {
        assert_eq!(validate_name(ok), Ok(()), "{ok}");
    }
    for bad in [
        "",
        ".hidden",
        "-x",
        "a/b",
        "a\\b",
        "a:b",
        "..",
        "a b",
        "é",
        &"x".repeat(MAX_NAME_LEN + 1),
        // Windows ignores case and trailing dots: these would share a file with
        // "disk".
        "Disk",
        "DISK",
        "disk.",
        "x..",
        "a...",
        // Windows devices, with or without an extension, in any case.
        "con",
        "CON",
        "prn",
        "aux",
        "aux.img",
        "nul",
        "nul.tar.gz",
        "com0",
        "com1",
        "COM1",
        "com9.img",
        "lpt0",
        "lpt1",
        "LPT1.img",
        // All digits: a VM argument like that is an id.
        "42",
        "007",
        "4294967296",
    ] {
        assert!(validate_name(bad).is_err(), "{bad}");
    }
}

#[test]
fn default_addresses_are_loopback() {
    for addr in [DEFAULT_AVALON_ADDR, DEFAULT_LAKE_ADDR] {
        let addr: std::net::SocketAddr = addr.parse().unwrap();
        assert!(addr.ip().is_loopback());
    }
}
