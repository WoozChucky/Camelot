//! The streaming layer against a scripted fake server, for what a real Avalon does
//! not do on demand: events queued while attaching, events for other VMs, events
//! that do not decode and frames of unknown types, a server that never confirms a
//! detach, one whose answer does not decode, one that sends a frame out of place or
//! hangs up while a detach is due, input while detaching, a server that answers out
//! of turn, and one that goes away.

mod common;

use avalon_proto::{AttachKind, Error, Frame, Message, VmState};
use common::*;
use herald::{End, DETACH_KEY};
use std::io::ErrorKind;
use std::time::{Duration, Instant};

const VM: u32 = 7;

fn output(data: &[u8]) -> Message {
    Message::ConsoleOutput {
        vm: VM,
        data: data.to_vec(),
    }
}

/// Everything for this VM is shown in order: the scrollback, the output that came
/// while attaching, then the live output. Other VMs' output and broken events are
/// passed over. The server hanging up is an error.
#[test]
fn output_in_order_and_a_lost_connection() {
    let (addr, server) = fake_server(|mut conn| {
        fake_attach(&mut conn, VM, VmState::Running, b"old ", &[output(b"queued ")]);
        conn.send(&Message::ConsoleOutput {
            vm: VM + 1,
            data: b"other VM ".to_vec(),
        })
        .unwrap();
        // A ConsoleOutput whose payload stops short, and an event of the screen.
        conn.send_frame(&Frame {
            ty: 0x0204,
            payload: vec![VM as u8],
        })
        .unwrap();
        conn.send(&Message::Detached {
            vm: VM,
            kind: AttachKind::Screen,
        })
        .unwrap();
        // A frame of a type Herald does not know, with no answer due: passed over.
        conn.send_frame(&Frame {
            ty: 0x7777,
            payload: vec![1, 2, 3],
        })
        .unwrap();
        conn.send(&output(b"live")).unwrap();
        // Input reaches the server as ConsoleInput for this VM.
        match conn.recv().unwrap() {
            Message::ConsoleInput { vm: VM, data } => assert_eq!(data, b"key"),
            other => panic!("{other:?}"),
        }
        // Then the server goes away.
    });
    let mut herald = Herald::attach_at(&addr, avalon_proto::VmRef::Id(VM), |_| {});
    assert_eq!(herald.vm.name, "fake");
    herald.wait_for("live");
    herald.type_keys(b"key");
    server.join().unwrap();
    // A clean close, or a reset where the host does that.
    match herald.end() {
        Err(e) => assert!(e.is_fatal(), "{e}"),
        Ok(end) => panic!("{end:?}"),
    }
    assert_eq!(herald.output.text(), "old queued live");
}

/// A server that never confirms the detach: the session ends anyway, after its
/// timeout.
#[test]
fn a_detach_without_an_answer_times_out() {
    let (addr, server) = fake_server(|mut conn| {
        fake_attach(&mut conn, VM, VmState::Running, b"", &[]);
        assert_eq!(conn.recv().unwrap(), Message::ConsoleDetach { vm: VM });
        // No answer; wait for the client to hang up.
        assert!(conn.recv().is_err());
    });
    let mut herald = Herald::attach_at(&addr, avalon_proto::VmRef::Id(VM), |s| {
        s.detach_timeout = Duration::from_millis(200)
    });
    let start = Instant::now();
    herald.type_keys(&[DETACH_KEY]);
    assert_eq!(herald.end().unwrap(), End::Detached);
    assert!(start.elapsed() >= Duration::from_millis(200));
    server.join().unwrap();
}

/// Output that arrives before the detach is confirmed is still shown; an `Error`
/// reply (the server had already let go) confirms it as well as `Ok`.
#[test]
fn output_before_the_detach_reply_is_shown() {
    let (addr, server) = fake_server(|mut conn| {
        fake_attach(&mut conn, VM, VmState::Paused, b"", &[]);
        assert_eq!(conn.recv().unwrap(), Message::ConsoleDetach { vm: VM });
        conn.send(&output(b"last words")).unwrap();
        conn.send(&Message::Error {
            code: avalon_proto::ErrorCode::InvalidState,
            message: "not attached".into(),
        })
        .unwrap();
        assert!(conn.recv().is_err());
    });
    let mut herald = Herald::attach_at(&addr, avalon_proto::VmRef::Id(VM), |_| {});
    assert_eq!(herald.vm.state, VmState::Paused);
    herald.close_input();
    assert_eq!(herald.end().unwrap(), End::InputEnded);
    assert_eq!(herald.output.text(), "last words");
    server.join().unwrap();
}

