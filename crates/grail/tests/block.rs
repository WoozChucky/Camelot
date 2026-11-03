//! Xcal block device end to end (spec section 9.6): `examples/disk` on memory disks,
//! image files and host-made backends, the completion interrupt through the PLIC into a
//! trap, disks in the sysinfo device table, and the `--disk` / `--disk-ro` flags.

use excalibur::csr::{IRQ_M_EXT, MSTATUS_MIE};
use grail::block::{self, BlockBackend, FileDisk, MemDisk, SECTOR_SIZE};
use grail::bus::{BLK_BASE, BLK_SIZE, PLIC_BASE, RAM_BASE};
use grail::devices::BLK_IRQ;
use grail::{DiskConfig, Machine, MachineConfig, RunResult};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

/// The text at the start of sector 0 of `examples/disk/disk.img`.
const SECTOR0: &str =
    "Hello from sector 0 of disk.img!\nThis text is on the disk image, not in the program.\n";
/// The text at the start of its last sector, 7.
const SECTOR7: &str =
    "Sector 7: the example saves this sector, overwrites it, reads it back and restores it.\n";

/// `examples/disk/disk.img`: 8 sectors of zeros, with `SECTOR0` and `SECTOR7` at the
/// start of sectors 0 and 7.
fn disk_image() -> Vec<u8> {
    image(&[SECTOR0, "", "", "", "", "", "", SECTOR7])
}

/// A disk image with one sector per entry, each starting with that text.
fn image(sectors: &[&str]) -> Vec<u8> {
    let mut img = vec![0; sectors.len() * SECTOR_SIZE];
    for (i, text) in sectors.iter().enumerate() {
        img[i * SECTOR_SIZE..][..text.len()].copy_from_slice(text.as_bytes());
    }
    img
}

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

fn disk_elf() -> Vec<u8> {
    std::fs::read(examples().join("disk/disk.elf")).unwrap()
}

/// A file in the temp directory, removed when dropped.
struct TempFile(PathBuf);

