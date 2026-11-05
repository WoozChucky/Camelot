//! Bad input is an error, never a panic: truncated and padded frames, oversize and
//! undersize lengths, unknown types, out-of-range field values, and connections that
//! end in the middle of a frame.

mod common;

use avalon_proto::*;
use common::{samples, Duplex};
use std::io::{self, Read};

/// Replaces the payload of a frame, fixing up its length field.
fn with_payload(ty: u16, payload: &[u8]) -> Vec<u8> {
    Frame {
        ty,
        payload: payload.to_vec(),
    }
    .to_bytes()
}

fn payload_of(msg: &Message) -> Vec<u8> {
    Frame::from_message(msg).unwrap().payload
}

#[test]
fn every_prefix_of_a_frame_is_truncated() {
    for msg in samples() {
        let bytes = encode(&msg).unwrap();
        // Skip most of the big frames' prefixes; the interesting cuts are near fields.
        let cuts: Vec<usize> = if bytes.len() > 4096 {
            (0..256).chain(bytes.len() - 256..bytes.len()).collect()
        } else {
            (0..bytes.len()).collect()
        };
        for cut in cuts {
            assert!(
                matches!(decode(&bytes[..cut]), Err(Error::Truncated)),
                "{} cut at {cut}",
                msg.name()
            );
        }
    }
}

#[test]
fn every_shortened_payload_is_truncated() {
    // The frame is complete, but the payload stops early: every field length in the
    // protocol is explicit, so this is always caught.
    for msg in samples() {
        let payload = payload_of(&msg);
        let cuts: Vec<usize> = if payload.len() > 4096 {
            (0..256).chain(payload.len() - 256..payload.len()).collect()
        } else {
            (0..payload.len()).collect()
        };
        for cut in cuts {
            let frame = with_payload(msg.type_code(), &payload[..cut]);
            assert!(
                matches!(decode(&frame), Err(Error::Truncated)),
                "{} payload cut at {cut}: {:?}",
                msg.name(),
                decode(&frame)
            );
        }
    }
}

#[test]
fn trailing_bytes_are_rejected() {
    for msg in samples() {
        let mut payload = payload_of(&msg);
        payload.extend_from_slice(&[0, 0, 0]);
        let frame = with_payload(msg.type_code(), &payload);
        assert!(
            matches!(decode(&frame), Err(Error::TrailingBytes(3))),
            "{}",
            msg.name()
        );
    }
}

#[test]
fn oversize_length_is_rejected_without_reading_the_body() {
    /// A stream that holds only a header and fails the test if read past it.
    struct HeaderOnly(Vec<u8>, usize);
    impl Read for HeaderOnly {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            assert!(self.1 < self.0.len(), "read past the length field");
            let n = buf.len().min(self.0.len() - self.1);
            buf[..n].copy_from_slice(&self.0[self.1..self.1 + n]);
            self.1 += n;
            Ok(n)
        }
    }
    for len in [MAX_FRAME_LEN + 1, 0x8000_0000, u32::MAX] {
        let header = len.to_le_bytes().to_vec();
        let mut stream = HeaderOnly(header.clone(), 0);
        match read_frame(&mut stream) {
            Err(e @ Error::FrameTooLarge(n)) => {
                assert_eq!(n, len as u64);
                assert!(e.is_fatal());
                assert_eq!(e.code(), ErrorCode::Malformed);
            }
            other => panic!("{other:?}"),
        }
        let mut buf = header;
        buf.extend_from_slice(&[0x04, 0x00]);
        assert!(matches!(decode(&buf), Err(Error::FrameTooLarge(n)) if n == len as u64));
    }
}

