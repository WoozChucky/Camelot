//! Lake and Avalon end to end, both in process: a VM whose profile names a Lake disk
//! (`lake://addr/name`) writes to it and is destroyed; a new VM attaches the same
//! image and reads the data back. Along the way, Lake's one-writer rule holds across
//! VMs, and a VM's attachment ends the moment Avalon has destroyed it.

mod common;

use avalon_proto::{
    Connection, DeviceSpec, DiskSource, DiskSpec, ErrorCode, HardwareProfile, Message, VmRef, VmState,
};
use common::*;
use std::net::TcpStream;
use std::time::Instant;

/// What the writer leaves at the start of sector 3.
const MESSAGE: &str = "Written by VM 'writer' to a Lake disk, read back by another VM.\n";

/// Copies `MESSAGE` (with its NUL) into a buffer in RAM, writes it to sector 3 of the
/// first disk and flushes, prints `written`, then spins, keeping the disk attached
/// until the VM is destroyed. A disk error halts with 100 + `BLK_ERROR`.
fn writer() -> Vec<u8> {
    guest(&format!(
        r#"
    .equ UART,        0x10000000
    .equ BLK,         0x10001000    # disk 0 (Excalibur spec section 9.6)
    .equ BUF,         0x80080000
    .globl _start
_start:
    li    s0, BLK
    li    s1, BUF
    la    t0, msg
    mv    t1, s1
1:  lbu   t2, 0(t0)
    sb    t2, 0(t1)
    addi  t0, t0, 1
    addi  t1, t1, 1
    bnez  t2, 1b
    li    t0, 3
    sw    t0, 0x08(s0)              # BLK_SECTOR
    li    t0, 1
    sw    t0, 0x10(s0)              # BLK_COUNT
    sw    s1, 0x0c(s0)              # BLK_DMA
    li    t0, 2
    sw    t0, 0x14(s0)              # BLK_CMD = write
    lw    a0, 0x1c(s0)              # BLK_ERROR
    bnez  a0, fail
    li    t0, 3
    sw    t0, 0x14(s0)              # BLK_CMD = flush
    lw    a0, 0x1c(s0)
    bnez  a0, fail
    la    a1, done
    call  puts
2:  j     2b
fail:
    addi  a0, a0, 100
    xcal.halt a0

# Prints the NUL-terminated string at a1.
puts:
    li    t0, UART
3:  lbu   t1, 0(a1)
    beqz  t1, 4f
    sb    t1, 0(t0)
    addi  a1, a1, 1
    j     3b
4:  ret

msg:  .asciz "{}"
done: .asciz "written\n"
"#,
        MESSAGE.escape_default()
    ))
}

/// Reads sector 3 of the first disk into RAM, prints it up to its first NUL, and halts
/// with 0 (or 100 + `BLK_ERROR`).
fn reader() -> Vec<u8> {
    guest(
        r#"
    .equ UART,        0x10000000
    .equ BLK,         0x10001000
    .equ BUF,         0x80080000
    .globl _start
_start:
    li    s0, BLK
    li    s1, BUF
    li    t0, 3
    sw    t0, 0x08(s0)              # BLK_SECTOR
    li    t0, 1
    sw    t0, 0x10(s0)              # BLK_COUNT
    sw    s1, 0x0c(s0)              # BLK_DMA
    li    t0, 1
    sw    t0, 0x14(s0)              # BLK_CMD = read
    lw    a0, 0x1c(s0)              # BLK_ERROR
    bnez  a0, fail
    li    t0, UART
1:  lbu   t1, 0(s1)
    beqz  t1, 2f
    sb    t1, 0(t0)
    addi  s1, s1, 1
    j     1b
2:  li    a0, 0
    xcal.halt a0
fail:
    addi  a0, a0, 100
    xcal.halt a0
"#,
    )
}

/// A 1 MiB machine with one Lake disk.
fn profile(lake: &str, disk: &str, read_only: bool) -> HardwareProfile {
    HardwareProfile {
        devices: vec![DeviceSpec::Disk(DiskSpec {
            source: DiskSource::Lake {
                addr: lake.into(),
                name: disk.into(),
            },
            read_only,
        })],
        ..HardwareProfile::with_ram(1 << 20)
    }
}

fn avalon() -> avalon::ServerThread {
    let config = avalon::Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        log: false,
        max_ram: None,
        ..avalon::Config::default()
    };
    avalon::Server::bind(&config).unwrap().spawn().unwrap()
}

fn connect_avalon(server: &avalon::ServerThread) -> Connection<TcpStream> {
    let conn = Connection::connect(server.addr(), "test").unwrap();
    conn.get_ref().set_read_timeout(Some(SLOW)).unwrap();
    conn
}

