//! Helpers for the server tests: an in-process Avalon on an ephemeral port, clients,
//! console readers, and guest programs.

#![allow(dead_code)]

use avalon::{Config, Server, ServerThread};
use avalon_proto::{Connection, HardwareProfile, Message, MessageKind, VmDetails, VmRef, VmState};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

pub const MIB: u64 = 1024 * 1024;

/// Generous ceiling for anything that should take well under a second, so a loaded CI
/// machine does not make the tests flaky.
pub const SLOW: Duration = Duration::from_secs(10);

/// A server on 127.0.0.1 and an ephemeral port, quiet, with a RAM budget.
pub fn server(max_ram: Option<u64>) -> ServerThread {
    let config = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        max_ram,
        log: false,
    };
    Server::bind(&config).unwrap().spawn().unwrap()
}

pub fn connect(server: &ServerThread) -> Connection<TcpStream> {
    let conn = Connection::connect(server.addr(), "test").unwrap();
    conn.get_ref().set_read_timeout(Some(SLOW)).unwrap();
    conn
}

pub fn example(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../examples/{name}/{name}.elf"));
    std::fs::read(path).unwrap()
}

pub fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

/// Assembles and links a guest with merlin-as and merlin-ld (`.text` at 0x80000000,
/// entry `_start`).
pub fn guest(asm: &str) -> Vec<u8> {
    let obj = merlin_as::assemble_to_elf(asm).unwrap_or_else(|e| panic!("{e:?}"));
    let inputs = [merlin_ld::Input {
        name: "guest.o".into(),
        data: obj,
    }];
    merlin_ld::link(&inputs, &merlin_ld::Options::default())
        .unwrap_or_else(|e| panic!("{e:?}"))
        .elf
}

/// Prints `L` to the UART, then counts in a loop forever: a guest that never stops
/// by itself and never waits.
pub fn spinner() -> Vec<u8> {
    guest(
        "
    .globl _start
_start:
    li    t0, 0x10000000
    li    t1, 76
    sb    t1, 0(t0)
1:  addi  t2, t2, 1
    j     1b
",
    )
}

/// Prints `bye` and halts with exit code `code`.
pub fn exits_with(code: u32) -> Vec<u8> {
    guest(&format!(
        "
    .globl _start
_start:
    li    t0, 0x10000000
    li    t1, 98
    sb    t1, 0(t0)
    li    t1, 121
    sb    t1, 0(t0)
    li    t1, 101
    sb    t1, 0(t0)
    li    a0, {code}
    xcal.halt a0
"
    ))
}

pub fn profile(ram_mib: u64, clock_hz: Option<u32>) -> HardwareProfile {
    HardwareProfile {
        clock_hz,
        ..HardwareProfile::with_ram(ram_mib * MIB)
    }
}

pub fn create(
    conn: &mut Connection<TcpStream>,
    name: &str,
    profile: HardwareProfile,
    program: Vec<u8>,
) -> u32 {
    match conn.request(&Message::CreateVm {
        name: name.into(),
        profile,
        program,
    }) {
        Ok(Message::VmCreated { id }) => id,
        other => panic!("create {name}: {other:?}"),
    }
}

pub fn info(conn: &mut Connection<TcpStream>, vm: u32) -> VmDetails {
    match conn.request(&Message::GetVmInfo { vm: VmRef::Id(vm) }) {
        Ok(Message::VmInfo(details)) => details,
        other => panic!("info {vm}: {other:?}"),
    }
}

pub fn state(conn: &mut Connection<TcpStream>, vm: u32) -> VmState {
    info(conn, vm).vm.state
}