#[test]
fn largest_frame_is_accepted() {
    // Length field exactly MAX_FRAME_LEN: type + vm + data length + data.
    let data_len = MAX_FRAME_LEN as usize - 2 - 4 - 4;
    let msg = Message::ConsoleOutput {
        vm: 1,
        data: vec![b'x'; data_len],
    };
    let bytes = encode(&msg).unwrap();
    assert_eq!(bytes.len(), 4 + MAX_FRAME_LEN as usize);
    assert_eq!(read_frame(&mut &bytes[..]).unwrap().message().unwrap(), msg);

    // One byte more does not fit.
    let msg = Message::ConsoleOutput {
        vm: 1,
        data: vec![b'x'; data_len + 1],
    };
    assert!(matches!(encode(&msg), Err(Error::FrameTooLarge(n)) if n == MAX_FRAME_LEN as u64 + 1));
    let mut conn = Connection::new(Duplex::new(Vec::new()));
    assert!(matches!(conn.send(&msg), Err(Error::FrameTooLarge(_))));
    assert!(conn.get_ref().output.is_empty(), "nothing is sent");
    let frame = Frame {
        ty: ty::CONSOLE_OUTPUT,
        payload: vec![0; MAX_FRAME_LEN as usize - 1],
    };
    assert!(matches!(
        write_frame(&mut Vec::new(), &frame),
        Err(Error::FrameTooLarge(_))
    ));
}

