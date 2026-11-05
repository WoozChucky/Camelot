//! The streaming layer against an in-process Avalon server, with no terminal: keys go
//! in through a channel and the console comes out into a buffer. Guests that echo
//! their UART input (`examples/uart-echo`, the Camelot kernel's shell) are attached
//! to, typed at, detached from and attached to again; several sessions share a VM;
//! the VM pausing, exiting and being destroyed reach the session.

mod common;

use avalon_proto::{ErrorCode, Message, VmRef, VmState};
use common::*;
use herald::{End, DETACH_KEY};
use std::time::Duration;

/// Attach, type at a guest that echoes, detach (the VM carries on), attach again and
/// find the session in the scrollback.
#[test]
fn attach_type_detach_and_reattach() {
    on_both_widths(|| {
        let server = server();
        let id = start_vm(&server, "echo", 1, example("uart-echo"));

        let mut first = Herald::attach(&server, VmRef::Name("echo".into()));
        assert_eq!((first.vm.id, first.vm.name.as_str()), (id, "echo"));
        assert_eq!(first.vm.state, VmState::Running);
        assert!(first.scrollback.is_empty());
        assert_eq!(consoles(&server, id), 1);
        first.type_keys(b"hello\r\n");
        first.wait_for("hello\r\n");

        // The detach key ends the session; what follows it never reaches the VM.
        first.type_keys(&[DETACH_KEY]);
        first.type_keys(b"lost");
        assert_eq!(first.end().unwrap(), End::Detached);
        assert_eq!(first.output.text(), "hello\r\n");
        wait_until("the server to let go of the console", || {
            consoles(&server, id) == 0
        });
        assert_eq!(state(&server, id), VmState::Running);

        // Attached again (by id this time): the scrollback has the earlier session.
        let mut second = Herald::attach(&server, VmRef::Id(id));
        assert_eq!(second.scrollback, b"hello\r\n");
        second.type_keys(b"again");
        // The session writes the scrollback first, then the live output.
        assert_eq!(second.wait_for("again"), "hello\r\nagain");
        second.close_input();
        assert_eq!(second.end().unwrap(), End::InputEnded);
        assert_eq!(state(&server, id), VmState::Running);
        assert!(!second.output.text().contains("lost"));
        server.stop().unwrap();
    });
}

/// Bytes typed before the detach key in the same burst still reach the VM.
#[test]
fn keys_before_the_detach_key_are_sent() {
    on_both_widths(|| {
        let server = server();
        let id = start_vm(&server, "echo", 1, example("uart-echo"));
        let mut herald = Herald::attach(&server, VmRef::Id(id));
        let mut keys = b"ab".to_vec();
        keys.push(DETACH_KEY);
        keys.extend(b"cd");
        herald.type_keys(&keys);
        assert_eq!(herald.end().unwrap(), End::Detached);

        // The VM may echo "ab" before or after the next attach: in the scrollback, or
        // live. Either way it comes before what is typed next, and "cd" never does.
        let mut again = Herald::attach(&server, VmRef::Id(id));
        again.type_keys(b"!");
        assert_eq!(again.wait_for("!"), "ab!");
        again.type_keys(&[DETACH_KEY]);
        assert_eq!(again.end().unwrap(), End::Detached);
        server.stop().unwrap();
    });
}

/// The Camelot kernel's shell, through Herald: a command, a detach, the scrollback on
/// the way back, and `exit` ending the VM and the session with it.
#[test]
fn the_kernel_shell() {
    on_both_widths(|| {
        let server = server();
        let id = start_vm(&server, "vm001", 4, kernel());
        let mut herald = Herald::attach(&server, VmRef::Name("vm001".into()));
        herald.wait_for("Type help for the list of commands.\n$ ");
        herald.type_keys(b"ticks\r");
        herald.wait_for("ticks: ");
        herald.type_keys(&[DETACH_KEY]);
        assert_eq!(herald.end().unwrap(), End::Detached);

        let mut again = Herald::attach(&server, VmRef::Id(id));
        let scrollback = String::from_utf8_lossy(&again.scrollback).into_owned();
        assert!(scrollback.contains("$ ticks"), "{scrollback}");
        assert!(scrollback.contains("ticks: "), "{scrollback}");
        again.type_keys(b"help\r");
        again.wait_for("leave the shell");
        again.type_keys(b"exit\r");
        assert_eq!(again.end().unwrap(), End::VmEnded(VmState::Exited(0)));
        server.stop().unwrap();
    });
}

/// Several Heralds on one VM: every one sees all the output, and input from any of
/// them reaches the VM. One leaving does not disturb the others.
#[test]
fn several_heralds_share_a_console() {
    on_both_widths(|| {
        let server = server();
        let id = start_vm(&server, "shared", 1, example("uart-echo"));
        let mut heralds: Vec<Herald> = (0..3).map(|_| Herald::attach(&server, VmRef::Id(id))).collect();
        assert_eq!(consoles(&server, id), 3);
        heralds[0].type_keys(b"one ");
        for h in &heralds {
            h.wait_for("one ");
        }
        // Input from different connections is taken in the order it arrives.
        heralds[1].type_keys(b"two ");
        heralds[0].wait_for("two ");
        heralds[2].type_keys(b"three");
        for h in &heralds {
            let text = h.wait_for("three");
            assert_eq!(text, "one two three");
        }

        let mut gone = heralds.remove(1);
        gone.type_keys(&[DETACH_KEY]);
        assert_eq!(gone.end().unwrap(), End::Detached);
        heralds[1].type_keys(b"!");
        for h in &heralds {
            h.wait_for("three!");
        }
        assert_eq!(gone.output.text(), "one two three");

        // EOT halts uart-echo: every session ends with the VM.
        heralds[0].type_keys(&[4]);
        for h in &mut heralds {
            assert_eq!(h.end().unwrap(), End::VmEnded(VmState::Exited(0)));
        }
        server.stop().unwrap();
    });
}

