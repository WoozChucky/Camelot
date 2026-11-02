//! Grail's network disk (`grail::lake::LakeDisk`) against an in-process Lake:
//! `examples/disk` runs on a Lake disk read-write and read-only, big transfers are
//! split into requests, attachments follow the machines that hold them, and a server
//! that goes away or answers nonsense is an I/O error for the guest, never a crash.

mod common;

use avalon_proto::{Connection, Message, SECTOR_SIZE};
use common::*;
use grail::block::{self, BlockBackend, BlockDevice};
use grail::bus::RAM_BASE;
use grail::lake::LakeDisk;
use grail::{DiskConfig, Machine, MachineConfig, RunResult};
use std::net::{TcpListener, TcpStream};

/// `examples/disk/disk.img`, uploaded to `lake` as disk `name`.
fn upload_example_image(conn: &mut Connection<TcpStream>, name: &str) -> Vec<u8> {
    let image = std::fs::read(examples_dir().join("disk/disk.img")).unwrap();
    create_disk(conn, name, (image.len() / SECTOR_SIZE) as u64);
    let (h, _) = open(conn, name, false);
    write(conn, h, 0, &image);
    conn.request_ok(&Message::BlockClose { handle: h }).unwrap();
    image
}

/// The whole of disk `name`, read through the protocol.
fn contents(conn: &mut Connection<TcpStream>, name: &str) -> Vec<u8> {
    let (h, sectors) = open(conn, name, true);
    let data = read(conn, h, 0, sectors as u32);
    conn.request_ok(&Message::BlockClose { handle: h }).unwrap();
    data
}

/// Runs `examples/disk` on `m`: its exit and what it printed.
fn run_example(mut m: Machine) -> (RunResult, String, Machine) {
    m.cpu.bus.uart.capture = true;
    let elf = std::fs::read(examples_dir().join("disk/disk.elf")).unwrap();
    m.load_elf(&elf).unwrap();
    let result = m.run(Some(10_000_000), |_| {});
    let out = String::from_utf8(std::mem::take(&mut m.cpu.bus.uart.captured)).unwrap();
    (result, out, m)
}

fn machine(disks: Vec<DiskConfig>) -> Result<Machine, String> {
    Machine::new(MachineConfig {
        disks,
        ..MachineConfig::with_ram(1 << 20)
    })
}

const SECTOR0: &str =
    "Hello from sector 0 of disk.img!\nThis text is on the disk image, not in the program.\n";

#[test]
fn example_disk_runs_on_a_lake_disk() {
    let dir = TempDir::new("example");
    let server = lake(&dir);
    let mut rt = connect(&server);
    let image = upload_example_image(&mut rt, "disk");
    let addr = server.addr().to_string();

    let m = machine(vec![DiskConfig::lake(&addr, "disk")]).unwrap();
    // The machine holds the image while it exists.
    assert!(disk_info(&mut rt, "disk").writer);
    let (result, out, m) = run_example(m);
    assert_eq!(result, RunResult::Exited(0), "{out}");
    assert_eq!(
        out,
        format!(
            "disk 0x10001000, irq 3: 8 sectors\nsector 0:\n{SECTOR0}read past the end: error 2\n\
             write/read-back of sector 7: ok\n"
        )
    );
    assert!(!m.cpu.bus.disks[0].done(), "every completion was acknowledged");
    drop(m);
    // Dropping the machine detached the image before it returned.
    assert!(!disk_info(&mut rt, "disk").writer);
    assert_eq!(contents(&mut rt, "disk"), image, "sector 7 was restored");

    // Read-only, by two machines at once.
    let ro = || machine(vec![DiskConfig::lake(&addr, "disk").with_read_only(true)]).unwrap();
    let (a, b) = (ro(), ro());
    assert_eq!(disk_info(&mut rt, "disk").readers, 2);
    let rt_busy = error_of(
        &mut rt,
        &Message::BlockOpen {
            name: "disk".into(),
            read_only: false,
        },
    );
    assert_eq!(rt_busy, avalon_proto::ErrorCode::Busy);
    for m in [a, b] {
        let (result, out, _) = run_example(m);
        assert_eq!(result, RunResult::Exited(0), "{out}");
        assert!(
            out.starts_with("disk 0x10001000, irq 3: 8 sectors (read-only)\n"),
            "{out}"
        );
        assert!(out.ends_with("read-only disk: write test skipped\n"), "{out}");
    }
    assert_eq!(disk_info(&mut rt, "disk").readers, 0);
    server.stop().unwrap();
}