#[test]
fn undersize_lengths_are_rejected() {
    for len in [0u32, 1] {
        let mut buf = len.to_le_bytes().to_vec();
        buf.extend_from_slice(&[0; 4]);
        match decode(&buf) {
            Err(e @ Error::FrameTooShort(n)) => {
                assert_eq!(n, len);
                assert!(!e.is_fatal());
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn unknown_types_are_rejected_and_the_stream_stays_in_step() {
    let known: Vec<u16> = CATALOGUE.iter().map(|(c, _, _)| *c).collect();
    for code in (0..=u16::MAX).filter(|c| !known.contains(c)) {
        assert!(matches!(decode(&with_payload(code, &[])), Err(Error::UnknownType(c)) if c == code));
    }
    let mut input = with_payload(0x0abc, &[1, 2, 3]);
    input.extend(encode(&Message::Ping { token: 5 }).unwrap());
    let mut conn = Connection::new(Duplex::new(input));
    match conn.recv() {
        Err(e @ Error::UnknownType(0x0abc)) => {
            assert!(!e.is_fatal());
            assert_eq!(e.code(), ErrorCode::UnknownMessage);
            conn.send_error(&e).unwrap();
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(conn.recv().unwrap(), Message::Ping { token: 5 });
    let (reply, _) = decode(&conn.get_ref().output).unwrap();
    assert!(matches!(
        reply,
        Message::Error {
            code: ErrorCode::UnknownMessage,
            ..
        }
    ));
}

#[test]
fn bad_payloads_leave_the_stream_in_step() {
    let mut input = with_payload(ty::PING, &[1, 2]);
    input.extend(with_payload(ty::START_VM, &[9, 0, 0, 0, 0]));
    input.extend(encode(&Message::Ok).unwrap());
    input.extend_from_slice(&[0x10, 0, 0]); // half a length field
    let mut conn = Connection::new(Duplex::new(input));
    assert!(matches!(conn.recv(), Err(Error::Truncated)));
    assert!(matches!(conn.recv(), Err(Error::Invalid(_))));
    assert_eq!(conn.recv().unwrap(), Message::Ok);
    match conn.recv() {
        Err(Error::Io(e)) => assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof),
        other => panic!("{other:?}"),
    }
}

#[test]
fn connection_closed_mid_frame_is_an_io_error() {
    let bytes = encode(&Message::Ping { token: 1 }).unwrap();
    for cut in 1..bytes.len() {
        match read_frame(&mut &bytes[..cut]) {
            Err(e @ Error::Io(_)) => assert!(e.is_fatal()),
            other => panic!("cut at {cut}: {other:?}"),
        }
    }
    assert!(matches!(read_frame(&mut &bytes[..0]), Err(Error::Closed)));
}

/// Field values outside what the protocol allows, as (type, payload).
fn invalid_payloads() -> Vec<(&'static str, u16, Vec<u8>)> {
    let le = |v: u32| v.to_le_bytes().to_vec();
    let cat = |parts: &[&[u8]]| parts.concat();
    vec![
        ("bool 2", ty::BLOCK_OPEN, cat(&[&le(1), b"a", &[2]])),
        ("VM ref tag", ty::START_VM, cat(&[&[2], &le(1)])),
        ("VM state tag", ty::VM_STATE_CHANGED, cat(&[&le(1), &[6]])),
        ("attach kind 0", ty::DETACHED, cat(&[&le(1), &[0]])),
        ("attach kind 4", ty::DETACHED, cat(&[&le(1), &[4]])),
        ("debug attach VM ref tag", ty::DEBUG_ATTACH, cat(&[&[2], &le(1)])),
        (
            "debug attach invalid UTF-8",
            ty::DEBUG_ATTACH,
            cat(&[&[1], &le(2), &[0xc3, 0x28]]),
        ),
        ("invalid UTF-8", ty::DISK_DELETE, cat(&[&le(2), &[0xc3, 0x28]])),
        ("option tag", ty::CREATE_VM, cat(&[&le(0), &[0; 8], &[2]])),
        // A paging mode is satp's 9 (Sv48) or 8 (Sv39).
        (
            "paging mode 10 (Sv57)",
            ty::CREATE_VM,
            cat(&[&le(0), &[0; 8], &[0], &le(1), &le(1), &[10], &le(0), &le(0)]),
        ),
        (
            "paging mode 1 (Sv32)",
            ty::CREATE_VM,
            cat(&[&le(0), &[0; 8], &[0], &le(1), &le(1), &[1], &le(0), &le(0)]),
        ),
        // A firmware is none (0), default (1) or a file (2).
        (
            "firmware tag 3",
            ty::CREATE_VM,
            cat(&[&le(0), &[0; 8], &[0], &le(1), &le(1), &[9, 3], &le(0), &le(0)]),
        ),
        (
            "device tag",
            ty::CREATE_VM,
            cat(&[&le(0), &[0; 8], &[0], &le(1), &le(1), &[9, 0], &le(1), &[9]]),
        ),
        (
            "disk source tag",
            ty::CREATE_VM,
            cat(&[&le(0), &[0; 8], &[0], &le(1), &le(1), &[9, 0], &le(1), &[1, 7]]),
        ),
        (
            "card switch option tag",
            ty::CREATE_VM,
            cat(&[&le(0), &[0; 8], &[0], &le(1), &le(1), &[9, 0], &le(1), &[2, 7]]),
        ),
        (
            "card MAC option tag",
            ty::CREATE_VM,
            cat(&[&le(0), &[0; 8], &[0], &le(1), &le(1), &[9, 0], &le(1), &[2, 0, 3]]),
        ),
        (
            "port owner tag",
            ty::SWITCH_LIST,
            cat(&[&le(1), &le(0), &[0], &[0], &le(1), &[3], &[0; 6], &[0; 40]]),
        ),
        ("NAT option tag", ty::CREATE_SWITCH, cat(&[&le(1), b"s", &[2]])),
        (
            "forwarding protocol tag",
            ty::CREATE_SWITCH,
            cat(&[
                &le(1),
                b"s",
                &[1],
                &le(1),
                &[3],
                &[127, 0, 0, 1, 1, 0],
                &[10, 0, 2, 15, 80, 0],
            ]),
        ),
        (
            "guest port 0",
            ty::CREATE_SWITCH,
            cat(&[
                &le(1),
                b"s",
                &[1],
                &le(1),
                &[1],
                &[127, 0, 0, 1, 1, 0],
                &[10, 0, 2, 15, 0, 0],
            ]),
        ),
        (
            "guest address of the gateway",
            ty::SET_SWITCH_NAT,
            cat(&[
                &le(1),
                b"s",
                &[1],
                &le(1),
                &[1],
                &[127, 0, 0, 1, 1, 0],
                &[10, 0, 2, 2, 80, 0],
            ]),
        ),
        (
            "guest address off the network",
            ty::SWITCH_LIST,
            cat(&[
                &le(1),
                &le(1),
                b"s",
                &[0],
                &[1],
                &le(1),
                &[2],
                &[127, 0, 0, 1, 1, 0],
                &[192, 168, 0, 1, 80, 0],
                &le(0),
                &le(0),
            ]),
        ),
        (
            "more forwarding rules than MAX_FORWARDS",
            ty::CREATE_SWITCH,
            cat(&[
                &le(1),
                b"s",
                &[1],
                &le(MAX_FORWARDS as u32 + 1),
                &[1, 127, 0, 0, 1, 0, 0, 10, 0, 2, 15, 80, 0].repeat(MAX_FORWARDS + 1),
            ]),
        ),
        (
            "capture option tag",
            ty::CAPTURE_SWITCH,
            cat(&[&le(1), b"s", &[2]]),
        ),
        (
            "key code and usage 0",
            ty::KEY_INPUT,
            cat(&[&le(1), &[0, 0, 0, 0]]),
        ),
        ("key flags", ty::KEY_INPUT, cat(&[&le(1), &[b'a', 0x10, 0x04, 0]])),
        // Release (bit 0) and repeat (bit 3): a repeat is a press.
        (
            "key repeat on a release",
            ty::KEY_INPUT,
            cat(&[&le(1), &[b'a', 0x09, 0x04, 0]]),
        ),
        (
            "pointer motion tag",
            ty::POINTER_INPUT,
            cat(&[&le(1), &[3, 0, 0]]),
        ),
        (
            "pointer button 3",
            ty::POINTER_INPUT,
            cat(&[&le(1), &[0, 0x08, 0]]),
        ),
        (
            "pointer buttons",
            ty::POINTER_INPUT,
            cat(&[&le(1), &[1], &le(5), &le(5), &[0xf9, 0]]),
        ),
        ("read 0 sectors", ty::BLOCK_READ, cat(&[&le(1), &[0; 8], &le(0)])),
        (
            "read too many sectors",
            ty::BLOCK_READ,
            cat(&[&le(1), &[0; 8], &le(MAX_BLOCK_SECTORS + 1)]),
        ),
        ("write 0 bytes", ty::BLOCK_WRITE, cat(&[&le(1), &[0; 8], &le(0)])),
        (
            "write part of a sector",
            ty::BLOCK_WRITE,
            cat(&[&le(1), &[0; 8], &le(511), &[0; 511]]),
        ),
        (
            "data 513 bytes",
            ty::BLOCK_DATA,
            cat(&[&le(1), &[0; 8], &le(513), &[0; 513]]),
        ),
        (
            "write too much",
            ty::BLOCK_WRITE,
            cat(&[
                &le(1),
                &[0; 8],
                &le((MAX_BLOCK_SECTORS + 1) * 512),
                &vec![0; (MAX_BLOCK_SECTORS as usize + 1) * 512],
            ]),
        ),
        (
            "pixels too short",
            ty::SCREEN_FRAME,
            cat(&[&le(1), &le(0), &le(0), &le(2), &le(2), &le(15), &[0; 15]]),
        ),
        (
            "pixels too long",
            ty::SCREEN_FRAME,
            cat(&[&le(1), &le(0), &le(0), &le(1), &le(1), &le(5), &[0; 5]]),
        ),
        (
            "empty rectangle",
            ty::SCREEN_FRAME,
            cat(&[&le(1), &le(0), &le(0), &le(0), &le(9), &le(0)]),
        ),
        (
            "huge rectangle",
            ty::SCREEN_FRAME,
            cat(&[&le(1), &le(0), &le(0), &le(u32::MAX), &le(u32::MAX), &le(0)]),
        ),
    ]
}

#[test]
fn invalid_field_values_are_rejected() {
    for (what, code, payload) in invalid_payloads() {
        match decode(&with_payload(code, &payload)) {
            Err(e @ Error::Invalid(_)) => {
                assert!(!e.is_fatal());
                assert_eq!(e.code(), ErrorCode::Malformed, "{what}");
            }
            other => panic!("{what}: {other:?}"),
        }
    }
}

#[test]
fn encoder_refuses_what_the_decoder_would_reject() {
    let bad = [
        Message::KeyInput {
            vm: 1,
            key: KeyEvent {
                code: 0,
                release: false,
                shift: false,
                ctrl: false,
                repeat: false,
                usage: 0,
            },
        },
        Message::KeyInput {
            vm: 1,
            key: KeyEvent {
                code: b'a',
                release: true,
                shift: false,
                ctrl: false,
                repeat: true,
                usage: 0x04,
            },
        },
        Message::BlockRead {
            handle: 1,
            sector: 0,
            count: 0,
        },
        Message::BlockWrite {
            handle: 1,
            sector: 0,
            data: vec![0; 100],
        },
        Message::BlockData {
            handle: 1,
            sector: 0,
            data: Vec::new(),
        },
        Message::ScreenFrame {
            vm: 1,
            x: 0,
            y: 0,
            width: 3,
            height: 3,
            pixels: vec![0; 35],
        },
        // A cursor's pixels must be its size, the size 1 to MAX_CURSOR_SIZE each way,
        // and the hotspot in it.
        Message::CursorShape {
            vm: 1,
            width: 2,
            height: 2,
            hot_x: 0,
            hot_y: 0,
            pixels: vec![0; 15],
        },
        Message::CursorShape {
            vm: 1,
            width: 0,
            height: 2,
            hot_x: 0,
            hot_y: 0,
            pixels: Vec::new(),
        },
        Message::CursorShape {
            vm: 1,
            width: 1,
            height: MAX_CURSOR_SIZE + 1,
            hot_x: 0,
            hot_y: 0,
            pixels: vec![0; (MAX_CURSOR_SIZE as usize + 1) * 4],
        },
        Message::CursorShape {
            vm: 1,
            width: 2,
            height: 2,
            hot_x: 2,
            hot_y: 0,
            pixels: vec![0; 16],
        },
        Message::CursorShape {
            vm: 1,
            width: 2,
            height: 2,
            hot_x: 0,
            hot_y: 2,
            pixels: vec![0; 16],
        },
        Message::PointerInput {
            vm: 1,
            pointer: PointerChange {
                motion: PointerMotion::None,
                buttons: 0x80,
                wheel: 0,
            },
        },
        Message::SetSwitchNat {
            switch: "s".into(),
            nat: Some(NatConfig {
                hostfwd: vec![PortForward {
                    proto: FwdProto::Tcp,
                    host: "127.0.0.1:8080".parse().unwrap(),
                    guest: "10.0.2.15:0".parse().unwrap(),
                }],
            }),
        },
        Message::CreateSwitch {
            name: "s".into(),
            nat: Some(NatConfig {
                hostfwd: vec![PortForward {
                    proto: FwdProto::Udp,
                    host: "127.0.0.1:53".parse().unwrap(),
                    guest: "10.0.3.15:53".parse().unwrap(),
                }],
            }),
        },
    ];
    for msg in bad {
        assert!(matches!(encode(&msg), Err(Error::Invalid(_))), "{msg:?}");
        assert!(msg.check().is_err());
    }
}

#[test]
fn a_gateway_has_at_most_max_forwards_rules() {
    // Each rule binds a socket on the server's host, so the count is bounded: the
    // encoder refuses one rule too many, in a request or in a SwitchList, and
    // MAX_FORWARDS rules go through.
    let rules = |n: usize| -> Vec<PortForward> {
        (0..n)
            .map(|i| PortForward {
                proto: FwdProto::Tcp,
                host: format!("127.0.0.1:{}", 20000 + i).parse().unwrap(),
                guest: "10.0.2.15:80".parse().unwrap(),
            })
            .collect()
    };
    let create = |n| Message::CreateSwitch {
        name: "s".into(),
        nat: Some(NatConfig { hostfwd: rules(n) }),
    };
    let set = |n| Message::SetSwitchNat {
        switch: "s".into(),
        nat: Some(NatConfig { hostfwd: rules(n) }),
    };
    let list = |n| Message::SwitchList {
        switches: vec![SwitchInfo {
            name: "s".into(),
            capture: None,
            nat: Some(NatInfo {
                hostfwd: rules(n),
                leases: Vec::new(),
            }),
            ports: Vec::new(),
        }],
    };
    for make in [&create as &dyn Fn(usize) -> Message, &set, &list] {
        let fine = make(MAX_FORWARDS);
        let (back, _) = decode(&encode(&fine).unwrap()).unwrap();
        assert_eq!(back, fine);
        let over = make(MAX_FORWARDS + 1);
        assert!(matches!(encode(&over), Err(Error::Invalid(_))), "{}", over.name());
        // (invalid_payloads has the frame a peer could send anyway, which the decoder
        // reports as Malformed.)
    }
}

#[test]
fn hello_needs_the_magic() {
    let mut payload = payload_of(&Message::Hello {
        version: PROTOCOL_VERSION,
        agent: "x".into(),
    });
    payload[..4].copy_from_slice(b"AVLX");
    match decode(&with_payload(ty::HELLO, &payload)) {
        Err(e @ Error::BadMagic(_)) => {
            assert_eq!(e.code(), ErrorCode::Malformed);
            assert!(e.to_string().contains("not an Avalon client"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_stray_http_client_is_an_oversize_frame() {
    // "GET " as a little-endian length is about 540 MB: refused before reading on.
    let mut stream: &[u8] = b"GET / HTTP/1.1\r\nHost: x\r\n\r\n";
    assert!(matches!(read_frame(&mut stream), Err(Error::FrameTooLarge(_))));
}

#[test]
fn error_display_and_codes() {
    let cases: Vec<(Error, ErrorCode, bool)> = vec![
        (
            Error::Io(io::ErrorKind::BrokenPipe.into()),
            ErrorCode::Internal,
            true,
        ),
        (Error::Closed, ErrorCode::Internal, true),
        (Error::FrameTooLarge(1 << 30), ErrorCode::Malformed, true),
        (Error::FrameTooShort(1), ErrorCode::Malformed, false),
        (Error::Truncated, ErrorCode::Malformed, false),
        (Error::TrailingBytes(2), ErrorCode::Malformed, false),
        (Error::UnknownType(0x999), ErrorCode::UnknownMessage, false),
        (Error::Invalid("x".into()), ErrorCode::Malformed, false),
        (Error::BadMagic(*b"GET "), ErrorCode::Malformed, false),
        (
            Error::VersionMismatch { ours: 1, theirs: 2 },
            ErrorCode::Internal,
            true,
        ),
        (
            Error::Remote {
                code: ErrorCode::Busy,
                message: "disk has a writer".into(),
            },
            ErrorCode::Busy,
            false,
        ),
        (Error::Unexpected(ty::OK), ErrorCode::Unexpected, false),
        (Error::OutOfStep("x".into()), ErrorCode::Unexpected, true),
    ];
    for (err, code, fatal) in cases {
        assert_eq!(err.code(), code, "{err:?}");
        assert_eq!(err.is_fatal(), fatal, "{err:?}");
        assert!(!err.to_string().is_empty());
    }
    assert_eq!(
        Error::Remote {
            code: ErrorCode::Busy,
            message: "disk has a writer".into()
        }
        .to_string(),
        "busy: disk has a writer"
    );
    assert_eq!(Error::Unexpected(ty::OK).to_string(), "unexpected message Ok");
}

#[test]
fn send_error_forwards_remote_messages_verbatim() {
    let mut conn = Connection::new(Duplex::new(Vec::new()));
    conn.send_error(&Error::Remote {
        code: ErrorCode::OverBudget,
        message: "RAM budget is 256 MiB".into(),
    })
    .unwrap();
    conn.send_error(&Error::Truncated).unwrap();
    let out = common::decode_all(&conn.get_ref().output);
    assert_eq!(
        out,
        [
            Message::Error {
                code: ErrorCode::OverBudget,
                message: "RAM budget is 256 MiB".into()
            },
            Message::Error {
                code: ErrorCode::Malformed,
                message: "message is truncated".into()
            },
        ]
    );
}
