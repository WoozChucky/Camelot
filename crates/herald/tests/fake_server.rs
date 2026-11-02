//! The streaming layer against a scripted fake server, for what a real Avalon does
//! not do on demand: events queued while attaching, events for other VMs and events
//! that do not decode, a server that never confirms a detach, one that answers out of
//! turn, and one that goes away.

mod common;

use avalon_proto::{AttachKind, Error, Frame, Message, VmState};
use common::*;
use herald::{End, DETACH_KEY};
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
