//! Booting from disk (spec section 7, "Boot from disk"): the boot ROM's disk loader
//! with hand-made boot blocks on in-memory disks (where the block lands, the registers
//! it starts with, every way the ROM refuses a disk, a trap in the boot block, the
//! writable entry point and the parked harts), on RV32 and RV64 machines, each with
//! the loader of its width, then the `grail` binary booting the Camelot kernel from an
//! image that `camelot-fs` made bootable with the committed bootloader
//! (`kernel/boot/boot.elf`) and kernel, with no program argument, and an RV64 machine
//! booting a hand-made RV64 boot block.
//!
//! Every run has a step limit: a ROM that hung instead of failing would hit it.

use grail::block::BlockBackend;
use grail::boot::{self, BOOT_BLOCK_SIZE, DISK_BOOT_PC, EXIT_BOOT_FAILED};
use grail::bus::{BLK_BASE, BOOT_ROM_BASE, PARK_PC, SYSINFO_BASE};
use grail::{DiskConfig, Machine, MachineConfig, RunResult, Xlen};
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

/// Both widths: each test runs on an RV32 and on an RV64 machine.
const XLENS: [Xlen; 2] = [Xlen::Rv32, Xlen::Rv64];

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
/// A store of `funct3`'s width: 2 for `sw`, 3 for `sd`.
fn store(funct3: u32, rs2: u32, rs1: u32, imm: u32) -> u32 {
    (imm >> 5) << 25 | rs2 << 20 | rs1 << 15 | funct3 << 12 | (imm & 31) << 7 | 0x23
}
fn sw(rs2: u32, rs1: u32, imm: u32) -> u32 {
    store(2, rs2, rs1, imm)
}
/// A store of a register's whole width: `sw` on RV32, `sd` on RV64. The ROM's entry
/// point takes only that one (spec section 7).
fn sx(xlen: Xlen, rs2: u32, rs1: u32, imm: u32) -> u32 {
    match xlen {
        Xlen::Rv32 => store(2, rs2, rs1, imm),
        Xlen::Rv64 => store(3, rs2, rs1, imm),
    }
}
/// `j .`
const STOP: u32 = 0x0000_006f;
/// `xcal.halt zero`: exit code 0.
const HALT: u32 = 0x0000_100b;

/// The width of the ROM's entry point and of a register, in bytes.
fn width(xlen: Xlen) -> u32 {
    match xlen {
        Xlen::Rv32 => 4,
        Xlen::Rv64 => 8,
    }
}

/// `BLK_STATUS`: where the disk's registers put it (spec section 9.6: version 1 on
/// RV32 machines, 2 on RV64 ones).
fn blk_status(xlen: Xlen) -> u64 {
    match xlen {
        Xlen::Rv32 => 0x18,
        Xlen::Rv64 => 0x24,
    }
}

/// A register as wide as the machine: an RV32 hart's low 32 bits (it keeps them
/// sign-extended), an RV64 hart's whole register.
fn reg(m: &Machine, hart: usize, r: u32) -> u64 {
    let v = m.cpu.hart(hart).x[r as usize];
    match m.cpu.xlen() {
        Xlen::Rv32 => v & 0xffff_ffff,
        Xlen::Rv64 => v,
    }
}

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