#[test]
fn big_transfers_are_split_into_requests() {
    let dir = TempDir::new("split");
    let server = lake(&dir);
    let mut rt = connect(&server);
    create_disk(&mut rt, "big", 5000);
    let addr = server.addr().to_string();
    let mut disk = LakeDisk::open(&addr, "big", false).unwrap();
    assert_eq!((disk.capacity(), disk.read_only()), (5000, false));
    assert!(disk.broken().is_none());

    // 4100 sectors: three requests of at most 2048.
    let data: Vec<u8> = (0..4100 * SECTOR_SIZE).map(|i| (i % 251) as u8).collect();
    disk.write(450, &data).unwrap();
    disk.flush().unwrap();
    let mut back = vec![0; data.len()];
    disk.read(450, &mut back).unwrap();
    assert!(back == data);
    let mut edge = vec![0; 2 * SECTOR_SIZE];
    disk.read(449, &mut edge).unwrap();
    assert_eq!(edge[..SECTOR_SIZE], [0; SECTOR_SIZE]);
    assert_eq!(edge[SECTOR_SIZE..], data[..SECTOR_SIZE]);
    let file = std::fs::read(dir.path().join("big")).unwrap();
    assert!(file[450 * SECTOR_SIZE..4550 * SECTOR_SIZE] == data[..]);

    // Bad requests are refused before anything is sent; the disk stays usable.
    assert!(disk.read(4999, &mut [0; 2 * SECTOR_SIZE]).is_err());
    assert!(disk.write(0, &[0; 100]).is_err());
    assert!(disk.read(u64::MAX, &mut [0; SECTOR_SIZE]).is_err());
    assert!(disk.broken().is_none());
    disk.read(4999, &mut [0; SECTOR_SIZE]).unwrap();
    drop(disk);

    // Read-only: refused locally.
    let mut ro = LakeDisk::open(&addr, "big", true).unwrap();
    let e = ro.write(0, &[0; SECTOR_SIZE]).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
    ro.flush().unwrap();
    server.stop().unwrap();
}

#[test]
fn attaching_follows_lakes_rule() {
    let dir = TempDir::new("rule");
    let server = lake(&dir);
    let mut rt = connect(&server);
    create_disk(&mut rt, "d", 8);
    let addr = server.addr().to_string();

    let writer = machine(vec![DiskConfig::lake(&addr, "d")]).unwrap();
    for read_only in [false, true] {
        let e = machine(vec![DiskConfig::lake(&addr, "d").with_read_only(read_only)])
            .err()
            .unwrap();
        assert!(e.starts_with(&format!("disk 0: lake://{addr}/d: busy: ")), "{e}");
    }
    // Disks keep their order: the Lake disk is disk 1, after a memory disk.
    let e = machine(vec![
        DiskConfig::memory(vec![0; 512]),
        DiskConfig::lake(&addr, "d"),
    ])
    .err()
    .unwrap();
    assert!(e.starts_with("disk 1: "), "{e}");
    drop(writer);

    // Dropping a machine detaches at once: the next one never finds it busy.
    for _ in 0..20 {
        let m = machine(vec![DiskConfig::lake(&addr, "d")]).unwrap();
        drop(m);
    }

    let e = machine(vec![DiskConfig::lake(&addr, "missing")]).err().unwrap();
    assert!(e.contains("no such disk"), "{e}");
    server.stop().unwrap();
}

