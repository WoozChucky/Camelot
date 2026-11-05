//! Helpers for Round Table's tests: an in-process Avalon, a stand-in Lake that speaks
//! the disk-management part of the protocol, and Round Table's command layer.
//!
//! The tests of VMs run twice ([`on_both_widths`]): with RV32 VMs, then with RV64
//! ones. Meanwhile the examples are the width's builds ([`example`]), and
//! [`xlen_arg`], [`arch`] and [`arch_text`] give what Round Table takes and shows of
//! the width.

#![allow(dead_code)]

use avalon::{Config, Server, ServerThread};
use avalon_proto::{validate_name, Connection, DiskInfo, ErrorCode, Message, VmRef, VmState, Xlen};
use round_table::Failure;
use std::cell::Cell;
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

thread_local! {
    /// The width of the VMs this thread's test makes ([`on_both_widths`]).
    static XLEN: Cell<Xlen> = const { Cell::new(Xlen::Rv32) };
}

/// Runs `test` with RV32 VMs, then with RV64 ones. A failure says which width failed:
/// the panic is raised again with the width before its message.
pub fn on_both_widths(test: impl Fn()) {
    for xlen in [Xlen::Rv32, Xlen::Rv64] {
        XLEN.with(|x| x.set(xlen));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&test));
        XLEN.with(|x| x.set(Xlen::Rv32));
        if let Err(payload) = result {
            let message = if let Some(s) = payload.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "a panic with no message".to_string()
            };
            panic!("on {xlen}: {message}");
        }
    }
}

/// The width of the VMs the current test makes (RV32 outside [`on_both_widths`]).
pub fn xlen() -> Xlen {
    XLEN.with(Cell::get)
}

/// `create`'s `--xlen` value for the current width: `32` or `64`.
pub fn xlen_arg() -> &'static str {
    match xlen() {
        Xlen::Rv32 => "32",
        Xlen::Rv64 => "64",
    }
}

/// What `list` shows of a VM of the current width: `RV32` or `RV64`.
pub fn arch() -> &'static str {
    match xlen() {
        Xlen::Rv32 => "RV32",
        Xlen::Rv64 => "RV64",
    }
}

/// What `info` shows of a VM of the current width with the default paging.
pub fn arch_text() -> &'static str {
    match xlen() {
        Xlen::Rv32 => "RV32 (Sv32 paging)",
        Xlen::Rv64 => "RV64 (Sv48 and Sv39 paging)",
    }
}

pub const MIB: u64 = 1024 * 1024;

/// Generous ceiling for anything that should take well under a second.
pub const SLOW: Duration = Duration::from_secs(10);

/// An Avalon server on 127.0.0.1 and an ephemeral port, quiet, with a RAM budget.
pub fn avalon(max_ram_mib: u64) -> ServerThread {
    let config = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        log: false,
        max_ram: Some(max_ram_mib * MIB),
        ..Config::default()
    };
    Server::bind(&config).unwrap().spawn().unwrap()
}

/// Runs Round Table's command layer with `args`, as the binary would, and returns what
/// it printed.
pub fn rt(args: &[&str]) -> Result<String, Failure> {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let mut out = Vec::new();
    let result = round_table::run(&args, &mut out);
    let text = String::from_utf8(out).expect("the output is UTF-8");
    result.map(|()| text)
}

/// Runs a command against the Avalon at `addr` that must succeed.
pub fn ok(addr: SocketAddr, args: &[&str]) -> String {
    let server = addr.to_string();
    let mut full = vec!["--server", &server];
    full.extend_from_slice(args);
    rt(&full).unwrap_or_else(|e| panic!("{args:?}: {e}"))
}

/// Runs a command against the Avalon at `addr` that must fail with a server's
/// refusal; returns the message.
pub fn refused(addr: SocketAddr, args: &[&str]) -> String {
    let server = addr.to_string();
    let mut full = vec!["--server", &server];
    full.extend_from_slice(args);
    match rt(&full) {
        Err(Failure::Failed(why)) => why,
        other => panic!("{args:?} should be refused, got {other:?}"),
    }
}

/// The example `name`, built for the current width: `<name>.elf` or
/// `<name>-rv64.elf`.
pub fn example(name: &str) -> PathBuf {
    let file = match xlen() {
        Xlen::Rv32 => format!("{name}.elf"),
        Xlen::Rv64 => format!("{name}-rv64.elf"),
    };
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
        .join(file)
}

pub fn example_str(name: &str) -> String {
    example(name).to_str().unwrap().to_string()
}

/// A direct protocol connection, to check what Round Table did independently of it.
pub fn connect(addr: SocketAddr) -> Connection<TcpStream> {
    let conn = Connection::connect(addr, "test").unwrap();
    conn.get_ref().set_read_timeout(Some(SLOW)).unwrap();
    conn
}

pub fn state(addr: SocketAddr, vm: &str) -> VmState {
    match connect(addr).request(&Message::GetVmInfo { vm: VmRef::parse(vm) }) {
        Ok(Message::VmInfo(details)) => details.vm.state,
        other => panic!("info {vm}: {other:?}"),
    }
}

pub fn steps(addr: SocketAddr, vm: &str) -> u64 {
    match connect(addr).request(&Message::GetVmInfo { vm: VmRef::parse(vm) }) {
        Ok(Message::VmInfo(details)) => details.steps,
        other => panic!("info {vm}: {other:?}"),
    }
}