fn create_vm(
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

/// A connection attached to VM `vm`'s console.
fn console(server: &avalon::ServerThread, vm: u32) -> Connection<TcpStream> {
    let mut conn = connect_avalon(server);
    match conn.request(&Message::ConsoleAttach { vm: VmRef::Id(vm) }) {
        Ok(Message::ConsoleAttached { scrollback, .. }) => assert!(scrollback.is_empty()),
        other => panic!("attach: {other:?}"),
    }
    conn
}

/// Reads the console until the VM's state becomes `until`; returns the output.
fn output_until(console: &mut Connection<TcpStream>, until: impl Fn(VmState, &str) -> bool) -> String {
    let start = Instant::now();
    let mut out = String::new();
    let mut state = VmState::Created;
    while !until(state, &out) {
        assert!(
            start.elapsed() < SLOW,
            "waiting; output so far {out:?}, state {state}"
        );
        match console.recv().unwrap() {
            Message::ConsoleOutput { data, .. } => out.push_str(&String::from_utf8_lossy(&data)),
            Message::VmStateChanged { state: s, .. } => state = s,
            other => panic!("{other:?}"),
        }
    }
    out
}

fn start(avalon: &mut Connection<TcpStream>, vm: u32) {
    avalon
        .request_ok(&Message::StartVm { vm: VmRef::Id(vm) })
        .unwrap();
}

fn destroy(avalon: &mut Connection<TcpStream>, vm: u32) {
    avalon
        .request_ok(&Message::DestroyVm { vm: VmRef::Id(vm) })
        .unwrap();
}

/// The create of a VM that must fail: its error code and message.
fn create_fails(conn: &mut Connection<TcpStream>, profile: HardwareProfile) -> (ErrorCode, String) {
    match conn.request(&Message::CreateVm {
        name: "intruder".into(),
        profile,
        program: reader(),
    }) {
        Err(avalon_proto::Error::Remote { code, message }) => (code, message),
        other => panic!("the create should fail: {other:?}"),
    }
}

#[test]
fn a_vm_writes_a_lake_disk_and_another_reads_it_back() {
    let dir = TempDir::new("e2e");
    let lake = lake(&dir);
    let lake_addr = lake.addr().to_string();
    let avalon = avalon();
    let mut storage = connect(&lake);
    let mut rt = connect_avalon(&avalon);
    create_disk(&mut storage, "shared", 16);

    // The writer attaches the disk read-write when it is created.
    let w = create_vm(&mut rt, "writer", profile(&lake_addr, "shared", false), writer());
    let info = disk_info(&mut storage, "shared");
    assert!(info.writer && info.readers == 0, "{info:?}");
    // So no other VM may attach it, read-write or read-only.
    for read_only in [false, true] {
        let (code, message) = create_fails(&mut rt, profile(&lake_addr, "shared", read_only));
        assert_eq!(code, ErrorCode::InvalidProfile);
        assert!(
            message.contains(&format!("lake://{lake_addr}/shared: busy: ")),
            "{message}"
        );
    }

    let mut out = console(&avalon, w);
    start(&mut rt, w);
    output_until(&mut out, |_, out| out == "written\n");
    // The writer is still running, holding the disk, when it is destroyed.
    assert!(disk_info(&mut storage, "shared").writer);
    destroy(&mut rt, w);
    // Destroyed means detached: nothing to wait for.
    let info = disk_info(&mut storage, "shared");
    assert!(!info.writer && info.readers == 0, "{info:?}");
    // The data is on the disk: in Lake's image, and in its file.
    let (h, _) = open(&mut storage, "shared", true);
    let sector = read(&mut storage, h, 3, 1);
    assert_eq!(&sector[..MESSAGE.len()], MESSAGE.as_bytes());
    assert_eq!(sector[MESSAGE.len()], 0);
    storage.request_ok(&Message::BlockClose { handle: h }).unwrap();
    let file = std::fs::read(dir.path().join("shared")).unwrap();
    assert_eq!(&file[3 * 512..][..MESSAGE.len()], MESSAGE.as_bytes());

    // A new VM attaches the same image, read-write, at once, and reads the data back.
    let r = create_vm(&mut rt, "reader", profile(&lake_addr, "shared", false), reader());
    let mut out = console(&avalon, r);
    start(&mut rt, r);
    let printed = output_until(&mut out, |state, _| state == VmState::Exited(0));
    assert_eq!(printed, MESSAGE);
    destroy(&mut rt, r);

    // Any number of VMs can share it read-only.
    let readers: Vec<u32> = (0..3)
        .map(|i| {
            create_vm(
                &mut rt,
                &format!("ro-{i}"),
                profile(&lake_addr, "shared", true),
                reader(),
            )
        })
        .collect();
    assert_eq!(disk_info(&mut storage, "shared").readers, 3);
    let (code, _) = create_fails(&mut rt, profile(&lake_addr, "shared", false));
    assert_eq!(
        code,
        ErrorCode::InvalidProfile,
        "no writer while there are readers"
    );
    for &vm in &readers {
        let mut out = console(&avalon, vm);
        start(&mut rt, vm);
        let printed = output_until(&mut out, |state, _| state == VmState::Exited(0));
        assert_eq!(printed, MESSAGE);
    }
    // An exited VM's machine is dropped as its thread ends (just after the state
    // change is sent), and its attachment with it.
    wait_until("the exited readers to detach", || {
        disk_info(&mut storage, "shared").readers == 0
    });
    for vm in readers {
        destroy(&mut rt, vm);
    }

    // The disk can be deleted once no VM has it.
    storage
        .request_ok(&Message::DiskDelete {
            name: "shared".into(),
        })
        .unwrap();
    avalon.stop().unwrap();
    lake.stop().unwrap();
}

#[test]
fn missing_and_unreachable_lake_disks_are_errors() {
    let dir = TempDir::new("missing");
    let lake = lake(&dir);
    let lake_addr = lake.addr().to_string();
    let avalon = avalon();
    let mut rt = connect_avalon(&avalon);
    let (code, message) = create_fails(&mut rt, profile(&lake_addr, "nope", false));
    assert_eq!(code, ErrorCode::InvalidProfile);
    assert!(message.contains("no such disk"), "{message}");
    let (code, message) = create_fails(&mut rt, profile(&lake_addr, "Bad", false));
    assert_eq!(code, ErrorCode::InvalidProfile);
    assert!(message.contains("lake://"), "{message}");
    lake.stop().unwrap();
    let (code, _) = create_fails(&mut rt, profile(&lake_addr, "nope", false));
    assert_eq!(code, ErrorCode::InvalidProfile);
    // Avalon carries on.
    assert_eq!(
        rt.request(&Message::ListVms).unwrap(),
        Message::VmList { vms: vec![] }
    );
    avalon.stop().unwrap();
}

/// The README's Lake example is the test below, but for the server's address: each of
/// its statements must be there, word for word (spaces and commas aside, so rustfmt's
/// line breaks do not count).
#[test]
fn the_readme_lake_example_is_the_test() {
    // A Windows checkout may have CRLF line endings.
    let readme = include_str!("../../../README.md").replace("\r\n", "\n");
    let section = &readme[readme.find("### Running Lake").unwrap()..];
    let start = section.find("```rust\n").unwrap() + "```rust\n".len();
    let block = &section[start..start + section[start..].find("```").unwrap()];
    let squash = |s: &str| {
        s.chars()
            .filter(|c| !c.is_whitespace() && *c != ',')
            .collect::<String>()
    };
    let test = squash(include_str!("avalon.rs"));
    let mut checked = 0;
    for line in block.lines() {
        let line = line.split(" // ").next().unwrap();
        if line.trim().is_empty() || line.starts_with("use ") || line.starts_with("//") {
            continue;
        }
        let line = line.replace("\"127.0.0.1:7401\"", "lake_addr.as_str()").replace(
            "\"lake://127.0.0.1:7401/root\"",
            "&format!(\"lake://{lake_addr}/root\")",
        );
        assert!(test.contains(&squash(&line)), "not in the test: {line}");
        checked += 1;
    }
    assert_eq!(checked, 7);
}

/// The Lake example in the README, against in-process servers.
#[test]
fn the_readme_lake_example() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new("readme");
    let server = lake(&dir);
    let lake_addr = server.addr().to_string();
    let avalon = avalon();

    let mut lake = Connection::connect(lake_addr.as_str(), "example")?;
    // Not in the README: a broken server fails the test instead of hanging it.
    lake.get_ref().set_read_timeout(Some(SLOW))?;
    lake.request_ok(&Message::DiskCreate {
        name: "root".into(),
        sectors: 2048,
        source: None,
    })?;
    let source = DiskSource::parse(&format!("lake://{lake_addr}/root"))?;
    let profile = HardwareProfile {
        devices: vec![DeviceSpec::Disk(DiskSpec {
            source,
            read_only: false,
        })],
        ..HardwareProfile::with_ram(64 << 20)
    };

    let mut rt = connect_avalon(&avalon);
    let vm = create_vm(&mut rt, "demo", profile, reader());
    let info = disk_info(&mut lake, "root");
    assert!(info.writer && info.sectors == 2048, "{info:?}");
    let details = match rt.request(&Message::GetVmInfo { vm: VmRef::Id(vm) })? {
        Message::VmInfo(details) => details,
        other => panic!("{other:?}"),
    };
    // The guest sees one disk.
    let disk = grail::sysinfo::DeviceType::Block as u32;
    assert_eq!(details.devices.iter().filter(|d| d.kind == disk).count(), 1);
    destroy(&mut rt, vm);
    avalon.stop()?;
    server.stop()?;
    Ok(())
}