/// An `xlen` machine of `harts` harts with `image` as disk 0, set to boot from it,
/// its UART captured.
fn machine(xlen: Xlen, harts: usize, image: Vec<u8>) -> Machine {
    let config = MachineConfig {
        xlen,
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
    assert_eq!(m.cpu.xlen(), xlen);
    assert_eq!(m.cpu.bus.read(BOOT_ROM_BASE + 16, 8), Ok(DISK_BOOT_PC));
    let loader = boot::disk_loader(xlen);
    let at = DISK_BOOT_PC as usize - BOOT_ROM_BASE as usize;
    assert_eq!(
        &m.cpu.bus.rom[at..],
        &loader[..],
        "{xlen:?}: the loader of its width"
    );
    assert_eq!(m.run(Some(STEPS), |_| {}), RunResult::Exited(0));
    assert_eq!(uart(&m), "", "nothing to say");
    assert_eq!(
        reg(&m, 0, T0),
        LOAD_ADDR.into(),
        "{xlen:?}: the last 64 KiB of RAM"
    );
    assert_eq!(boot::boot_block_addr(RAM), Some(LOAD_ADDR.into()));
    assert_eq!(reg(&m, 0, A0), 0, "the hart id");
    assert_eq!(reg(&m, 0, A1), SYSINFO_BASE);
    assert_eq!(reg(&m, 0, A2), BLK_BASE, "disk 0's registers");
    // The whole block is in RAM, and nothing after it.
    let at = (LOAD_ADDR - 0x8000_0000) as usize;
    assert_eq!(&m.cpu.bus.ram[at..at + BOOT_BLOCK_SIZE], &block[..]);
    assert!(m.cpu.bus.ram[at + BOOT_BLOCK_SIZE..].iter().all(|&b| b == 0));
    // The ROM's trap handler is still in place, and the disk's DONE acknowledged.
    let h = m.cpu.hart(0);
    assert!((DISK_BOOT_PC..BOOT_ROM_BASE + 0x1000).contains(&h.csr.mtvec));
    assert_eq!(m.cpu.bus.read(BLK_BASE + BLK_STATUS, 4), Ok(0));
}

#[test]
fn the_boot_area_follows_the_ram_size() {
    let block = boot_block(&[auipc(T0), HALT]);
    // RV64 reads the 64-bit SYS_RAM_SIZE: 5 GiB puts the block past 4 GiB.
    for (xlen, mib) in [
        (Xlen::Rv32, 1),
        (Xlen::Rv32, 3),
        (Xlen::Rv32, 128),
        (Xlen::Rv64, 1),
        (Xlen::Rv64, 3),
        (Xlen::Rv64, 128),
        (Xlen::Rv64, 5 << 10),
    ] {
        let config = MachineConfig {
            ram_size: mib << 20,
            disks: vec![DiskConfig::memory(disk(&block, 2))],
            ..MachineConfig::default()
        };
        let mut m = Machine::new(config).unwrap();
        m.boot_from_disk().unwrap();
        assert_eq!(m.run(Some(STEPS), |_| {}), RunResult::Exited(0));
        let want = 0x8000_0000 + ((mib as u64) << 20) - 0x1_0000;
        assert_eq!(reg(&m, 0, T0), want, "{xlen:?}, {mib} MiB");
        assert_eq!(boot::boot_block_addr(mib << 20), Some(want));
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

/// The checksum is modulo 2^32 at both widths. Two words of 0x8000_0000 (data after
/// the code) add up to 0 in 32 bits, but to -2^32 once RV64's `lw` has sign-extended
/// them: the RV64 loader adds with `addw`, and boots the block.
#[test]
fn the_checksum_is_32_bit_on_both_widths() {
    let block = boot_block(&[auipc(T0), HALT, 0x8000_0000, 0x8000_0000]);
    let mut m = machine(1, disk(&block, 2));
    assert_eq!(m.run(Some(STEPS), |_| {}), RunResult::Exited(0));
    assert_eq!(uart(&m), "");
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
    let config = MachineConfig {
        xlen,
        ..MachineConfig::with_ram(RAM)
    };
    let mut m = Machine::with_backends(config, vec![Box::new(Broken)]).unwrap();
    m.boot_from_disk().unwrap();
    m.cpu.bus.uart.capture = true;
    refused(&mut m, "cannot read the boot block from disk 0");
}

#[test]
fn a_trap_in_the_boot_block_is_reported() {
    // A signed block of zeros: its first word is an illegal instruction. The values
    // are as wide as the machine's registers.
    let block = boot_block(&[]);
    let mut m = machine(Xlen::Rv32, 1, disk(&block, 2));
    assert_eq!(m.run(Some(STEPS), |_| {}), RunResult::Exited(EXIT_BOOT_FAILED));
    assert_eq!(
        uart(&m),
        format!("boot ROM: unexpected trap, mcause 0x00000002, mepc {LOAD_ADDR:#010x}, mtval 0x00000000\n")
    );
    let mut m = machine(Xlen::Rv64, 1, disk(&block, 2));
    assert_eq!(m.run(Some(STEPS), |_| {}), RunResult::Exited(EXIT_BOOT_FAILED));
    assert_eq!(
        uart(&m),
        format!(
            "boot ROM: unexpected trap, mcause 0x0000000000000002, mepc {LOAD_ADDR:#018x}, \
             mtval 0x0000000000000000\n"
        )
    );
}

#[test]
fn no_disk_no_disk_boot() {
    let config = MachineConfig {
        xlen,
        ..MachineConfig::with_ram(RAM)
    };
    let mut m = Machine::new(config).unwrap();
    assert!(m.boot_from_disk().unwrap_err().contains("no disk"));
}

/// The boot area is the last 64 KiB of RAM: a smaller machine cannot boot from disk,
/// and says so before it runs (the ROM would read the block below RAM and fail with a
/// read error that hides why).
#[test]
fn disk_boot_takes_64_kib_of_ram() {
    let block = boot_block(&[auipc(T0), HALT]);
    for kib in [4, 32, 60] {
        let config = MachineConfig {
            ram_size: kib << 10,
            disks: vec![DiskConfig::memory(disk(&block, 2))],
            ..MachineConfig::default()
        };
        let mut m = Machine::new(config).unwrap();
        let err = m.boot_from_disk().unwrap_err();
        assert!(err.contains("at least 64 KiB of RAM"), "{kib} KiB: {err}");
    }
    let config = MachineConfig {
        xlen,
        ram_size: 64 << 10,
        disks: vec![DiskConfig::memory(disk(&block, 2))],
        ..MachineConfig::default()
    };
    let mut m = Machine::new(config).unwrap();
    m.boot_from_disk().unwrap();
    assert_eq!(m.run(Some(STEPS), |_| {}), RunResult::Exited(0));
    assert_eq!(reg(&m, 0, T0), 0x8000_0000, "all of RAM is the boot area");
}

/// The boot block of a two-hart machine points the ROM's entry point at code of its
/// own and wakes hart 1, as Camelot's bootloader does for its kernel: hart 1 leaves
/// the park loop for that code. An IPI that came before (here, from the test, before
/// anything ran) sends hart 1 back to the park loop instead of booting twice. The
/// entry point is a word on RV32 and a doubleword on RV64, written whole.
#[test]
fn the_boot_block_can_start_the_other_harts() {
    let block = boot_block(&[
        auipc(T0),          // 0x00
        addi(T0, T0, 0x20), //      t0 = hart 1's code, at 0x20
        lui(T1, 1),         //      t1 = the boot ROM
        sd(T0, T1, 16),     //    the entry point
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
        "{xlen:?}: parked again"
    );
    assert_eq!(m.cpu.bus.read(0x0200_0004, 4), Ok(0), "the early IPI was taken");

    assert_eq!(m.run(Some(STEPS), |_| {}), RunResult::Exited(0));
    let entry = u64::from(LOAD_ADDR + 0x20);
    assert_eq!(m.cpu.bus.read(BOOT_ROM_BASE + 16, 8), Ok(entry), "{xlen:?}");
    assert_eq!(reg(&m, 1, A1), 0x11, "hart 1 ran the code with a0 = 1");
    assert_eq!(
        m.cpu.hart(0).pc,
        u64::from(LOAD_ADDR + 0x1c),
        "hart 0 waits at its j ."
    );
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
            out.starts_with("boot: kernel at sector 0x00000800\n\n    ____ "),
            "{out}"
        );
        assert!(out.contains("|_|\n\nCamelot v0.1.0 booting...\n"), "{out}");
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

/// `grail --xlen 64 --disk IMG`: an RV64 machine with no program boots through the
/// RV64 loader. The boot block reads sysinfo's 64-bit `SYS_RAM_SIZE` with `ld` and
/// stops with the RAM's size in MiB as the exit code. On an RV32 machine the same block
/// is not code (`ld` is an RV64 instruction): the RV32 loader reports the trap.
#[test]
fn grail_boots_an_rv64_machine_from_disk() {
    const LD_T1_RAM_SIZE: u32 = 0x10 << 20 | A1 << 15 | 3 << 12 | T1 << 7 | 0x03; // ld t1, 16(a1)
    const SRLI_T1_20: u32 = 20 << 20 | T1 << 15 | 5 << 12 | T1 << 7 | 0x13; // srli t1, t1, 20
    const HALT_T1: u32 = T1 << 15 | HALT; // xcal.halt t1
    let img = scratch("rv64.img");
    let block = boot_block(&[LD_T1_RAM_SIZE, SRLI_T1_20, HALT_T1]);
    std::fs::write(&img, disk(&block, 2)).unwrap();
    let img = img.to_str().unwrap();
    for (args, ram) in [
        (vec!["--xlen", "64"], 4),
        (vec!["--xlen", "64", "--ram", "64"], 64),
    ] {
        let (code, out, err) = grail(&[&args[..], &["--disk", img]].concat(), b"");
        assert_eq!(code, Some(ram), "{args:?}:\n{out}{err}");
        assert_eq!(out, "");
    }
    let (code, out, _) = grail(&["--disk", img], b"");
    assert_eq!(code, Some(EXIT_BOOT_FAILED as i32));
    assert_eq!(
        out,
        format!(
            "boot ROM: unexpected trap, mcause 0x00000002, mepc 0x803f0000, mtval {LD_T1_RAM_SIZE:#010x}\n"
        )
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

/// `bnez rs1, offset` (`bne rs1, zero, offset`), for a small forward offset.
fn bnez(rs1: u32, offset: u32) -> u32 {
    assert!(offset < 0x800 && offset.is_multiple_of(2));
    (offset >> 5 & 0x3f) << 25 | rs1 << 15 | 1 << 12 | (offset & 0x1e) << 7 | 0x63
}

/// A minimal ELF32 RISC-V executable: the header, `phdrs` program headers (offset,
/// paddr, filesz, memsz) right after it, then `data` placed at each file offset given.
fn kernel_elf(entry: u32, phdrs: &[(u32, u32, u32, u32)], data: &[(usize, &[u8])], size: usize) -> Vec<u8> {
    let mut f = vec![0u8; size];
    f[..7].copy_from_slice(b"ELF");
    f[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    f[18..20].copy_from_slice(&243u16.to_le_bytes()); // EM_RISCV
    f[20..24].copy_from_slice(&1u32.to_le_bytes());
    f[24..28].copy_from_slice(&entry.to_le_bytes());
    f[28..32].copy_from_slice(&52u32.to_le_bytes()); // e_phoff
    f[40..42].copy_from_slice(&52u16.to_le_bytes());
    f[42..44].copy_from_slice(&32u16.to_le_bytes());
    f[44..46].copy_from_slice(&(phdrs.len() as u16).to_le_bytes());
    for (i, &(offset, paddr, filesz, memsz)) in phdrs.iter().enumerate() {
        let at = 52 + 32 * i;
        for (j, v) in [1, offset, paddr, paddr, filesz, memsz, 7, 0x1000]
            .iter()
            .enumerate()
        {
            f[at + 4 * j..at + 4 * j + 4].copy_from_slice(&v.to_le_bytes());
        }
    }
    for &(at, bytes) in data {
        f[at..at + bytes.len()].copy_from_slice(bytes);
    }
    f
}

/// Camelot's real bootloader with a small kernel of our own, on a two-hart machine
/// whose RAM is filled with 0xee first (a reset leaves zeros, which would hide a
/// bootloader that did not zero .bss). The bootloader must copy exactly the file
/// bytes (a partial last sector included, nothing after it), zero .bss (also for a
/// .bss-only segment at a file offset that is not a multiple of 512, as merlin-ld
/// places one), write the entry point to the ROM's entry word, and start the kernel
/// on hart 0, which wakes hart 1: hart 1 leaves the park loop for the kernel too.
#[test]
fn the_bootloader_loads_segments_and_hands_the_entry_word_over() {
    const BASE: u32 = 0x8000_0000;
    // The kernel's code: hart 0 sends hart 1 an IPI and waits; hart 1 stops the
    // machine with exit code 0 and a1 = 0x10 + its hart id.
    let code: Vec<u8> = [
        bnez(A0, 0x14),     // 0x00: hart 1 goes to 0x14
        lui(T1, 0x2000),    //       t1 = the CLINT
        addi(A3, ZERO, 1),  //
        sw(A3, T1, 4),      //       msip of hart 1
        STOP,               // 0x10
        addi(A1, A0, 0x10), // 0x14: hart 1
        HALT,
    ]
    .iter()
    .flat_map(|w| w.to_le_bytes())
    .collect();
    // Segment 1: code and data, one sector and a bit (0x234 bytes) at 0x200 in the
    // file, then .bss up to 0x400. Segment 2: 0x10 bytes, all in a tail sector whose
    // other bytes (0x55) must stay on the disk. Segment 3: .bss only, 0x40 bytes.
    let text: Vec<u8> = code
        .iter()
        .copied()
        .chain((code.len()..0x234).map(|i| i as u8 | 0x80))
        .collect();
    let data: Vec<u8> = (0..0x10).map(|i| 0x10 + i as u8).collect();
    let after = [0x55u8; 0x1f0];
    let elf = kernel_elf(
        BASE,
        &[
            (0x200, BASE, 0x234, 0x400),
            (0x600, BASE + 0x2000, 0x10, 0x10),
            (0x610, BASE + 0x3010, 0, 0x40),
        ],
        &[(0x200, &text), (0x600, &data), (0x610, &after)],
        0x800,
    );
    let mut image = camelot_fs::mkfs::mkfs(
        &camelot_fs::mkfs::Options {
            size: 1024,
            ..Default::default()
        },
        None,
    )
    .unwrap();
    let boot = std::fs::read(repo().join("kernel/boot/boot.elf")).unwrap();
    camelot_fs::boot::install(&mut image, &boot, &elf).unwrap();

    let mut m = machine(Xlen::Rv32, 2, image);
    m.cpu.bus.ram[..0x4000].fill(0xee);
    assert_eq!(
        m.run(Some(1_000_000), |_| {}),
        RunResult::Exited(0),
        "{}",
        uart(&m)
    );
    assert_eq!(
        uart(&m),
        "boot: kernel at sector 0x00000800
"
    );

    let ram = &m.cpu.bus.ram;
    let all = |range: std::ops::Range<usize>, b: u8| ram[range].iter().all(|&x| x == b);
    assert_eq!(&ram[..0x234], &text[..], "segment 1's file bytes");
    assert!(all(0x234..0x400, 0), "segment 1's .bss");
    assert!(all(0x400..0x2000, 0xee), "nothing past segment 1");
    assert_eq!(&ram[0x2000..0x2010], &data[..], "segment 2");
    assert!(
        all(0x2010..0x3010, 0xee),
        "nothing of segment 2's sector past its bytes"
    );
    assert!(all(0x3010..0x3050, 0), "segment 3, .bss only");
    assert!(all(0x3050..0x4000, 0xee), "nothing past segment 3");

    // The entry word points at the kernel, and hart 1 got there through it.
    assert_eq!(m.cpu.bus.peek(BOOT_ROM_BASE + 16), Some(BASE));
    let (h0, h1) = (m.cpu.hart(0), m.cpu.hart(1));
    assert_eq!(h0.pc, u64::from(BASE + 0x10), "hart 0 waits at its j .");
    assert_eq!((h0.x[A0 as usize], h0.x[A1 as usize]), (0, SYSINFO_BASE));
    assert_eq!(h1.x[A1 as usize], 0x11, "hart 1 ran the kernel with a0 = 1");
}
