//! Helpers for the Lake tests: an in-process Lake on an ephemeral port serving a fresh
//! temporary directory, clients, and guest programs.
//!
//! The tests of machines on Lake disks run twice ([`on_both_widths`]): with RV32
//! machines, then with RV64 ones, whose disks have version 2's registers (Excalibur
//! spec section 9.6). Meanwhile the guests this module makes ([`guest`],
//! [`blk_equates`]) and the examples it reads ([`example_in`]) are of that width.

#![allow(dead_code)]

use avalon_proto::{Connection, DiskInfo, ErrorCode, Message};
use grail::Xlen;
use lake::{Config, Server, ServerThread};
use std::cell::Cell;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

thread_local! {
    /// The width of the machines this thread's test makes: set by [`on_both_widths`], and
    /// carried into the threads the test starts by [`spawn`]; `None` elsewhere.
    static XLEN: Cell<Option<Xlen>> = const { Cell::new(None) };
}

/// Runs `test` with RV32 machines, then with RV64 ones. A failure says which width
/// failed: the panic is raised again with the width before its message.
pub fn on_both_widths(test: impl Fn()) {
    for xlen in [Xlen::Rv32, Xlen::Rv64] {
        XLEN.with(|x| x.set(Some(xlen)));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&test));
        XLEN.with(|x| x.set(None));
        if let Err(payload) = result {
            let message = if let Some(s) = payload.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "a panic with no message".to_string()
            };
            panic!("on RV{}: {message}", xlen.bits());
        }
    }
}

/// The width of the machines the current test makes (RV32 outside [`on_both_widths`]).
///
/// A thread the test starts has the width if [`spawn`] started it. In a thread
/// `std::thread::spawn` starts the width is not set, and RV32 there would make RV32
/// guests in an RV64 test without a word: asking there panics. Such a thread is told
/// from the test's own by its name: libtest runs each test in a thread named after it,
/// and `std::thread::spawn`'s threads have none.
pub fn xlen() -> Xlen {
    XLEN.with(Cell::get).unwrap_or_else(|| {
        assert!(
            std::thread::current().name().is_some(),
            "a thread that asks for the width is started with common::spawn, which gives it the test's width"
        );
        Xlen::Rv32
    })
}

/// Starts a thread as `std::thread::spawn` does, with the width of the thread that
/// starts it: what the thread makes ([`xlen`]) is of the test's width.
pub fn spawn<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> std::thread::JoinHandle<T> {
    let xlen = xlen();
    std::thread::spawn(move || {
        XLEN.with(|x| x.set(Some(xlen)));
        f()
    })
}

/// The same width as the protocol says it, for a VM's profile.
pub fn profile_xlen() -> avalon_proto::Xlen {
    match xlen() {
        Xlen::Rv32 => avalon_proto::Xlen::Rv32,
        Xlen::Rv64 => avalon_proto::Xlen::Rv64,
    }
}

/// The program `name` of the example directory `dir`, built for the current width:
/// `<name>.elf` or `<name>-rv64.elf`.
pub fn example_in(dir: &str, name: &str) -> Vec<u8> {
    let file = match xlen() {
        Xlen::Rv32 => format!("{name}.elf"),
        Xlen::Rv64 => format!("{name}-rv64.elf"),
    };
    std::fs::read(examples_dir().join(dir).join(file)).unwrap()
}

/// An address as the examples print one: as many hex digits as the width's
/// registers hold.
pub fn addr_text(addr: u64) -> String {
    match xlen() {
        Xlen::Rv32 => format!("{addr:#010x}"),
        Xlen::Rv64 => format!("{addr:#018x}"),
    }
}

/// Where a disk's registers are and how wide, on the current width (spec section
/// 9.6): version 1's on RV32, all 32-bit; version 2's on RV64, `BLK_SECTOR` and
/// `BLK_DMA` 64-bit.
#[derive(Debug, Clone, Copy)]
pub struct BlkRegs {
    pub sector: u32,
    pub dma: u32,
    pub count: u32,
    pub cmd: u32,
    pub error: u32,
    /// The size of an access to `BLK_SECTOR` and `BLK_DMA`: 4 or 8 bytes.
    pub wide: u32,
}

pub fn blk_regs() -> BlkRegs {
    use grail::block;
    match xlen() {
        Xlen::Rv32 => BlkRegs {
            sector: block::REG_SECTOR,
            dma: block::REG_DMA,
            count: block::REG_COUNT,
            cmd: block::REG_CMD,
            error: block::REG_ERROR,
            wide: 4,
        },
        Xlen::Rv64 => BlkRegs {
            sector: block::v2::REG_SECTOR,
            dma: block::v2::REG_DMA,
            count: block::v2::REG_COUNT,
            cmd: block::v2::REG_CMD,
            error: block::v2::REG_ERROR,
            wide: 8,
        },
    }
}

/// A guest's names for its disk's registers on the current width ([`blk_regs`]):
/// `.equ`s of `BLK_SECTOR`, `BLK_DMA`, `BLK_COUNT`, `BLK_CMD` and `BLK_ERROR`. A guest
/// stores `BLK_SECTOR` and `BLK_DMA` with [`wide_store`].
pub fn blk_equates() -> String {
    let r = blk_regs();
    format!(
        "    .equ BLK_SECTOR, {:#x}\n    .equ BLK_DMA, {:#x}\n    .equ BLK_COUNT, {:#x}\n    \
         .equ BLK_CMD, {:#x}\n    .equ BLK_ERROR, {:#x}\n",
        r.sector, r.dma, r.count, r.cmd, r.error
    )
}

/// The store of a 64-bit register on the current width: `sw` on RV32, `sd` on RV64.
pub fn wide_store() -> &'static str {
    match xlen() {
        Xlen::Rv32 => "sw",
        Xlen::Rv64 => "sd",
    }
}

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

/// Assembles and links a guest of the current width with merlin-as and merlin-ld
/// (`.text` at 0x80000000, entry `_start`).
pub fn guest(asm: &str) -> Vec<u8> {
    let options = merlin_as::Options { xlen: xlen() };
    let obj = merlin_as::assemble_to_elf_with(asm, &options).unwrap_or_else(|e| panic!("{e:?}"));
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
