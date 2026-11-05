//! Helpers for the server tests: an in-process Avalon on an ephemeral port, clients,
//! console readers, screen viewers, and guest programs.

#![allow(dead_code)]

use avalon::{Config, Server, ServerThread};
use avalon_proto::{
    Connection, HardwareProfile, KeyEvent, Message, MessageKind, PointerChange, Rect, Screen, VmDetails,
    VmRef, VmState,
};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

pub const MIB: u64 = 1024 * 1024;

/// An address as the examples print one (`netlib.S`'s `print_hex`): 16 hex digits,
/// as a register holds it (`0x0000000010009000`).
pub fn addr_text(addr: u64) -> String {
    format!("{addr:#018x}")
}

/// Generous ceiling for anything that should take well under a second, so a loaded CI
/// machine does not make the tests flaky.
pub const SLOW: Duration = Duration::from_secs(10);

/// A server on 127.0.0.1 and an ephemeral port, quiet, with a RAM budget.
pub fn server(max_ram: Option<u64>) -> ServerThread {
    server_with(Config { max_ram, ..quiet() })
}

/// The configuration [`server`] starts from: 127.0.0.1, an ephemeral port, no log.
pub fn quiet() -> Config {
    Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        log: false,
        ..Config::default()
    }
}

pub fn server_with(config: Config) -> ServerThread {
    Server::bind(&config).unwrap().spawn().unwrap()
}

pub fn connect(server: &ServerThread) -> Connection<TcpStream> {
    let conn = Connection::connect(server.addr(), "test").unwrap();
    conn.get_ref().set_read_timeout(Some(SLOW)).unwrap();
    conn
}

/// The example `name` (`examples/<name>/<name>.elf`).
pub fn example(name: &str) -> Vec<u8> {
    example_in(name, name)
}

/// The program `name` of the example directory `dir` (`examples/net`'s `net-ping`):
/// `<name>.elf`, as `cargo xtask examples` builds it.
pub fn example_in(dir: &str, name: &str) -> Vec<u8> {
    std::fs::read(examples_dir().join(dir).join(format!("{name}.elf"))).unwrap()
}

pub fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