/// A reply-type frame that fails to decode is still the answer to the detach (the
/// protocol, section 4): the session ends then, not after its timeout.
#[test]
fn a_detach_reply_that_does_not_decode_still_answers() {
    let (addr, server) = fake_server(|mut conn| {
        fake_attach(&mut conn, VM, VmState::Running, b"", &[]);
        assert_eq!(conn.recv().unwrap(), Message::ConsoleDetach { vm: VM });
        // An Ok with a byte too many.
        conn.send_frame(&Frame {
            ty: Message::Ok.type_code(),
            payload: vec![0],
        })
        .unwrap();
        assert!(conn.recv().is_err());
    });
    let mut herald = Herald::attach_at(&addr, avalon_proto::VmRef::Id(VM), |s| {
        // Far beyond `end`'s own wait, so only the reply can end the session in time.
        s.detach_timeout = Duration::from_secs(3600)
    });
    herald.type_keys(&[DETACH_KEY]);
    assert_eq!(herald.end().unwrap(), End::Detached);
    server.join().unwrap();
}

/// While the detach waits for its answer, a frame that is neither an event nor a
/// reply (a request, or a type not in the catalogue) cannot be placed: the session
/// ends with `OutOfStep`, rather than take a later reply for the answer.
#[test]
fn a_frame_out_of_place_while_detaching_is_out_of_step() {
    let request = Frame::from_message(&Message::GetVmInfo {
        vm: avalon_proto::VmRef::Id(VM),
    })
    .unwrap();
    let unknown = Frame {
        ty: 0x7777,
        payload: Vec::new(),
    };
    for frame in [request, unknown] {
        let (addr, server) = fake_server(move |mut conn| {
            fake_attach(&mut conn, VM, VmState::Running, b"", &[]);
            assert_eq!(conn.recv().unwrap(), Message::ConsoleDetach { vm: VM });
            conn.send_frame(&frame).unwrap();
            // Herald may have hung up already.
            let _ = conn.send(&Message::Ok);
            let _ = conn.recv();
        });
        let mut herald = Herald::attach_at(&addr, avalon_proto::VmRef::Id(VM), |s| {
            s.detach_timeout = Duration::from_secs(3600)
        });
        herald.type_keys(&[DETACH_KEY]);
        match herald.end() {
            Err(Error::OutOfStep(_)) => {}
            other => panic!("{other:?}"),
        }
        server.join().unwrap();
    }
}

