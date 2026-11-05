//! A VM's GDB stub over the Avalon connection (protocol doc, section 6.8): a debugger
//! attaches, the VM stops for it (state debug), and Grail's stub answers the GDB remote
//! serial protocol carried in `DebugInput` and `DebugOutput`. Breakpoints, steps,
//! interrupts, detaching, the end of the machine, and the requests' errors; the console
//! goes on as it would without a debugger.

mod common;

use avalon_proto::{AttachKind, Connection, ErrorCode, Message, VmRef, VmState};
use common::*;
use grail::gdb::packet::{self, Decoder, Event};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// A debugger on its own connection: what it sends goes out as `DebugInput`, and the
/// `DebugOutput` it gets is decoded as a GDB client decodes a stub's bytes.
struct Gdb {
    conn: Connection<TcpStream>,
    vm: u32,
    decoder: Decoder,
    /// Packets and acks decoded, in order.
    events: Vec<Event>,
    /// The other messages the server sent (state changes, `Detached`).
    messages: Vec<Message>,
}

impl Gdb {
    fn attach(server: &avalon::ServerThread, vm: VmRef) -> Gdb {
        let mut conn = connect(server);
        let vm = match conn.request(&Message::DebugAttach { vm }) {
            Ok(Message::DebugAttached { vm }) => vm,
            other => panic!("attach: {other:?}"),
        };
        let mut gdb = Gdb {
            conn,
            vm,
            decoder: Decoder::default(),
            events: Vec::new(),
            messages: Vec::new(),
        };
        // The state change to debug came before the reply, set aside.
        while gdb.conn.queued_events() > 0 {
            let msg = gdb.conn.recv().unwrap();
            gdb.take(msg);
        }
        gdb
    }

    fn raw(&mut self, data: &[u8]) {
        let msg = Message::DebugInput {
            vm: self.vm,
            data: data.to_vec(),
        };
        self.conn.send(&msg).unwrap();
    }

    fn send(&mut self, payload: &str) {
        self.raw(&packet::frame(payload.as_bytes()));
    }

    fn take(&mut self, msg: Message) {
        match msg {
            Message::DebugOutput { vm, data } => {
                assert_eq!(vm, self.vm);
                for b in data {
                    if let Some(e) = self.decoder.push(b) {
                        self.events.push(e);
                    }
                }
            }
            other => self.messages.push(other),
        }
    }

    /// The next packet the stub sends (acks are kept in `events`).
    fn reply(&mut self) -> String {
        let start = Instant::now();
        loop {
            if let Some(i) = self.events.iter().position(|e| matches!(e, Event::Packet(_))) {
                let Event::Packet(p) = self.events.remove(i) else {
                    unreachable!()
                };
                return String::from_utf8(p).unwrap();
            }
            assert!(start.elapsed() < SLOW, "no reply; messages {:?}", self.messages);
            let msg = self.conn.recv().unwrap();
            self.take(msg);
        }
    }

    fn ask(&mut self, payload: &str) -> String {
        self.send(payload);
        self.reply()
    }

    /// Waits for `msg` among the other messages.
    fn wait_for(&mut self, msg: &Message) {
        let start = Instant::now();
        while !self.messages.contains(msg) {
            assert!(start.elapsed() < SLOW, "no {msg:?}; messages {:?}", self.messages);
            let msg = self.conn.recv().unwrap();
            self.take(msg);
        }
    }

    fn acks(&self) -> usize {
        self.events.iter().filter(|e| **e == Event::Ack).count()
    }
}

/// The pc in a stop reply (`...;20:PC;`), little-endian hex of 4 bytes on RV32.
fn stop_pc(reply: &str) -> u64 {
    let hex = reply.split("20:").nth(1).unwrap().split(';').next().unwrap();
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    bytes.iter().rev().fold(0, |v, &b| v << 8 | u64::from(b))
}

fn changed(vm: u32, state: VmState) -> Message {
    Message::VmStateChanged { vm, state }
}