/// The frames of a pcap file, after its 24-byte header.
pub fn pcap_frames(bytes: &[u8]) -> Vec<Vec<u8>> {
    assert_eq!(bytes[..4], 0xa1b2_c3d4u32.to_le_bytes());
    let mut rest = &bytes[24..];
    let mut frames = Vec::new();
    while !rest.is_empty() {
        let len = u32::from_le_bytes(rest[8..12].try_into().unwrap()) as usize;
        frames.push(rest[16..16 + len].to_vec());
        rest = &rest[16 + len..];
    }
    frames
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

/// The smallest program: an ELF64 file with one `PT_LOAD` segment at the start of
/// RAM, its entry, holding `addi a0, zero, 7` and `j .`.
pub fn rv64_spinner() -> Vec<u8> {
    rv64_spinner_at(0x8000_0000)
}

/// [`rv64_spinner`] at 0x8020_0000, where a VM with firmware loads its program (the
/// firmware has the 2 MiB below it).
pub fn firmware_spinner() -> Vec<u8> {
    rv64_spinner_at(grail::firmware::FW_JUMP_ADDR)
}

/// [`rv64_spinner`] at `at`, its entry.
fn rv64_spinner_at(at: u64) -> Vec<u8> {
    let code: Vec<u8> = [0x0070_0513u32, 0x0000_006f]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    let mut f = vec![0u8; 64 + 56];
    f[..8].copy_from_slice(b"\x7fELF\x02\x01\x01\0");
    f[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    f[18..20].copy_from_slice(&243u16.to_le_bytes()); // EM_RISCV
    f[20..24].copy_from_slice(&1u32.to_le_bytes());
    f[24..32].copy_from_slice(&at.to_le_bytes()); // e_entry
    f[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
    f[52..54].copy_from_slice(&64u16.to_le_bytes()); // e_ehsize
    f[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
    f[56..58].copy_from_slice(&1u16.to_le_bytes()); // e_phnum
    f[58..60].copy_from_slice(&64u16.to_le_bytes()); // e_shentsize
    let ph = &mut f[64..];
    ph[..4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    ph[4..8].copy_from_slice(&5u32.to_le_bytes()); // R + X
    ph[8..16].copy_from_slice(&120u64.to_le_bytes()); // p_offset
    ph[16..24].copy_from_slice(&at.to_le_bytes()); // p_vaddr
    ph[24..32].copy_from_slice(&at.to_le_bytes()); // p_paddr
    ph[32..40].copy_from_slice(&(code.len() as u64).to_le_bytes()); // p_filesz
    ph[40..48].copy_from_slice(&(code.len() as u64).to_le_bytes()); // p_memsz
    ph[48..56].copy_from_slice(&4u64.to_le_bytes()); // p_align
    f.extend_from_slice(&code);
    f
}

/// A profile with `ram_mib` MiB and the clock `clock_hz`.
pub fn profile(ram_mib: u64, clock_hz: Option<u32>) -> HardwareProfile {
    HardwareProfile {
        clock_hz,
        ..hardware(ram_mib * MIB)
    }
}

/// The default profile with `ram_size` bytes of RAM.
pub fn hardware(ram_size: u64) -> HardwareProfile {
    HardwareProfile::with_ram(ram_size)
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

/// Slices VM `vm`'s thread has run (calls of Grail's `run_for`).
pub fn slices(server: &ServerThread, vm: u32) -> u64 {
    server.debug_vm(vm).expect("the VM exists").slices
}

/// Waits until `done` holds, polling.
pub fn wait_until(what: &str, limit: Duration, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < limit, "still waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
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

/// A connection attached to a VM's screen: a thread reads everything the server sends,
/// and the frames are applied to [`Viewer::screen`] as they come, as a viewer does.
pub struct Viewer {
    pub conn: Connection<TcpStream>,
    pub vm: u32,
    /// The screen as the frames received so far draw it.
    pub screen: Screen,
    /// Every frame's rectangle, in order.
    pub rects: Vec<Rect>,
    /// Each `ScreenResized`: how many frames had come before it, and the new size.
    pub resizes: Vec<(usize, u32, u32)>,
    /// The cursor as the `CursorShape` and `CursorPosition` received so far give it.
    pub cursor: avalon_proto::Cursor,
    /// Each of those messages: how many frames had come before it, and it.
    pub cursor_messages: Vec<(usize, Message)>,
    /// Other events, in order.
    pub events: Vec<Message>,
    rx: Receiver<Message>,
}

impl Viewer {
    /// Attaches to `vm`'s screen on a new connection, and waits for the full frame
    /// that follows the attach.
    pub fn attach(server: &ServerThread, vm: VmRef) -> Viewer {
        Viewer::attach_on(connect(server), vm)
    }

    pub fn attach_on(mut conn: Connection<TcpStream>, vm: VmRef) -> Viewer {
        let (vm, width, height) = match conn.request(&Message::ScreenAttach { vm }) {
            Ok(Message::ScreenAttached {
                vm,
                width,
                height,
                format,
            }) => {
                assert_eq!(format, avalon_proto::PIXEL_FORMAT_XRGB8888);
                (vm, width, height)
            }
            other => panic!("attach: {other:?}"),
        };
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
        let mut viewer = Viewer {
            conn,
            vm,
            screen: Screen::new(width, height),
            rects: Vec::new(),
            resizes: Vec::new(),
            cursor: avalon_proto::Cursor::default(),
            cursor_messages: Vec::new(),
            events: Vec::new(),
            rx,
        };
        viewer.wait_until(|v| !v.rects.is_empty(), "the first frame");
        assert_eq!(viewer.rects[0], viewer.screen.bounds(), "the first frame is full");
        viewer
    }

    pub fn key(&mut self, key: KeyEvent) {
        self.conn.send(&Message::KeyInput { vm: self.vm, key }).unwrap();
    }

    pub fn pointer(&mut self, pointer: PointerChange) {
        self.conn
            .send(&Message::PointerInput { vm: self.vm, pointer })
            .unwrap();
    }

    /// Takes in what has arrived, waiting at most `timeout` for the next message.
    /// Returns false once the connection has ended.
    pub fn pump(&mut self, timeout: Duration) -> bool {
        match self.rx.recv_timeout(timeout) {
            Ok(frame @ Message::ScreenFrame { .. }) => {
                if let Message::ScreenFrame { vm, .. } = frame {
                    assert_eq!(vm, self.vm);
                }
                let rect = self.screen.apply_frame(&frame).unwrap();
                self.rects.push(rect);
                true
            }
            // A new screen, black, of the new size; the frames that follow fit it.
            Ok(Message::ScreenResized {
                vm,
                width,
                height,
                format,
            }) => {
                assert_eq!((vm, format), (self.vm, avalon_proto::PIXEL_FORMAT_XRGB8888));
                self.screen = Screen::new(width, height);
                self.resizes.push((self.rects.len(), width, height));
                true
            }
            Ok(msg @ (Message::CursorShape { .. } | Message::CursorPosition { .. })) => {
                assert!(self.cursor.apply(&msg));
                self.cursor_messages.push((self.rects.len(), msg));
                true
            }
            Ok(other) => {
                self.events.push(other);
                true
            }
            Err(RecvTimeoutError::Timeout) => true,
            Err(RecvTimeoutError::Disconnected) => false,
        }
    }

    /// Waits until `done` holds for this viewer.
    pub fn wait_until(&mut self, done: impl Fn(&Viewer) -> bool, what: &str) {
        let start = Instant::now();
        while !done(self) {
            assert!(
                start.elapsed() < SLOW,
                "waiting for {what}; {} frames, events {:?}",
                self.rects.len(),
                self.events
            );
            if !self.pump(Duration::from_millis(20)) && !done(self) {
                panic!("connection ended waiting for {what}; events {:?}", self.events);
            }
        }
    }

    /// Waits for an event.
    pub fn wait_for_event(&mut self, event: Message) {
        let what = format!("{event:?}");
        self.wait_until(|v| v.events.contains(&event), &what);
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

    /// Sends a request on this connection and returns its reply, keeping what comes
    /// before it.
    pub fn request(&mut self, msg: &Message) -> Message {
        self.conn.send(msg).unwrap();
        let start = Instant::now();
        loop {
            assert!(start.elapsed() < SLOW, "no reply to {msg:?}");
            let before = self.events.len();
            assert!(self.pump(Duration::from_millis(20)), "connection closed");
            if self.events.len() > before && self.events[before].kind() == MessageKind::Reply {
                return self.events.remove(before);
            }
        }
    }
}

impl Drop for Viewer {
    /// Closes the connection (the reading thread holds a second handle on it).
    fn drop(&mut self) {
        let _ = self.conn.get_ref().shutdown(std::net::Shutdown::Both);
    }
}

/// The framebuffer a program leaves when Grail runs it to its end on its own, for
/// comparing with what a viewer received: black where the display is off.
pub fn grail_screen(program: &[u8]) -> Vec<u8> {
    let mut m = grail::Machine::with_ram(16 * 1024 * 1024);
    m.load_elf(program).unwrap();
    assert_eq!(m.run(Some(100_000_000), |_| {}), grail::RunResult::Exited(0));
    // What the display shows, from VRAM or RAM, at its mode's size; black when off.
    let frame = m.cpu.bus.frame();
    match frame.pixels {
        Some(pixels) => pixels.to_vec(),
        None => vec![0; frame.width as usize * frame.height as usize * 4],
    }
}

/// What `examples/sysinfo` prints on a machine with this RAM and clock and no disks
/// (as in grail's tests/machine.rs): version 2 of the sysinfo block, its 64-bit
/// addresses and sizes.
pub fn sysinfo_output(ram_mib: u32, hz: u32, real_time: bool) -> String {
    let kind = if real_time { "real time" } else { "nominal" };
    format!(
        "sysinfo v2
RAM      {ram_mib} MiB at 0x0000000080000000
CPU      {hz} Hz ({kind})
timebase {hz} Hz
9 devices:
  test      0x0000000000100000 + 0x0000000000001000
  CLINT     0x0000000002000000 + 0x0000000000010000
  PLIC      0x000000000c000000 + 0x0000000004000000
  UART      0x0000000010000000 + 0x0000000000000100  irq 1
  fb-ctrl   0x0000000030000000 + 0x0000000000001000  irq 16
  keyboard  0x0000000030001000 + 0x0000000000001000  irq 2
  sysinfo   0x0000000030002000 + 0x0000000000001000
  pointer   0x0000000030003000 + 0x0000000000001000  irq 11
  fb-vram   0x0000000031000000 + 0x000000000012c000
"
    )
}