/// Paused and resumed, the VM keeps the session; the session says so.
#[test]
fn pause_and_resume_are_noticed() {
    on_both_widths(|| {
        let server = server();
        let id = start_vm(&server, "echo", 1, example("uart-echo"));
        let mut herald = Herald::attach(&server, VmRef::Id(id));
        let mut rt = connect(&server);
        rt.request_ok(&Message::PauseVm { vm: VmRef::Id(id) }).unwrap();
        herald.type_keys(b"x");
        wait_until("the pause notice", || herald.notices.text().contains("paused"));
        rt.request_ok(&Message::ResumeVm { vm: VmRef::Id(id) }).unwrap();
        herald.wait_for("x");
        assert_eq!(
            herald.notices.text(),
            format!("herald: VM {id} is paused\nherald: VM {id} is running\n")
        );
        assert!(!herald.has_ended());
        herald.type_keys(&[DETACH_KEY]);
        assert_eq!(herald.end().unwrap(), End::Detached);
        server.stop().unwrap();
    });
}

/// A VM that has already exited: its scrollback, then the session ends by itself.
#[test]
fn attaching_to_an_exited_vm() {
    on_both_widths(|| {
        let server = server();
        let id = start_vm(&server, "done", 16, example("sysinfo"));
        wait_until("sysinfo to exit", || state(&server, id) == VmState::Exited(0));
        let mut herald = Herald::attach(&server, VmRef::Id(id));
        assert_eq!(herald.end().unwrap(), End::VmEnded(VmState::Exited(0)));
        let text = herald.output.text();
        // Version 2 of the sysinfo block on RV64 (spec section 9.5).
        let version = match xlen() {
            avalon_proto::Xlen::Rv32 => 1,
            avalon_proto::Xlen::Rv64 => 2,
        };
        assert!(
            text.starts_with(&format!("sysinfo v{version}\nRAM      16 MiB")),
            "{text}"
        );
        assert_eq!(herald.output.bytes(), herald.scrollback);
    });
}

/// Stopping and destroying the VM end the session.
#[test]
fn stopped_and_destroyed_vms_end_the_session() {
    on_both_widths(|| {
        let server = server();
        let mut rt = connect(&server);
        let id = start_vm(&server, "a", 1, example("uart-echo"));
        let mut herald = Herald::attach(&server, VmRef::Id(id));
        rt.request_ok(&Message::StopVm { vm: VmRef::Id(id) }).unwrap();
        assert_eq!(herald.end().unwrap(), End::VmEnded(VmState::Stopped));

        let id = start_vm(&server, "b", 1, example("uart-echo"));
        let mut herald = Herald::attach(&server, VmRef::Id(id));
        herald.type_keys(b"hi");
        herald.wait_for("hi");
        rt.request_ok(&Message::DestroyVm { vm: VmRef::Id(id) }).unwrap();
        // Destroying a running VM stops it first, which the session may hear about first.
        let end = herald.end().unwrap();
        assert!(
            matches!(end, End::Destroyed | End::VmEnded(VmState::Stopped)),
            "{end:?}"
        );
        server.stop().unwrap();
    });
}

/// Attaching to a VM that does not exist fails with the server's error.
#[test]
fn no_such_vm() {
    let server = server();
    for vm in [VmRef::Id(9), VmRef::Name("ghost".into())] {
        let conn = avalon_proto::Connection::connect(server.addr(), herald::AGENT).unwrap();
        match herald::Session::attach(conn, &vm) {
            Err(avalon_proto::Error::Remote {
                code: ErrorCode::NoSuchVm,
                ..
            }) => {}
            Err(e) => panic!("{vm}: {e}"),
            Ok(_) => panic!("{vm}: attached"),
        }
    }
    server.stop().unwrap();
}

/// A burst of input larger than one `ConsoleInput` arrives whole and in order.
#[test]
fn a_long_paste_arrives_in_order() {
    on_both_widths(|| {
        let server = server();
        let id = start_vm(&server, "paste", 1, example("uart-echo"));
        let mut herald = Herald::attach(&server, VmRef::Id(id));
        let paste: Vec<u8> = (0..10_000u32).map(|i| b'a' + (i % 26) as u8).collect();
        herald.type_keys(&paste);
        let start = std::time::Instant::now();
        while herald.output.bytes().len() < paste.len() {
            assert!(
                start.elapsed() < SLOW,
                "{} bytes echoed",
                herald.output.bytes().len()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(herald.output.bytes(), paste);
        herald.type_keys(&[DETACH_KEY]);
        assert_eq!(herald.end().unwrap(), End::Detached);
        server.stop().unwrap();
    });
}