#[test]
fn a_debugger_stops_the_vm_breaks_steps_and_detaches() {
    let server = server(None);
    let mut control = connect(&server);
    let id = create(&mut control, "spin", profile(4, None), spinner());
    control
        .request_ok(&Message::StartVm { vm: VmRef::Id(id) })
        .unwrap();
    let mut console = Console::attach(&server, VmRef::Id(id));
    console.wait_for("L");

    let mut gdb = Gdb::attach(&server, VmRef::Name("spin".into()));
    assert_eq!(gdb.vm, id);
    assert_eq!(state(&mut control, id), VmState::Debug);
    assert!(server.debug_vm(id).unwrap().debugged);
    console.wait_for_event(changed(id, VmState::Debug));
    // The debugger hears of it too, before its reply.
    assert!(
        gdb.messages.contains(&changed(id, VmState::Debug)),
        "{:?}",
        gdb.messages
    );
    // Stopped, the machine runs no step.
    let steps = info(&mut control, id).steps;
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(info(&mut control, id).steps, steps);

    // The stub is Grail's: acks until no-ack mode, a new client's stop is an interrupt.
    let stop = gdb.ask("?");
    assert!(stop.starts_with("T02thread:1;20:"), "{stop}");
    assert_eq!(gdb.acks(), 1, "the stub acks the packet");
    assert_eq!(gdb.ask("QStartNoAckMode"), "OK");
    // The loop of the spinner, `addi t2, t2, 1`, is the fourth instruction.
    let at = 0x8000_000cu64;
    assert_eq!(gdb.ask(&format!("Z0,{at:x},4")), "OK");
    gdb.send("vCont;c");
    let hit = gdb.reply();
    assert!(hit.starts_with("T05"), "{hit}");
    assert_eq!(stop_pc(&hit), at);
    // Resumed, then stopped again: two state changes.
    gdb.wait_for(&changed(id, VmState::Running));
    console.wait_for_event(changed(id, VmState::Running));
    assert_eq!(state(&mut control, id), VmState::Debug);
    // A step changes no state.
    let stepped = gdb.ask("vCont;s:1");
    assert_eq!(stop_pc(&stepped), at + 4, "{stepped}");
    assert_eq!(state(&mut control, id), VmState::Debug);
    // The VM is the debugger's: PauseVm and ResumeVm do not apply.
    assert_eq!(
        error_of(&mut control, &Message::PauseVm { vm: VmRef::Id(id) }),
        ErrorCode::InvalidState
    );
    assert_eq!(
        error_of(&mut control, &Message::ResumeVm { vm: VmRef::Id(id) }),
        ErrorCode::InvalidState
    );

    // Running for the debugger: paused and resumed by others, stopped by its Ctrl-C.
    assert_eq!(gdb.ask(&format!("z0,{at:x},4")), "OK");
    gdb.send("c");
    wait_for_state(&mut control, id, |s| s == VmState::Running);
    control
        .request_ok(&Message::PauseVm { vm: VmRef::Id(id) })
        .unwrap();
    control
        .request_ok(&Message::ResumeVm { vm: VmRef::Id(id) })
        .unwrap();
    let ran = info(&mut control, id).steps;
    wait_until("the VM to run", SLOW, || info(&mut control, id).steps > ran);
    gdb.raw(&[0x03]);
    let interrupted = gdb.reply();
    assert!(interrupted.starts_with("T02"), "{interrupted}");
    wait_for_state(&mut control, id, |s| s == VmState::Debug);

    // `D`: the VM runs on, without the debugger's breakpoints.
    assert_eq!(gdb.ask(&format!("Z0,{at:x},4")), "OK");
    assert_eq!(gdb.ask("D"), "OK");
    wait_for_state(&mut control, id, |s| s == VmState::Running);
    assert!(!server.debug_vm(id).unwrap().debugged);
    let ran = info(&mut control, id).steps;
    wait_until("the VM to run past the breakpoint", SLOW, || {
        info(&mut control, id).steps > ran + 1_000_000
    });
    assert_eq!(state(&mut control, id), VmState::Running);

    // A new debugger, whose connection ends: the VM runs on.
    let gdb = Gdb::attach(&server, VmRef::Id(id));
    assert_eq!(state(&mut control, id), VmState::Debug);
    drop(gdb);
    wait_for_state(&mut control, id, |s| s == VmState::Running);
    wait_until("the debugger to go", SLOW, || {
        !server.debug_vm(id).unwrap().debugged
    });
    // The console was never detached.
    assert_eq!(console.output, b"L");
    server.stop().unwrap();
}

#[test]
fn a_created_vm_waits_for_its_debugger_and_ends_under_it() {
    let server = server(None);
    let mut control = connect(&server);
    let id = create(&mut control, "bye", profile(4, None), exits_with(3));
    let mut console = Console::attach(&server, VmRef::Id(id));
    let mut gdb = Gdb::attach(&server, VmRef::Id(id));
    assert_eq!(state(&mut control, id), VmState::Debug);
    // Before its first instruction, in the boot ROM, as `grail --gdb-wait` waits.
    assert_eq!(gdb.ask("p20"), "00100000");
    assert_eq!(info(&mut control, id).steps, 0);
    assert_eq!(gdb.ask("c"), "W03");
    gdb.wait_for(&Message::Detached {
        vm: id,
        kind: AttachKind::Debug,
    });
    assert_eq!(state(&mut control, id), VmState::Exited(3));
    console.wait_for("bye");
    assert!(!server.debug_vm(id).unwrap().debugged);
    // An ended VM has nothing to debug.
    let mut other = connect(&server);
    assert_eq!(
        error_of(&mut other, &Message::DebugAttach { vm: VmRef::Id(id) }),
        ErrorCode::InvalidState
    );

    // Stepping to the exit ends the session the same way.
    let id = create(&mut control, "step", profile(4, None), exits_with(4));
    let mut gdb = Gdb::attach(&server, VmRef::Id(id));
    // The ROM's few instructions, then the program's eight.
    let mut last = String::new();
    for _ in 0..100 {
        last = gdb.ask("s");
        if last.starts_with('W') {
            break;
        }
    }
    assert_eq!(last, "W04");
    gdb.wait_for(&Message::Detached {
        vm: id,
        kind: AttachKind::Debug,
    });
    assert_eq!(state(&mut control, id), VmState::Exited(4));
    server.stop().unwrap();
}

