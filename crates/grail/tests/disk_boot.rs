//! Booting from disk (spec section 7, "Boot from disk"): the boot ROM's disk loader
//! with hand-made boot blocks on in-memory disks (where the block lands, the registers
//! it starts with, every way the ROM refuses a disk, a trap in the boot block, the
//! writable entry word and the parked harts), then the `grail` binary booting the
//! Camelot kernel from an image that `camelot-fs` made bootable with the committed
//! bootloader (`kernel/boot/boot.elf`) and kernel, with no program argument.
//!
//! Every run has a step limit: a ROM that hung instead of failing would hit it.

use grail::block::BlockBackend;
use grail::boot::{self, BOOT_BLOCK_SIZE, DISK_BOOT_PC, EXIT_BOOT_FAILED};
use grail::bus::{BLK_BASE, BOOT_ROM_BASE, PARK_PC, SYSINFO_BASE};
use grail::{DiskConfig, Machine, MachineConfig, RunResult};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const T0: u32 = 5;
const T1: u32 = 6;
const A0: u32 = 10;
const A1: u32 = 11;
const A2: u32 = 12;
const A3: u32 = 13;
const ZERO: u32 = 0;

/// 1 MiB of RAM: the boot block goes at 0x800f_0000.
const RAM: usize = 1 << 20;
const LOAD_ADDR: u32 = 0x800f_0000;
/// Far more than the ROM needs (it reads two sectors and adds up 256 words).
const STEPS: u64 = 100_000;

fn addi(rd: u32, rs1: u32, imm: i32) -> u32 {
    ((imm as u32) & 0xfff) << 20 | rs1 << 15 | rd << 7 | 0x13
}
fn auipc(rd: u32) -> u32 {
    rd << 7 | 0x17
}
fn lui(rd: u32, imm20: u32) -> u32 {
    imm20 << 12 | rd << 7 | 0x37
}
fn sw(rs2: u32, rs1: u32, imm: u32) -> u32 {
    (imm >> 5) << 25 | rs2 << 20 | rs1 << 15 | 2 << 12 | (imm & 31) << 7 | 0x23
}
/// `j .`
const STOP: u32 = 0x0000_006f;
/// `xcal.halt zero`: exit code 0.
const HALT: u32 = 0x0000_100b;