#[test]
fn a_server_that_goes_away_is_an_io_error() {
    let dir = TempDir::new("gone");
    let server = lake(&dir);
    let mut rt = connect(&server);
    create_disk(&mut rt, "d", 8);
    let disk = LakeDisk::open(&server.addr().to_string(), "d", false).unwrap();
    let mut dev = BlockDevice::new(Box::new(disk)).unwrap();
    let mut ram = vec![0; 4 * SECTOR_SIZE];
    let mut cmd = |dev: &mut BlockDevice, cmd: u32| {
        for (reg, val) in [
            (block::REG_SECTOR, 1),
            (block::REG_COUNT, 2),
            (block::REG_DMA, RAM_BASE),
            (block::REG_CMD, cmd),
        ] {
            dev.write(reg, 4, val, &mut ram).unwrap();
        }
        dev.read(block::REG_ERROR, 4).unwrap()
    };
    assert_eq!(cmd(&mut dev, block::CMD_WRITE), block::error::NONE);
    assert_eq!(cmd(&mut dev, block::CMD_READ), block::error::NONE);
    drop(rt);
    server.stop().unwrap();

    // Every command now fails with BLK_ERROR 5, and nothing hangs or panics.
    for c in [
        block::CMD_READ,
        block::CMD_WRITE,
        block::CMD_FLUSH,
        block::CMD_READ,
    ] {
        assert_eq!(cmd(&mut dev, c), block::error::IO, "command {c}");
    }
    // The device's own checks still come first.
    dev.write(block::REG_SECTOR, 4, 100, &mut ram).unwrap();
    dev.write(block::REG_CMD, 4, block::CMD_READ, &mut ram).unwrap();
    assert_eq!(dev.read(block::REG_ERROR, 4).unwrap(), block::error::OUT_OF_RANGE);
    drop(dev);

    // And with no server at all, the disk cannot be opened.
    let e = LakeDisk::open(&server_addr_gone(), "d", false).unwrap_err();
    assert!(e.starts_with("lake://"), "{e}");
}

/// An address nobody listens on.
fn server_addr_gone() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().to_string()
}

/// A fake Lake that does the handshake, then answers each request with `answer`.
fn fake_lake(answer: impl Fn(Message) -> Message + Send + 'static) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let mut conn = Connection::new(stream);
            if conn.server_handshake("fake").is_err() {
                continue;
            }
            while let Ok(msg) = conn.recv() {
                if conn.send(&answer(msg)).is_err() {
                    break;
                }
            }
        }
    });
    addr
}

#[test]
fn a_server_that_answers_nonsense_is_an_io_error() {
    let opened = |read_only| Message::BlockOpened {
        handle: 7,
        sectors: 4,
        read_only,
    };
    // Answers every read with the wrong sector.
    let addr = fake_lake(move |msg| match msg {
        Message::BlockOpen { .. } => opened(false),
        Message::BlockRead {
            handle,
            sector,
            count,
        } => Message::BlockData {
            handle,
            sector: sector + 1,
            data: vec![0; count as usize * SECTOR_SIZE],
        },
        Message::BlockWrite { .. } => Message::Pong { token: 0 },
        _ => Message::Ok,
    });
    let mut disk = LakeDisk::open(&addr, "d", false).unwrap();
    assert_eq!((disk.handle(), disk.capacity()), (7, 4));
    disk.flush().unwrap();
    let e = disk.read(0, &mut [0; SECTOR_SIZE]).unwrap_err();
    assert!(e.to_string().contains("unexpected answer to BlockRead"), "{e}");
    // The connection is no longer trusted: every request fails from now on.
    assert!(disk.broken().is_some());
    assert!(disk.flush().is_err());
    drop(disk);

    let mut disk = LakeDisk::open(&addr, "d", false).unwrap();
    let e = disk.write(0, &[0; SECTOR_SIZE]).unwrap_err();
    assert!(e.to_string().contains("unexpected answer to BlockWrite"), "{e}");
    drop(disk);

    // A server that attaches read-only when asked for read-write.
    let addr = fake_lake(move |_| opened(true));
    let e = LakeDisk::open(&addr, "d", false).unwrap_err();
    assert!(e.contains("unexpected answer to BlockOpen"), "{e}");

    // Lake's own error replies leave the disk usable.
    let addr = fake_lake(move |msg| match msg {
        Message::BlockOpen { .. } => opened(false),
        Message::BlockRead { .. } => Message::Error {
            code: avalon_proto::ErrorCode::Io,
            message: "the host's disk failed".into(),
        },
        _ => Message::Ok,
    });
    let mut disk = LakeDisk::open(&addr, "d", false).unwrap();
    let e = disk.read(0, &mut [0; SECTOR_SIZE]).unwrap_err();
    assert!(e.to_string().contains("I/O error: the host's disk failed"), "{e}");
    assert!(disk.broken().is_none());
    disk.write(0, &[0; SECTOR_SIZE]).unwrap();
}