#[test]
fn stopping_or_killing_a_debugged_vm_ends_the_attachment() {
    let server = server(None);
    let mut control = connect(&server);

    // StopVm while the debugger waits for a stop reply: `X09`, then `Detached`.
    let id = create(&mut control, "a", profile(4, None), spinner());
    let mut gdb = Gdb::attach(&server, VmRef::Id(id));
    gdb.send("c");
    wait_for_state(&mut control, id, |s| s == VmState::Running);
    control
        .request_ok(&Message::StopVm { vm: VmRef::Id(id) })
        .unwrap();
    assert_eq!(gdb.reply(), "X09");
    gdb.wait_for(&Message::Detached {
        vm: id,
        kind: AttachKind::Debug,
    });
    assert_eq!(state(&mut control, id), VmState::Stopped);

    // StopVm of a VM stopped for its debugger: no stop reply is owed.
    let id = create(&mut control, "b", profile(4, None), spinner());
    let mut gdb = Gdb::attach(&server, VmRef::Id(id));
    control
        .request_ok(&Message::StopVm { vm: VmRef::Id(id) })
        .unwrap();
    gdb.wait_for(&Message::Detached {
        vm: id,
        kind: AttachKind::Debug,
    });
    assert!(gdb.events.iter().all(|e| !matches!(e, Event::Packet(_))));
    assert_eq!(state(&mut control, id), VmState::Stopped);

    // `k` stops the VM as StopVm does.
    let id = create(&mut control, "c", profile(4, None), spinner());
    control
        .request_ok(&Message::StartVm { vm: VmRef::Id(id) })
        .unwrap();
    let mut gdb = Gdb::attach(&server, VmRef::Id(id));
    assert_eq!(gdb.ask("k"), "X09");
    gdb.wait_for(&Message::Detached {
        vm: id,
        kind: AttachKind::Debug,
    });
    assert_eq!(state(&mut control, id), VmState::Stopped);

    // DestroyVm: `Detached` too.
    let id = create(&mut control, "d", profile(4, None), spinner());
    let mut gdb = Gdb::attach(&server, VmRef::Id(id));
    control
        .request_ok(&Message::DestroyVm { vm: VmRef::Id(id) })
        .unwrap();
    gdb.wait_for(&Message::Detached {
        vm: id,
        kind: AttachKind::Debug,
    });
    server.stop().unwrap();
}

#[test]
fn one_debugger_at_a_time_and_only_its_input() {
    let server = server(None);
    let mut control = connect(&server);
    assert_eq!(
        error_of(&mut control, &Message::DebugAttach { vm: VmRef::Id(9) }),
        ErrorCode::NoSuchVm
    );
    let id = create(&mut control, "spin", profile(4, None), spinner());
    control
        .request_ok(&Message::StartVm { vm: VmRef::Id(id) })
        .unwrap();
    let mut gdb = Gdb::attach(&server, VmRef::Id(id));
    // A second debugger, on another connection or on the same one.
    assert_eq!(
        error_of(&mut control, &Message::DebugAttach { vm: VmRef::Id(id) }),
        ErrorCode::Busy
    );
    assert_eq!(
        error_of(&mut gdb.conn, &Message::DebugAttach { vm: VmRef::Id(id) }),
        ErrorCode::Busy
    );
    // Detaching what it does not debug.
    assert_eq!(
        error_of(&mut control, &Message::DebugDetach { vm: id }),
        ErrorCode::InvalidState
    );
    assert_eq!(
        error_of(&mut control, &Message::DebugDetach { vm: 77 }),
        ErrorCode::NoSuchVm
    );
    // Input from anyone else is dropped: this `k` would stop the VM.
    control
        .send(&Message::DebugInput {
            vm: id,
            data: packet::frame(b"k"),
        })
        .unwrap();
    assert_eq!(gdb.ask("qC"), "QC1");
    assert_eq!(state(&mut control, id), VmState::Debug);
    // A malformed packet gets a `-`, and the stream goes on.
    gdb.raw(b"$g#00");
    gdb.send("qAttached");
    assert_eq!(gdb.reply(), "1");
    assert!(gdb.events.contains(&Event::Nack), "{:?}", gdb.events);
    // DebugDetach is `D`: the VM runs on.
    gdb.conn.request_ok(&Message::DebugDetach { vm: id }).unwrap();
    assert_eq!(state(&mut control, id), VmState::Running);
    assert!(!server.debug_vm(id).unwrap().debugged);
    // And then another client may debug it.
    let other = Gdb::attach(&server, VmRef::Id(id));
    assert_eq!(state(&mut control, id), VmState::Debug);
    drop(other);
    server.stop().unwrap();
}