/// A boot block holding `code` (at its start), signed and checksummed.
fn boot_block(code: &[u32]) -> [u8; BOOT_BLOCK_SIZE] {
    let mut block = [0u8; BOOT_BLOCK_SIZE];
    for (i, w) in code.iter().enumerate() {
        block[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
    }
    boot::seal(&mut block);
    block
}

/// A disk of `sectors` sectors starting with `start`.
fn disk(start: &[u8], sectors: usize) -> Vec<u8> {
    let mut d = vec![0u8; sectors * 512];
    d[..start.len()].copy_from_slice(start);
    d
}

/// A machine of `harts` harts with `image` as disk 0, set to boot from it, its UART
/// captured.
fn machine(harts: usize, image: Vec<u8>) -> Machine {
    let config = MachineConfig {
        ram_size: RAM,
        harts,
        disks: vec![DiskConfig::memory(image)],
        ..MachineConfig::default()
    };
    let mut m = Machine::new(config).unwrap();
    m.boot_from_disk().unwrap();
    m.cpu.bus.uart.capture = true;
    m
}

fn uart(m: &Machine) -> String {
    String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned()
}

/// Runs `image` and expects the ROM to refuse it with `message`.
fn refused(m: &mut Machine, message: &str) {
    assert_eq!(m.run(Some(STEPS), |_| {}), RunResult::Exited(EXIT_BOOT_FAILED));
    assert_eq!(uart(m), format!("boot ROM: {message}\n"));
}

#[test]
fn loads_the_boot_block_and_jumps_to_it() {
    // auipc t0, 0: where it runs. Then stop with exit code 0.
    let block = boot_block(&[auipc(T0), HALT]);
    let mut m = machine(1, disk(&block, 8));
    assert_eq!(m.cpu.bus.peek(BOOT_ROM_BASE + 16), Some(DISK_BOOT_PC));
    assert_eq!(m.run(Some(STEPS), |_| {}), RunResult::Exited(0));
    assert_eq!(uart(&m), "", "nothing to say");
    let h = m.cpu.hart(0);
    assert_eq!(h.x[T0 as usize], LOAD_ADDR, "the last 64 KiB of RAM");
    assert_eq!(boot::boot_block_addr(RAM), LOAD_ADDR);
    assert_eq!(h.x[A0 as usize], 0, "the hart id");
    assert_eq!(h.x[A1 as usize], SYSINFO_BASE);
    assert_eq!(h.x[A2 as usize], BLK_BASE, "disk 0's registers");
    // The whole block is in RAM, and nothing after it.
    let at = (LOAD_ADDR - 0x8000_0000) as usize;
    assert_eq!(&m.cpu.bus.ram[at..at + BOOT_BLOCK_SIZE], &block[..]);
    assert!(m.cpu.bus.ram[at + BOOT_BLOCK_SIZE..].iter().all(|&b| b == 0));
    // The ROM's trap handler is still in place, and the disk's DONE acknowledged.
    assert!((DISK_BOOT_PC..BOOT_ROM_BASE + 0x1000).contains(&h.csr.mtvec));
    assert_eq!(m.cpu.bus.read(BLK_BASE + 0x18, 4), Ok(0));
}

#[test]
fn the_boot_area_follows_the_ram_size() {
    let block = boot_block(&[auipc(T0), HALT]);
    for mib in [1, 3, 128] {
        let config = MachineConfig {
            ram_size: mib << 20,
            disks: vec![DiskConfig::memory(disk(&block, 2))],
            ..MachineConfig::default()
        };
        let mut m = Machine::new(config).unwrap();
        m.boot_from_disk().unwrap();
        assert_eq!(m.run(Some(STEPS), |_| {}), RunResult::Exited(0));
        let want = 0x8000_0000 + (mib << 20) as u32 - 0x1_0000;
        assert_eq!(m.cpu.hart(0).x[T0 as usize], want, "{mib} MiB");
    }
}

#[test]
fn an_empty_disk_is_not_bootable() {
    let mut m = machine(1, vec![0; 4096]);
    refused(&mut m, "disk 0 is not bootable (no boot signature)");
}

#[test]
fn a_corrupt_boot_block_is_refused() {
    let mut block = boot_block(&[auipc(T0), HALT]);
    block[4] ^= 0x10; // one bit of the code
    let mut m = machine(1, disk(&block, 2));
    refused(&mut m, "the boot block on disk 0 is corrupt (bad checksum)");

    // Random bytes have no signature either.
    let noise: Vec<u8> = (0..2048u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    let mut m = machine(1, noise);
    refused(&mut m, "disk 0 is not bootable (no boot signature)");
}

#[test]
fn a_disk_of_one_sector_is_too_small() {
    let mut m = machine(1, vec![0; 512]);
    refused(&mut m, "disk 0 is too small to hold a boot block");
}

/// A disk whose reads all fail (an I/O error on the host side).
struct Broken;

impl BlockBackend for Broken {
    fn capacity(&self) -> u64 {
        16
    }
    fn read_only(&self) -> bool {
        false
    }
    fn read(&mut self, _: u64, _: &mut [u8]) -> std::io::Result<()> {
        Err(std::io::Error::other("bad sector"))
    }
    fn write(&mut self, _: u64, _: &[u8]) -> std::io::Result<()> {
        Err(std::io::Error::other("bad sector"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_read_error_is_reported() {
    let mut m = Machine::with_backends(MachineConfig::with_ram(RAM), vec![Box::new(Broken)]).unwrap();
    m.boot_from_disk().unwrap();
    m.cpu.bus.uart.capture = true;
    refused(&mut m, "cannot read the boot block from disk 0");
}

#[test]
fn a_trap_in_the_boot_block_is_reported() {
    // A signed block of zeros: its first word is an illegal instruction.
    let block = boot_block(&[]);
    let mut m = machine(1, disk(&block, 2));
    assert_eq!(m.run(Some(STEPS), |_| {}), RunResult::Exited(EXIT_BOOT_FAILED));
    assert_eq!(
        uart(&m),
        format!("boot ROM: unexpected trap, mcause 0x00000002, mepc {LOAD_ADDR:#010x}, mtval 0x00000000\n")
    );
}

#[test]
fn no_disk_no_disk_boot() {
    let mut m = Machine::new(MachineConfig::with_ram(RAM)).unwrap();
    assert!(m.boot_from_disk().unwrap_err().contains("no disk"));
}

/// The boot block of a two-hart machine points the ROM's entry word at code of its
/// own and wakes hart 1, as Camelot's bootloader does for its kernel: hart 1 leaves
/// the park loop for that code. An IPI that came before (here, from the test, before
/// anything ran) sends hart 1 back to the park loop instead of booting twice.
#[test]
fn the_boot_block_can_start_the_other_harts() {
    let block = boot_block(&[
        auipc(T0),          // 0x00
        addi(T0, T0, 0x20), //      t0 = hart 1's code, at 0x20
        lui(T1, 1),         //      t1 = the boot ROM
        sw(T0, T1, 16),     //      the entry word
        lui(T1, 0x2000),    //      the CLINT
        addi(A3, ZERO, 1),  //
        sw(A3, T1, 4),      //      msip of hart 1
        STOP,               // 0x1c
        addi(A1, A0, 0x10), // 0x20: hart 1, with a0 = 1
        HALT,
    ]);
    let mut m = machine(2, disk(&block, 2));
    m.cpu.bus.write(0x0200_0004, 4, 1).unwrap(); // an early IPI
    for _ in 0..40 {
        m.cpu.step();
    }
    let h1 = m.cpu.hart(1);
    assert!(
        h1.waiting && (PARK_PC..PARK_PC + 0x40).contains(&h1.pc),
        "parked again"
    );
    assert_eq!(m.cpu.bus.read(0x0200_0004, 4), Ok(0), "the early IPI was taken");

    assert_eq!(m.run(Some(STEPS), |_| {}), RunResult::Exited(0));
    assert_eq!(m.cpu.bus.peek(BOOT_ROM_BASE + 16), Some(LOAD_ADDR + 0x20));
    assert_eq!(
        m.cpu.hart(1).x[A1 as usize],
        0x11,
        "hart 1 ran the code with a0 = 1"
    );
    assert_eq!(m.cpu.hart(0).pc, LOAD_ADDR + 0x1c, "hart 0 waits at its j .");
}

// The `grail` binary, booting the Camelot kernel.

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("grail-disk-boot");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// A bootable root image: an empty filesystem of 1 MiB, the committed bootloader
/// and kernel.
fn bootable_image() -> Vec<u8> {
    let opts = camelot_fs::mkfs::Options {
        size: 1024,
        ..Default::default()
    };
    let mut image = camelot_fs::mkfs::mkfs(&opts, None).unwrap();
    let boot = std::fs::read(repo().join("kernel/boot/boot.elf")).unwrap();
    let kernel = std::fs::read(repo().join("kernel/kernel.elf")).unwrap();
    camelot_fs::boot::install(&mut image, &boot, &kernel).unwrap();
    image
}

/// Runs the `grail` binary with `args` and `stdin`; returns the exit code and stdout.
fn grail(args: &[&str], stdin: &[u8]) -> (Option<i32>, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_grail"))
        .args(["--max-steps", "200000000"])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("cannot run grail");
    let _ = child.stdin.take().unwrap().write_all(stdin);
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(!stderr.contains("step limit"), "hung:\n{stdout}{stderr}");
    (out.status.code(), stdout, stderr)
}

const SHELL: &str = "Type help for the list of commands.\n$ ";

#[test]
fn grail_boots_the_kernel_from_disk_with_no_program() {
    let img = scratch("root.img");
    let image = bootable_image();
    std::fs::write(&img, &image).unwrap();
    for harts in ["1", "2"] {
        let (code, out, _) = grail(&["--harts", harts, "--disk", img.to_str().unwrap()], b"exit\n");
        assert_eq!(code, Some(0), "{harts} harts:\n{out}");
        // The bootloader's line, then the kernel as if Grail had loaded its ELF.
        assert!(
            out.starts_with("boot: kernel at sector 0x00000800\nCamelot v0.1.0 booting...\n"),
            "{out}"
        );
        assert!(out.contains("fs: disk 0 mounted: 1024 blocks"), "{out}");
        assert!(out.ends_with(&format!("{SHELL}exit\n")), "{out}");
    }
    // The kernel leaves its disk as it found it (its self-test puts back what it
    // writes), the kernel included.
    assert_eq!(std::fs::read(&img).unwrap(), image);
}

#[test]
fn grail_refuses_a_disk_that_is_not_bootable() {
    let img = scratch("empty.img");
    std::fs::write(&img, vec![0u8; 64 * 1024]).unwrap();
    let (code, out, _) = grail(&["--disk", img.to_str().unwrap()], b"");
    assert_eq!(code, Some(EXIT_BOOT_FAILED as i32));
    assert_eq!(out, "boot ROM: disk 0 is not bootable (no boot signature)\n");

    // A filesystem with no bootloader: block 0 is empty.
    let plain = scratch("plain.img");
    std::fs::write(&plain, camelot_fs::mkfs::mkfs(&Default::default(), None).unwrap()).unwrap();
    let (code, out, _) = grail(&["--disk", plain.to_str().unwrap()], b"");
    assert_eq!(code, Some(EXIT_BOOT_FAILED as i32));
    assert_eq!(out, "boot ROM: disk 0 is not bootable (no boot signature)\n");

    // Neither a program nor a disk: nothing to run.
    let (code, _, err) = grail(&[], b"");
    assert_eq!(code, Some(2));
    assert!(
        err.contains("give a program to run, or a disk to boot from"),
        "{err}"
    );
}

/// What Camelot's bootloader says when the kernel it was told about is not there or
/// will not load; the boot block's fields are rewritten (and the block resealed).
#[test]
fn the_bootloader_reports_a_bad_kernel() {
    let good = bootable_image();
    fn reseal(image: &mut [u8], at: usize, v: u32) {
        image[at..at + 4].copy_from_slice(&v.to_le_bytes());
        let block: &mut [u8; BOOT_BLOCK_SIZE] = (&mut image[..BOOT_BLOCK_SIZE]).try_into().unwrap();
        boot::seal(block);
    }
    // The kernel's ELF header, and its first program header.
    const KERNEL_AT: usize = 2048 * 512;
    const PHDR: usize = KERNEL_AT + 52;
    // What the bootloader says first: where the kernel is.
    const AT_2048: &str = "boot: kernel at sector 0x00000800\n";
    // What is said before the error, the error, and how the image is damaged.
    type Case = (&'static str, &'static str, fn(&mut [u8]));
    let cases: Vec<Case> = vec![
        // Checked before it says anything.
        ("", "no kernel on this disk", |i| reseal(i, 1012, 0)),
        (
            "boot: kernel at sector 0x00100000\n",
            "disk read error 0x00000002", // out of range
            |i| reseal(i, 1008, 0x10_0000),
        ),
        (
            AT_2048,
            "the kernel is not an Excalibur ELF executable",
            |i| i[KERNEL_AT] = 0, // its ELF magic
        ),
        (
            AT_2048,
            "a kernel segment runs past the end of the kernel",
            |i| reseal(i, 1012, 8), // only 4 KiB of it
        ),
        (
            AT_2048,
            "a kernel segment is not in free RAM",
            |i| i[PHDR + 12..PHDR + 16].copy_from_slice(&0x1000u32.to_le_bytes()), // p_paddr
        ),
        (
            AT_2048,
            "a kernel segment is not at a sector boundary",
            |i| i[PHDR + 4..PHDR + 8].copy_from_slice(&0x1004u32.to_le_bytes()), // p_offset
        ),
    ];
    for (i, (first, message, damage)) in cases.iter().enumerate() {
        let mut image = good.clone();
        damage(&mut image);
        let config = MachineConfig {
            disks: vec![DiskConfig::memory(image)],
            ..MachineConfig::default()
        };
        let mut m = Machine::new(config).unwrap();
        m.boot_from_disk().unwrap();
        m.cpu.bus.uart.capture = true;
        assert_eq!(
            m.run(Some(20_000_000), |_| {}),
            RunResult::Exited(0xfc),
            "case {i}"
        );
        assert_eq!(uart(&m), format!("{first}boot: {message}\n"), "case {i}");
    }
}

/// The kernel is too big for a machine whose RAM ends just past it: its segments
/// would run into the bootloader.
#[test]
fn the_kernel_must_fit_below_the_bootloader() {
    let config = MachineConfig {
        ram_size: 512 << 10,
        disks: vec![DiskConfig::memory(bootable_image())],
        ..MachineConfig::default()
    };
    let mut m = Machine::new(config).unwrap();
    m.boot_from_disk().unwrap();
    m.cpu.bus.uart.capture = true;
    assert_eq!(m.run(Some(20_000_000), |_| {}), RunResult::Exited(0xfc));
    assert!(
        uart(&m).ends_with("boot: a kernel segment is not in free RAM\n"),
        "{}",
        uart(&m)
    );
}