impl TempFile {
    fn new(name: &str, contents: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!("grail-block-{}-{name}", std::process::id()));
        std::fs::write(&path, contents).unwrap();
        TempFile(path)
    }

    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }

    fn read(&self) -> Vec<u8> {
        std::fs::read(&self.0).unwrap()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn with_disks(disks: Vec<DiskConfig>) -> MachineConfig {
    MachineConfig {
        disks,
        ..MachineConfig::default()
    }
}

/// Runs `examples/disk` on `m`: its exit and what it printed.
fn run_example(mut m: Machine) -> (RunResult, String, Machine) {
    m.cpu.bus.uart.capture = true;
    m.load_elf(&disk_elf()).unwrap();
    let result = m.run(Some(10_000_000), |_| {});
    let out = String::from_utf8(std::mem::take(&mut m.cpu.bus.uart.captured)).unwrap();
    (result, out, m)
}

/// What `examples/disk` prints for a disk with `sectors` sectors starting with `sector0`
/// (read-write), at disk slot 0.
fn example_output(sectors: u32, sector0: &str) -> String {
    format!(
        "disk 0x10001000, irq 3: {sectors} sectors\nsector 0:\n{sector0}read past the end: error 2\n\
         write/read-back of sector {}: ok\n",
        sectors - 1
    )
}

/// The whole contents of disk `i` of `m`, read through its backend.
fn disk_contents(m: &mut Machine, i: usize) -> Vec<u8> {
    let backend = m.cpu.bus.disks[i].backend();
    let mut data = vec![0; backend.capacity() as usize * SECTOR_SIZE];
    backend.read(0, &mut data).unwrap();
    data
}

#[test]
fn committed_disk_image_is_current() {
    let path = examples().join("disk/disk.img");
    if std::env::var_os("GRAIL_WRITE_DISK_IMG").is_some() {
        std::fs::write(&path, disk_image()).unwrap();
    }
    assert!(
        std::fs::read(&path).unwrap() == disk_image(),
        "examples/disk/disk.img is out of date: rerun this test with GRAIL_WRITE_DISK_IMG=1"
    );
}

#[test]
fn example_reads_sector_0_and_tests_writing_on_a_memory_disk() {
    let m = Machine::new(with_disks(vec![DiskConfig::memory(disk_image())])).unwrap();
    let (result, out, mut m) = run_example(m);
    assert_eq!(result, RunResult::Exited(0), "{out}");
    assert_eq!(out, example_output(8, SECTOR0));
    assert_eq!(disk_contents(&mut m, 0), disk_image(), "sector 7 was restored");
    assert!(!m.cpu.bus.disks[0].done(), "every completion was acknowledged");
}

#[test]
fn example_on_an_image_file() {
    let file = TempFile::new("example.img", &disk_image());
    let m = Machine::new(with_disks(vec![DiskConfig::file(&file.0)])).unwrap();
    let (result, out, m) = run_example(m);
    assert_eq!(result, RunResult::Exited(0), "{out}");
    assert_eq!(out, example_output(8, SECTOR0));
    drop(m);
    assert_eq!(file.read(), disk_image(), "the file is as it was");

    // Another image: three sectors, different text.
    let small = image(&["Three sectors.\n", "", "last"]);
    let file = TempFile::new("small.img", &small);
    let m = Machine::new(with_disks(vec![DiskConfig::file(&file.0)])).unwrap();
    let (result, out, _) = run_example(m);
    assert_eq!(result, RunResult::Exited(0), "{out}");
    assert_eq!(out, example_output(3, "Three sectors.\n"));
    assert_eq!(file.read(), small);
}

#[test]
fn example_on_a_read_only_disk() {
    let m = Machine::new(with_disks(vec![
        DiskConfig::memory(disk_image()).with_read_only(true)
    ]))
    .unwrap();
    let (result, out, mut m) = run_example(m);
    assert_eq!(result, RunResult::Exited(0), "{out}");
    assert_eq!(
        out,
        format!(
            "disk 0x10001000, irq 3: 8 sectors (read-only)\nsector 0:\n{SECTOR0}\
             read past the end: error 2\nread-only disk: write test skipped\n"
        )
    );
    assert_eq!(disk_contents(&mut m, 0), disk_image());
}

#[test]
fn example_without_a_disk() {
    let (result, out, _) = run_example(Machine::new(MachineConfig::default()).unwrap());
    assert_eq!(result, RunResult::Exited(1));
    assert_eq!(out, "no disk (try grail --disk examples/disk/disk.img)\n");
}

#[test]
fn example_uses_the_first_disk_and_leaves_the_others_alone() {
    let other = image(&["second disk\n"; 4]);
    let config = with_disks(vec![
        DiskConfig::memory(disk_image()),
        DiskConfig::memory(other.clone()),
    ]);
    let (result, out, mut m) = run_example(Machine::new(config).unwrap());
    assert_eq!(result, RunResult::Exited(0), "{out}");
    assert_eq!(out, example_output(8, SECTOR0));
    assert_eq!(disk_contents(&mut m, 1), other);
}

/// A host-made backend that logs what the device asks of it.
struct Logged {
    disk: MemDisk,
    log: Arc<Mutex<Vec<String>>>,
}

impl BlockBackend for Logged {
    fn capacity(&self) -> u64 {
        self.disk.capacity()
    }
    fn read(&mut self, sector: u64, buf: &mut [u8]) -> io::Result<()> {
        self.log
            .lock()
            .unwrap()
            .push(format!("read {sector}+{}", buf.len()));
        self.disk.read(sector, buf)
    }
    fn write(&mut self, sector: u64, buf: &[u8]) -> io::Result<()> {
        self.log
            .lock()
            .unwrap()
            .push(format!("write {sector}+{}", buf.len()));
        self.disk.write(sector, buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.log.lock().unwrap().push("flush".into());
        Ok(())
    }
}

#[test]
fn example_on_a_host_backend() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let backend = Logged {
        disk: MemDisk::from_image(disk_image()).unwrap(),
        log: Arc::clone(&log),
    };
    let m = Machine::with_backends(MachineConfig::default(), vec![Box::new(backend)]).unwrap();
    let (result, out, _) = run_example(m);
    assert_eq!(result, RunResult::Exited(0), "{out}");
    assert_eq!(out, example_output(8, SECTOR0));
    // The read past the end never reached the backend: the device refused it.
    assert_eq!(
        *log.lock().unwrap(),
        [
            "read 0+512",
            "read 7+512",
            "write 7+512",
            "read 7+512",
            "write 7+512",
            "flush"
        ]
    );
}

#[test]
fn host_backends_come_after_configured_disks() {
    let config = with_disks(vec![DiskConfig::memory(vec![0; 2 * SECTOR_SIZE])]);
    let m = Machine::with_backends(config.clone(), vec![Box::new(MemDisk::new(5))]).unwrap();
    let capacities: Vec<u32> = m.cpu.bus.disks.iter().map(|d| d.capacity()).collect();
    assert_eq!(capacities, [2, 5]);
    let blocks = m
        .cpu
        .bus
        .sysinfo
        .devices()
        .iter()
        .filter(|d| d.kind == grail::sysinfo::DeviceType::Block);
    assert_eq!(blocks.count(), 2);

    let too_many: Vec<Box<dyn BlockBackend>> = (0..8).map(|_| Box::new(MemDisk::new(1)) as _).collect();
    let e = Machine::with_backends(config, too_many).err().unwrap();
    assert_eq!(e, "9 disks, at most 8");
}

/// A backend that fails everything, like an unplugged network disk.
struct Broken;

impl BlockBackend for Broken {
    fn capacity(&self) -> u64 {
        8
    }
    fn read(&mut self, _: u64, _: &mut [u8]) -> io::Result<()> {
        Err(io::Error::other("unplugged"))
    }
    fn write(&mut self, _: u64, _: &[u8]) -> io::Result<()> {
        Err(io::Error::other("unplugged"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("unplugged"))
    }
}

#[test]
fn backend_failures_reach_the_program_as_error_5() {
    let m = Machine::with_backends(MachineConfig::default(), vec![Box::new(Broken)]).unwrap();
    let (result, out, _) = run_example(m);
    assert_eq!(result, RunResult::Exited(2));
    assert_eq!(out, "disk 0x10001000, irq 3: 8 sectors\ndisk error 5\n");
}

#[test]
fn bad_disks_are_machine_errors() {
    let e = Machine::new(with_disks(vec![DiskConfig::file("no/such/disk.img")]))
        .err()
        .unwrap();
    assert!(e.starts_with("disk 0: no/such/disk.img: "), "{e}");
    let odd = TempFile::new("odd.img", &[0; 1000]);
    let config = with_disks(vec![DiskConfig::memory(vec![0; 512]), DiskConfig::file(&odd.0)]);
    let e = Machine::new(config).err().unwrap();
    assert!(
        e.starts_with("disk 1: ") && e.ends_with("disk image size 1000 is not a multiple of 512 bytes"),
        "{e}"
    );
    let e = Machine::new(with_disks(vec![DiskConfig::memory(vec![0; 100])]))
        .err()
        .unwrap();
    assert!(e.contains("whole number of 512-byte sectors"), "{e}");
}

const MEIP: u64 = 1 << IRQ_M_EXT;
const ENABLE0: u64 = PLIC_BASE + 0x2000;
const CLAIM0: u64 = PLIC_BASE + 0x20_0004;

/// The address of register `off` of the disk whose page is at `disk`.
fn disk_reg(disk: u64, off: u32) -> u64 {
    disk + u64::from(off)
}

#[test]
fn completion_interrupt_traps_into_m_mode() {
    // Disk 1 (PLIC source 4) with completion interrupts, the hart taking M-mode external
    // interrupts, and nops to run.
    let config = with_disks(vec![
        DiskConfig::memory(vec![0; 512]),
        DiskConfig::memory(image(&["hi"; 2])),
    ]);
    let mut m = Machine::new(config).unwrap();
    m.cpu.bus.load(RAM_BASE, &[0x13, 0, 0, 0].repeat(16)).unwrap();
    m.cpu.pc = RAM_BASE;
    m.cpu.csr.mtvec = RAM_BASE + 0x20;
    m.cpu.csr.mie = MEIP;
    m.cpu.csr.mstatus |= MSTATUS_MIE;
    let disk1 = BLK_BASE + BLK_SIZE;
    let irq = u64::from(BLK_IRQ + 1);
    let bus = &mut m.cpu.bus;
    bus.write(PLIC_BASE + 4 * irq, 4, 1).unwrap();
    bus.write(ENABLE0, 4, 1 << irq).unwrap();
    bus.write(disk_reg(disk1, block::REG_CTRL), 4, block::CTRL_IRQ_ENABLE.into())
        .unwrap();
    m.cpu.step();
    assert_eq!(m.cpu.csr.mcause, 0, "nothing to interrupt yet");

    let bus = &mut m.cpu.bus;
    bus.write(disk_reg(disk1, block::REG_SECTOR), 4, 1).unwrap();
    bus.write(disk_reg(disk1, block::REG_COUNT), 4, 1).unwrap();
    bus.write(disk_reg(disk1, block::REG_DMA), 4, RAM_BASE + 0x800)
        .unwrap();
    bus.write(disk_reg(disk1, block::REG_CMD), 4, block::CMD_READ.into())
        .unwrap();
    assert_eq!(
        bus.read(RAM_BASE + 0x800, 2),
        Ok(u64::from(u16::from_le_bytes(*b"hi")))
    );
    m.cpu.step();
    assert_eq!(
        m.cpu.csr.mcause,
        0x8000_0000 | u64::from(IRQ_M_EXT),
        "machine external interrupt"
    );
    assert_eq!(m.cpu.pc, RAM_BASE + 0x20, "at the handler");
    assert_eq!(m.cpu.csr.mepc, RAM_BASE + 4, "after the one nop that ran");

    // The handler: claim, acknowledge at the disk, complete. The line stays low after.
    let bus = &mut m.cpu.bus;
    assert_eq!(bus.read(CLAIM0, 4), Ok(irq));
    assert_eq!(
        bus.read(disk_reg(disk1, block::REG_ERROR), 4),
        Ok(block::error::NONE.into())
    );
    bus.write(disk_reg(disk1, block::REG_STATUS), 4, block::STATUS_DONE.into())
        .unwrap();
    bus.write(CLAIM0, 4, irq).unwrap();
    m.cpu.step();
    assert_eq!(m.cpu.mip() & MEIP, 0);
    assert_eq!(m.cpu.bus.read(PLIC_BASE + 0x1000, 4), Ok(0), "nothing pending");
}

#[test]
fn unacknowledged_completion_interrupts_again() {
    let mut m = Machine::new(with_disks(vec![DiskConfig::memory(vec![0; 512])])).unwrap();
    m.cpu.bus.load(RAM_BASE, &[0x13, 0, 0, 0].repeat(16)).unwrap();
    m.cpu.pc = RAM_BASE;
    let irq = u64::from(BLK_IRQ);
    let bus = &mut m.cpu.bus;
    bus.write(PLIC_BASE + 4 * irq, 4, 1).unwrap();
    bus.write(ENABLE0, 4, 1 << irq).unwrap();
    bus.write(disk_reg(BLK_BASE, block::REG_CTRL), 4, 1).unwrap();
    bus.write(disk_reg(BLK_BASE, block::REG_CMD), 4, block::CMD_FLUSH.into())
        .unwrap();
    m.cpu.step();
    assert_eq!(m.cpu.mip() & MEIP, MEIP);
    // Complete without acknowledging: DONE still holds the line high, so it pends again.
    assert_eq!(m.cpu.bus.read(CLAIM0, 4), Ok(irq));
    m.cpu.bus.write(CLAIM0, 4, irq).unwrap();
    m.cpu.step();
    assert_eq!(m.cpu.bus.read(CLAIM0, 4), Ok(irq));
    m.cpu
        .bus
        .write(
            disk_reg(BLK_BASE, block::REG_STATUS),
            4,
            block::STATUS_DONE.into(),
        )
        .unwrap();
    m.cpu.bus.write(CLAIM0, 4, irq).unwrap();
    m.cpu.step();
    assert_eq!(m.cpu.bus.read(CLAIM0, 4), Ok(0));
}

#[test]
fn file_disk_writes_reach_the_file() {
    let file = TempFile::new("persist.img", &image(&["a", "b", "c"]));
    let mut disk = FileDisk::open(&file.0, false).unwrap();
    assert_eq!(disk.capacity(), 3);
    assert!(!disk.read_only());
    disk.write(1, &[0x55; 2 * SECTOR_SIZE]).unwrap();
    disk.flush().unwrap();
    let mut buf = [0; SECTOR_SIZE];
    disk.read(0, &mut buf).unwrap();
    assert_eq!(buf[0], b'a');
    // Bad requests are errors, not panics, and change nothing.
    assert!(disk.read(3, &mut buf).is_err());
    assert!(disk.write(2, &[0; 2 * SECTOR_SIZE]).is_err());
    assert!(disk.write(0, &[0; 10]).is_err());
    drop(disk);
    let data = file.read();
    assert_eq!(data.len(), 3 * SECTOR_SIZE, "the file never grows");
    assert_eq!(&data[..1], b"a");
    assert!(data[SECTOR_SIZE..].iter().all(|&b| b == 0x55));

    // Read-only: the file is opened for reading only, and writes fail.
    let mut ro = FileDisk::open(&file.0, true).unwrap();
    assert!(ro.read_only());
    assert_eq!(
        ro.write(0, &buf).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    ro.flush().unwrap();
    ro.read(1, &mut buf).unwrap();
    assert_eq!(buf, [0x55; SECTOR_SIZE]);
    assert_eq!(file.read(), data);

    assert!(FileDisk::open(TempFile::new("bad.img", &[1; 513]).path(), false).is_err());
    assert_eq!(
        FileDisk::open(TempFile::new("empty.img", &[]).path(), false)
            .unwrap()
            .capacity(),
        0
    );
}

#[test]
fn sysinfo_lists_the_disks() {
    let config = with_disks(vec![
        DiskConfig::memory(disk_image()),
        DiskConfig::memory(vec![0; 512]),
    ]);
    let mut m = Machine::new(config).unwrap();
    m.cpu.bus.uart.capture = true;
    m.load_elf(&std::fs::read(examples().join("sysinfo/sysinfo.elf")).unwrap())
        .unwrap();
    assert_eq!(m.run(Some(1_000_000), |_| {}), RunResult::Exited(0));
    let out = String::from_utf8(std::mem::take(&mut m.cpu.bus.uart.captured)).unwrap();
    let devices = out.split_once("11 devices:\n").expect(&out).1;
    assert_eq!(
        devices,
        "  test      0x00100000 + 0x00001000
  CLINT     0x02000000 + 0x00010000
  PLIC      0x0c000000 + 0x04000000
  UART      0x10000000 + 0x00000100  irq 1
  block     0x10001000 + 0x00001000  irq 3
  block     0x10002000 + 0x00001000  irq 4
  fb-ctrl   0x30000000 + 0x00001000
  keyboard  0x30001000 + 0x00001000  irq 2
  sysinfo   0x30002000 + 0x00001000
  pointer   0x30003000 + 0x00001000  irq 11
  fb-vram   0x31000000 + 0x0012c000
"
    );
}

fn grail(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_grail"))
        .args(args)
        .output()
        .expect("cannot run grail")
}

fn elf_path(name: &str) -> String {
    examples()
        .join(format!("{name}/{name}.elf"))
        .to_str()
        .unwrap()
        .to_string()
}

/// Makes a file read-only (the Windows attribute, or no write permission on
/// Linux) and puts its permissions back when dropped, so the `TempFile` can
/// remove it.
struct ReadOnly<'a> {
    file: &'a TempFile,
    original: std::fs::Permissions,
}

impl<'a> ReadOnly<'a> {
    fn new(file: &'a TempFile) -> Self {
        let original = std::fs::metadata(&file.0).unwrap().permissions();
        let mut ro = original.clone();
        ro.set_readonly(true);
        std::fs::set_permissions(&file.0, ro).unwrap();
        ReadOnly { file, original }
    }
}

impl Drop for ReadOnly<'_> {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.file.0, self.original.clone());
    }
}