/// Waits until VM `vm` is in a state `done` accepts.
pub fn wait_for_state(addr: SocketAddr, vm: &str, done: impl Fn(VmState) -> bool) -> VmState {
    let start = Instant::now();
    loop {
        let s = state(addr, vm);
        if done(s) {
            return s;
        }
        assert!(start.elapsed() < SLOW, "VM {vm} still {s}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// An address nothing listens on.
pub fn dead_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap()
}

/// A stand-in server named `name` on an ephemeral port: does the handshake, then
/// answers every request with `answer(request)`.
pub fn fake_server(name: &'static str, answer: fn(Message) -> Message) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            std::thread::spawn(move || {
                let mut conn = Connection::new(stream);
                if conn.server_handshake(name).is_err() {
                    return;
                }
                while let Ok(request) = conn.recv() {
                    if conn.send(&answer(request)).is_err() {
                        return;
                    }
                }
            });
        }
    });
    addr
}

/// Whether this process has the file at `path` open, as a VM's disk; `None` where
/// the test cannot tell.
pub fn file_is_open(path: &Path) -> Option<bool> {
    #[cfg(windows)]
    {
        // An open that shares nothing fails while any other handle is open.
        use std::os::windows::fs::OpenOptionsExt;
        const ERROR_SHARING_VIOLATION: i32 = 32;
        match std::fs::OpenOptions::new().read(true).share_mode(0).open(path) {
            Ok(_) => Some(false),
            Err(e) if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION) => Some(true),
            Err(e) => panic!("{}: {e}", path.display()),
        }
    }
    #[cfg(target_os = "linux")]
    {
        let path = path.canonicalize().unwrap();
        let fds = std::fs::read_dir("/proc/self/fd").unwrap();
        Some(
            fds.flatten()
                .any(|fd| std::fs::read_link(fd.path()).is_ok_and(|p| p == path)),
        )
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = path;
        None
    }
}

/// Waits until [`file_is_open`] says `open` (or cannot tell).
pub fn wait_for_open(path: &Path, open: bool) {
    let start = Instant::now();
    while file_is_open(path).is_some_and(|o| o != open) {
        assert!(
            start.elapsed() < SLOW,
            "{} is still {}",
            path.display(),
            if open { "closed" } else { "open" }
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A stand-in for Lake: serves the handshake and the disk-management messages
/// (`DiskCreate`, `ListDisks`, `DiskDelete`) from a table in memory, with Lake's
/// rules (names, one image per name, no deleting an attached image), and answers
/// everything else `Unsupported`. The real Lake brings an end-to-end test of its own.
pub struct FakeLake {
    pub addr: SocketAddr,
    /// The images, by name. Tests add attached ones directly.
    pub disks: Arc<Mutex<BTreeMap<String, DiskInfo>>>,
    /// The agent name of every client that completed the handshake.
    pub agents: Arc<Mutex<Vec<String>>>,
}

impl FakeLake {
    pub fn start() -> FakeLake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let disks = Arc::new(Mutex::new(BTreeMap::new()));
        let agents = Arc::new(Mutex::new(Vec::new()));
        let (d, a) = (disks.clone(), agents.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let (disks, agents) = (d.clone(), a.clone());
                std::thread::spawn(move || serve(stream, &disks, &agents));
            }
        });
        FakeLake { addr, disks, agents }
    }

    /// Adds an image as if clients had attached it.
    pub fn add(&self, name: &str, sectors: u64, readers: u32, writer: bool) {
        self.disks.lock().unwrap().insert(
            name.to_string(),
            DiskInfo {
                name: name.to_string(),
                sectors,
                readers,
                writer,
            },
        );
    }

    pub fn names(&self) -> Vec<String> {
        self.disks.lock().unwrap().keys().cloned().collect()
    }
}

fn serve(stream: TcpStream, disks: &Mutex<BTreeMap<String, DiskInfo>>, agents: &Mutex<Vec<String>>) {
    let mut conn = Connection::new(stream);
    match conn.server_handshake("fake lake") {
        Ok(agent) => agents.lock().unwrap().push(agent),
        Err(_) => return,
    }
    loop {
        let msg = match conn.recv() {
            Ok(msg) => msg,
            Err(e) if e.is_fatal() => return,
            Err(e) => {
                let _ = conn.send_error(&e);
                continue;
            }
        };
        let fail = |code, message: String| Message::Error { code, message };
        let reply = match msg {
            Message::Ping { token } => Message::Pong { token },
            Message::DiskCreate {
                name,
                sectors,
                source: None,
            } => {
                let mut disks = disks.lock().unwrap();
                if let Err(e) = validate_name(&name) {
                    fail(ErrorCode::InvalidName, e)
                } else if sectors == 0 {
                    fail(ErrorCode::Io, "an image needs at least one sector".into())
                } else {
                    match disks.entry(name.clone()) {
                        Entry::Occupied(_) => fail(
                            ErrorCode::AlreadyExists,
                            format!("a disk named '{name}' already exists"),
                        ),
                        Entry::Vacant(slot) => {
                            slot.insert(DiskInfo {
                                name,
                                sectors,
                                readers: 0,
                                writer: false,
                            });
                            Message::Ok
                        }
                    }
                }
            }
            Message::ListDisks => Message::DiskList {
                disks: disks.lock().unwrap().values().cloned().collect(),
            },
            Message::DiskDelete { name } => {
                let mut disks = disks.lock().unwrap();
                match disks.get(&name) {
                    None => fail(ErrorCode::NoSuchDisk, format!("no disk named '{name}'")),
                    Some(d) if d.writer || d.readers > 0 => {
                        fail(ErrorCode::Busy, format!("disk '{name}' is attached"))
                    }
                    Some(_) => {
                        disks.remove(&name);
                        Message::Ok
                    }
                }
            }
            other => fail(
                ErrorCode::Unsupported,
                format!("{}: Lake serves block I/O and disk management", other.name()),
            ),
        };
        if conn.send(&reply).is_err() {
            return;
        }
    }
}