/// Polls the VM's state until `done` says so.
pub fn wait_for_state(conn: &mut Connection<TcpStream>, vm: u32, done: impl Fn(VmState) -> bool) -> VmState {
    let start = Instant::now();
    loop {
        let s = state(conn, vm);
        if done(s) {
            return s;
        }
        assert!(start.elapsed() < SLOW, "VM {vm} still {s}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The error code of a request that must fail.
pub fn error_of(conn: &mut Connection<TcpStream>, msg: &Message) -> avalon_proto::ErrorCode {
    match conn.request(msg) {
        Err(avalon_proto::Error::Remote { code, .. }) => code,
        other => panic!("{msg:?} should fail, got {other:?}"),
    }
}

/// A connection attached to a VM's console: a thread reads everything the server
/// sends on it, so the test can wait for output with a timeout.
pub struct Console {
    /// For sending input (and, before attaching, requests).
    pub conn: Connection<TcpStream>,
    pub vm: u32,
    pub scrollback: Vec<u8>,
    /// Console output received so far (after the scrollback).
    pub output: Vec<u8>,
    /// Other events, in order.
    pub events: Vec<Message>,
    rx: Receiver<Message>,
}

impl Console {
    pub fn attach(server: &ServerThread, vm: VmRef) -> Console {
        let mut conn = connect(server);
        let (vm, scrollback) = match conn.request(&Message::ConsoleAttach { vm }) {
            Ok(Message::ConsoleAttached { vm, scrollback }) => (vm, scrollback),
            other => panic!("attach: {other:?}"),
        };
        assert_eq!(conn.queued_events(), 0);
        let mut reader = conn.try_clone().unwrap();
        reader.get_ref().set_read_timeout(None).unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(msg) = reader.recv() {
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });
        Console {
            conn,
            vm,
            scrollback,
            output: Vec::new(),
            events: Vec::new(),
            rx,
        }
    }

    pub fn send(&mut self, data: &[u8]) {
        self.conn
            .send(&Message::ConsoleInput {
                vm: self.vm,
                data: data.to_vec(),
            })
            .unwrap();
    }

    /// Takes in what has arrived, waiting at most `timeout` for the next message.
    /// Returns false once the connection has ended.
    fn pump(&mut self, timeout: Duration) -> bool {
        match self.rx.recv_timeout(timeout) {
            Ok(Message::ConsoleOutput { vm, data }) => {
                assert_eq!(vm, self.vm);
                self.output.extend(data);
                true
            }
            Ok(other) => {
                assert_ne!(
                    other.kind(),
                    MessageKind::Reply,
                    "a reply nobody waits for: {other:?}"
                );
                self.events.push(other);
                true
            }
            Err(RecvTimeoutError::Timeout) => true,
            Err(RecvTimeoutError::Disconnected) => false,
        }
    }

    /// Sends a request on this connection and returns its reply (the reading thread
    /// gets it), keeping the events that come first.
    pub fn request(&mut self, msg: &Message) -> Message {
        self.conn.send(msg).unwrap();
        let start = Instant::now();
        loop {
            assert!(start.elapsed() < SLOW, "no reply to {msg:?}");
            match self.rx.recv_timeout(Duration::from_millis(20)) {
                Ok(reply) if reply.kind() == MessageKind::Reply => return reply,
                Ok(Message::ConsoleOutput { data, .. }) => self.output.extend(data),
                Ok(event) => self.events.push(event),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => panic!("connection closed waiting for a reply"),
            }
        }
    }

    /// Waits until the output so far contains `needle`.
    pub fn wait_for(&mut self, needle: &str) -> String {
        self.wait_until(|c| String::from_utf8_lossy(&c.output).contains(needle), needle)
    }

    /// Waits until `done` holds for this console.
    pub fn wait_until(&mut self, done: impl Fn(&Console) -> bool, what: &str) -> String {
        let start = Instant::now();
        while !done(self) {
            assert!(
                start.elapsed() < SLOW,
                "waiting for {what}; output so far {:?}, events {:?}",
                String::from_utf8_lossy(&self.output),
                self.events
            );
            if !self.pump(Duration::from_millis(20)) && !done(self) {
                panic!(
                    "connection ended waiting for {what}; output {:?}, events {:?}",
                    String::from_utf8_lossy(&self.output),
                    self.events
                );
            }
        }
        String::from_utf8_lossy(&self.output).into_owned()
    }

    /// Waits for a state change event to `state`.
    pub fn wait_for_event(&mut self, event: Message) {
        let what = format!("{event:?}");
        self.wait_until(|c| c.events.contains(&event), &what);
    }

    /// Collects whatever arrives during `time`.
    pub fn idle(&mut self, time: Duration) {
        let end = Instant::now() + time;
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            if !self.pump(left.min(Duration::from_millis(20))) {
                break;
            }
        }
    }

    /// Waits for the server to end the connection.
    pub fn wait_closed(&mut self) {
        let start = Instant::now();
        while self.pump(Duration::from_millis(20)) {
            assert!(start.elapsed() < SLOW, "the connection is still open");
        }
    }
}

impl Drop for Console {
    /// Closes the connection (the reading thread holds a second handle on it).
    fn drop(&mut self) {
        let _ = self.conn.get_ref().shutdown(std::net::Shutdown::Both);
    }
}

/// What `examples/sysinfo` prints on a machine with this RAM and clock and no disks
/// (as in grail's tests/machine.rs).
pub fn sysinfo_output(ram_mib: u32, hz: u32, real_time: bool) -> String {
    let kind = if real_time { "real time" } else { "nominal" };
    format!(
        "sysinfo v1
RAM      {ram_mib} MiB at 0x80000000
CPU      {hz} Hz ({kind})
timebase {hz} Hz
8 devices:
  test      0x00100000 + 0x00001000
  CLINT     0x02000000 + 0x00010000
  PLIC      0x0c000000 + 0x04000000
  UART      0x10000000 + 0x00000100  irq 1
  fb-ctrl   0x30000000 + 0x00001000
  keyboard  0x30001000 + 0x00001000  irq 2
  sysinfo   0x30002000 + 0x00001000
  fb-vram   0x31000000 + 0x0012c000
"
    )
}