#[test]
fn a_read_only_file_is_opened_without_write_access() {
    // --disk-ro must work on media the host cannot write: FileDisk opens the file for
    // reading only. --disk asks for write access, which the host refuses.
    let file = TempFile::new("ro-attr.img", &disk_image());
    let _ro = ReadOnly::new(&file);
    // Someone allowed to write anyway (root on Linux) cannot check the refusal.
    let writable = std::fs::OpenOptions::new().write(true).open(&file.0).is_ok();

    let disk = FileDisk::open(file.path(), true).unwrap();
    assert!(disk.read_only());
    assert_eq!(disk.capacity(), 8);
    let out = grail(&["--disk-ro", file.path(), &elf_path("disk")]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8(out.stdout)
        .unwrap()
        .ends_with("read-only disk: write test skipped\n"));
    if !writable {
        assert!(FileDisk::open(file.path(), false).is_err());
        let out = grail(&["--disk", file.path(), &elf_path("disk")]);
        assert_eq!(out.status.code(), Some(2));
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(stderr.contains("grail: disk 0: "), "{stderr}");
    }
    assert_eq!(file.read(), disk_image());
}

#[test]
fn cli_disk_flags() {
    let file = TempFile::new("cli.img", &disk_image());
    let out = grail(&["--disk", file.path(), &elf_path("disk")]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8(out.stdout).unwrap(), example_output(8, SECTOR0));
    assert_eq!(file.read(), disk_image());

    let out = grail(&["--disk-ro", file.path(), &elf_path("disk")]);
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.starts_with("disk 0x10001000, irq 3: 8 sectors (read-only)\n"),
        "{stdout}"
    );
    assert!(
        stdout.ends_with("read-only disk: write test skipped\n"),
        "{stdout}"
    );

    // Several disks, in order.
    let second = TempFile::new("cli2.img", &[0; 1024]);
    let out = grail(&[
        "--disk",
        file.path(),
        "--disk-ro",
        second.path(),
        &elf_path("sysinfo"),
    ]);
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("11 devices:\n"), "{stdout}");
    assert!(
        stdout.contains("  block     0x10002000 + 0x00001000  irq 4\n"),
        "{stdout}"
    );
}

#[test]
fn cli_disk_errors() {
    let elf = elf_path("disk");
    let fails = |args: &[&str], want: &str| {
        let out = grail(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(stderr.contains(want), "{args:?}: {stderr}");
    };
    fails(
        &["--disk", "no/such/disk.img", &elf],
        "grail: disk 0: no/such/disk.img: ",
    );
    let odd = TempFile::new("cli-odd.img", &[0; 100]);
    fails(&["--disk", odd.path(), &elf], "is not a multiple of 512 bytes");
    let ok = TempFile::new("cli-ok.img", &[0; 512]);
    let nine: Vec<&str> = (0..9)
        .flat_map(|_| ["--disk-ro", ok.path()])
        .chain([elf.as_str()])
        .collect();
    fails(&nine, "grail: 9 disks, at most 8");
    fails(&[&elf, "--disk"], "--disk needs a value");

    // No disk at all: the example says so.
    let out = grail(&[&elf]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "no disk (try grail --disk examples/disk/disk.img)\n"
    );
}
