//! Helpers for the Lake tests: an in-process Lake on an ephemeral port serving a fresh
//! temporary directory, clients, and guest programs.

#![allow(dead_code)]

use avalon_proto::{Connection, DiskInfo, ErrorCode, Message};
use lake::{Config, Server, ServerThread};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Generous ceiling for anything that should take well under a second, so a loaded CI
/// machine does not make the tests flaky.
pub const SLOW: Duration = Duration::from_secs(10);

/// A fresh directory under the system's temporary directory, removed when dropped.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("lake-test-{}-{tag}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        TempDir(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The configuration [`lake`] starts from: 127.0.0.1, an ephemeral port, no log.
pub fn quiet(dir: &TempDir) -> Config {
    Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        log: false,
        ..Config::new(dir.path())
    }
}

/// A Lake on 127.0.0.1 and an ephemeral port, serving `dir`.
pub fn lake(dir: &TempDir) -> ServerThread {
    lake_with(quiet(dir))
}

pub fn lake_with(config: Config) -> ServerThread {
    Server::bind(&config).unwrap().spawn().unwrap()
}

pub fn connect(server: &ServerThread) -> Connection<TcpStream> {
    let conn = Connection::connect(server.addr(), "test").unwrap();
    conn.get_ref().set_read_timeout(Some(SLOW)).unwrap();
    conn
}

/// The error code of a request that must fail.
pub fn error_of(conn: &mut Connection<TcpStream>, msg: &Message) -> ErrorCode {
    match conn.request(msg) {
        Err(avalon_proto::Error::Remote { code, .. }) => code,
        other => panic!("{msg:?} should fail, got {other:?}"),
    }
}

pub fn create_disk(conn: &mut Connection<TcpStream>, name: &str, sectors: u64) {
    conn.request_ok(&Message::DiskCreate {
        name: name.into(),
        sectors,
        source: None,
    })
    .unwrap_or_else(|e| panic!("create {name}: {e}"));
}

pub fn list(conn: &mut Connection<TcpStream>) -> Vec<DiskInfo> {
    match conn.request(&Message::ListDisks) {
        Ok(Message::DiskList { disks }) => disks,
        other => panic!("list: {other:?}"),
    }
}

/// The one disk named `name`, from a listing.
pub fn disk_info(conn: &mut Connection<TcpStream>, name: &str) -> DiskInfo {
    list(conn)
        .into_iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("no disk {name}"))
}

/// Opens `name`, returning the handle and size.
pub fn open(conn: &mut Connection<TcpStream>, name: &str, read_only: bool) -> (u32, u64) {
    match conn.request(&Message::BlockOpen {
        name: name.into(),
        read_only,
    }) {
        Ok(Message::BlockOpened {
            handle,
            sectors,
            read_only: ro,
        }) => {
            assert_eq!(ro, read_only);
            (handle, sectors)
        }
        other => panic!("open {name}: {other:?}"),
    }
}

pub fn read(conn: &mut Connection<TcpStream>, handle: u32, sector: u64, count: u32) -> Vec<u8> {
    match conn.request(&Message::BlockRead {
        handle,
        sector,
        count,
    }) {
        Ok(Message::BlockData {
            handle: h,
            sector: s,
            data,
        }) => {
            assert_eq!((h, s), (handle, sector));
            assert_eq!(data.len(), count as usize * 512);
            data
        }
        other => panic!("read: {other:?}"),
    }
}

pub fn write(conn: &mut Connection<TcpStream>, handle: u32, sector: u64, data: &[u8]) {
    conn.request_ok(&Message::BlockWrite {
        handle,
        sector,
        data: data.to_vec(),
    })
    .unwrap_or_else(|e| panic!("write: {e}"));
}

/// Waits until `done` holds, polling.
pub fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < SLOW, "still waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
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

pub fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}
