//! Helpers for Round Table's tests: an in-process Avalon, a stand-in Lake that speaks
//! the disk-management part of the protocol, and Round Table's command layer.

#![allow(dead_code)]

use avalon::{Config, Server, ServerThread};
use avalon_proto::{validate_name, Connection, DiskInfo, ErrorCode, Message, VmRef, VmState};
use round_table::Failure;
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

pub fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../examples/{name}/{name}.elf"))
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
            Message::DiskCreate { name, sectors } => {
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