/// Keys typed, and the end of the input, while the detach waits for its answer go
/// nowhere: no ConsoleInput, no second ConsoleDetach.
#[test]
fn input_while_detaching_is_dropped() {
    let (typed_tx, typed) = std::sync::mpsc::channel::<()>();
    let (detach_tx, detach_seen) = std::sync::mpsc::channel::<()>();
    let (addr, server) = fake_server(move |mut conn| {
        fake_attach(&mut conn, VM, VmState::Running, b"", &[]);
        assert_eq!(conn.recv().unwrap(), Message::ConsoleDetach { vm: VM });
        detach_tx.send(()).unwrap();
        typed.recv_timeout(SLOW).unwrap();
        // Herald would send at once what it was going to send.
        conn.get_ref()
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        match conn.recv() {
            Err(Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            other => panic!("sent while detaching: {other:?}"),
        }
        conn.get_ref().set_read_timeout(Some(SLOW)).unwrap();
        conn.send(&Message::Ok).unwrap();
        assert!(conn.recv().is_err());
    });
    let mut herald = Herald::attach_at(&addr, avalon_proto::VmRef::Id(VM), |s| {
        s.detach_timeout = Duration::from_secs(3600)
    });
    herald.type_keys(&[DETACH_KEY]);
    detach_seen.recv_timeout(SLOW).unwrap();
    herald.type_keys(b"more");
    herald.close_input();
    typed_tx.send(()).unwrap();
    assert_eq!(herald.end().unwrap(), End::Detached);
    server.join().unwrap();
}

/// A server that hangs up on the detach instead of answering it: the detach is done
/// all the same, not an error.
#[test]
fn a_hang_up_on_the_detach_is_a_detach() {
    let (addr, server) = fake_server(|mut conn| {
        fake_attach(&mut conn, VM, VmState::Running, b"", &[]);
        assert_eq!(conn.recv().unwrap(), Message::ConsoleDetach { vm: VM });
        // Dropping the connection closes it.
    });
    let mut herald = Herald::attach_at(&addr, avalon_proto::VmRef::Id(VM), |s| {
        s.detach_timeout = Duration::from_secs(3600)
    });
    herald.type_keys(&[DETACH_KEY]);
    assert_eq!(herald.end().unwrap(), End::Detached);
    server.join().unwrap();
}

/// A reply nobody asked for means the server and Herald are out of step: an error.
#[test]
fn a_reply_out_of_turn_is_an_error() {
    let (addr, server) = fake_server(|mut conn| {
        fake_attach(&mut conn, VM, VmState::Running, b"", &[]);
        conn.send(&Message::Ok).unwrap();
        let _ = conn.recv();
    });
    let mut herald = Herald::attach_at(&addr, avalon_proto::VmRef::Id(VM), |_| {});
    match herald.end() {
        Err(Error::Unexpected(ty)) => assert_eq!(ty, Message::Ok.type_code()),
        other => panic!("{other:?}"),
    }
    server.join().unwrap();
}

/// With no detach key every byte goes to the VM, the detach key's included; only the
/// end of the input detaches.
#[test]
fn no_detach_key() {
    let (addr, server) = fake_server(|mut conn| {
        fake_attach(&mut conn, VM, VmState::Running, b"", &[]);
        let mut got = Vec::new();
        loop {
            match conn.recv().unwrap() {
                Message::ConsoleInput { vm: VM, data } => got.extend(data),
                Message::ConsoleDetach { vm: VM } => break,
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(got, [b'a', DETACH_KEY, b'b']);
        conn.send(&Message::Ok).unwrap();
    });
    let mut herald = Herald::attach_at(&addr, avalon_proto::VmRef::Id(VM), |s| s.detach_key = None);
    herald.type_keys(&[b'a', DETACH_KEY, b'b']);
    herald.close_input();
    assert_eq!(herald.end().unwrap(), End::InputEnded);
    server.join().unwrap();
}

/// A VM that ends while attaching (its state change queued with the replies) ends
/// the session once the output before it is shown.
#[test]
fn a_vm_that_exits_while_attaching() {
    let (addr, server) = fake_server(|mut conn| {
        let exited = Message::VmStateChanged {
            vm: VM,
            state: VmState::Exited(3),
        };
        fake_attach(&mut conn, VM, VmState::Exited(3), b"a", &[output(b"b"), exited]);
        let _ = conn.recv();
    });
    let mut herald = Herald::attach_at(&addr, avalon_proto::VmRef::Id(VM), |_| {});
    assert_eq!(herald.end().unwrap(), End::VmEnded(VmState::Exited(3)));
    assert_eq!(herald.output.text(), "ab");
    server.join().unwrap();
}

/// The server destroying the VM ends the session.
#[test]
fn destroyed() {
    let (addr, server) = fake_server(|mut conn| {
        fake_attach(&mut conn, VM, VmState::Running, b"", &[]);
        conn.send(&Message::Detached {
            vm: VM,
            kind: AttachKind::Console,
        })
        .unwrap();
        let _ = conn.recv();
    });
    let mut herald = Herald::attach_at(&addr, avalon_proto::VmRef::Id(VM), |_| {});
    assert_eq!(herald.end().unwrap(), End::Destroyed);
    server.join().unwrap();
}

/// Detaching while the server's frames are still coming (#234): closing then, with
/// them unread, would reset the connection, and the server could lose what Herald sent
/// last. Herald closes in order instead: the server reads everything Herald sent and
/// then a clean end, and its own sending never fails.
#[test]
fn detaching_while_the_server_sends_closes_in_order() {
    for _ in 0..10 {
        let (addr, server) = fake_server(|mut conn| {
            fake_attach(&mut conn, VM, VmState::Running, b"", &[]);
            // Another VM's output: Herald passes it over.
            let flood = Message::ConsoleOutput {
                vm: VM + 1,
                data: vec![b'.'; 1024],
            };
            let (got, end, flooding) =
                flood_until_the_client_closes(conn, flood, Message::ConsoleDetach { vm: VM });
            let (detach, input) = got.split_last().expect("nothing came");
            assert_eq!(*detach, Message::ConsoleDetach { vm: VM });
            let mut typed = Vec::new();
            for msg in input {
                match msg {
                    Message::ConsoleInput { vm: VM, data } => typed.extend_from_slice(data),
                    other => panic!("{other:?}"),
                }
            }
            assert_eq!(typed, b"key");
            assert!(matches!(end, Error::Closed), "a clean end, not {end}");
            flooding.expect("the server's sending ends cleanly too");
        });
        let mut herald = Herald::attach_at(&addr, avalon_proto::VmRef::Id(VM), |_| {});
        herald.type_keys(b"key");
        herald.type_keys(&[DETACH_KEY]);
        assert_eq!(herald.end().unwrap(), End::Detached);
        server.join().unwrap();
    }
}
