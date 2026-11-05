//! Booting Camelot from its disk (issue #69, spec section 7, "Boot from disk"):
//! the committed boot programs are what `cargo xtask boot` builds, and the root
//! image of `cargo xtask fs`, made bootable, boots the kernel on Grail with no
//! program, whose shell then runs programs from the disk's `/bin`. An RV64
//! machine boots an RV64 boot block built with merlin-as and merlin-ld through
//! the RV64 disk loader (#151; Camelot's RV64 bootloader and kernel boot from
//! disk in tests/kernel_rv64.rs, #154). The image
//! (`xtask::fs::IMAGE_SIZE` blocks, the size the spec gives) holds the disk kernel, which has no programs of its own (issue #108):
//! it boots in a machine of the default 4 MiB, and panics when there is no shell
//! to run. The RV64 root image (`cargo xtask fs --xlen 64`, #156) does the
//! same on an RV64 machine of the default 4 MiB, in an image of the same size.

use camelot_fs::mkfs::{Mkfs, Options};
use camelot_fs::{boot, fsck, read::Fs, BSIZE};
use grail::{DiskConfig, Machine, MachineConfig, RunResult};
use std::collections::BTreeSet;
use xtask::examples::Xlen;
use xtask::kernel::BOOT_MAX_STEPS;
use xtask::repo_root;

/// Boots `image` on a machine of the default hardware with no program, `input`
/// typed for the shell. Returns how it ended, what it printed, and the disk.
fn boot_image(image: Vec<u8>, input: &[u8]) -> (RunResult, String, Vec<u8>, Machine) {
    boot_image_on(Xlen::Rv32, image, input)
}

/// [`boot_image`] on an `xlen` machine: an RV64 one on Camelot's firmware,
/// which boots the disk, with its 2 MiB on top of the 4 MiB an RV32 one
/// has.
fn boot_image_on(xlen: Xlen, image: Vec<u8>, input: &[u8]) -> (RunResult, String, Vec<u8>, Machine) {
    let config = match xlen {
        Xlen::Rv32 => MachineConfig {
            disks: vec![DiskConfig::memory(image)],
            ..MachineConfig::default()
        },
        Xlen::Rv64 => MachineConfig {
            xlen: grail::Xlen::Rv64,
            bios: grail::Bios::Default,
            ram_size: 6 << 20,
            disks: vec![DiskConfig::memory(image)],
            ..MachineConfig::default()
        },
    };
    let mut m = Machine::new(config).unwrap();
    m.boot_from_disk().unwrap();
    m.cpu.bus.uart.capture = true;
    m.set_uart_input(xtask::kernel::typed(input));
    let result = m.run(Some(2 * BOOT_MAX_STEPS), |_| {});
    let out = String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
    let d = &mut m.cpu.bus.disks[0];
    let mut disk = vec![0; d.capacity() as usize * 512];
    d.backend().read(0, &mut disk).unwrap();
    (result, out, disk, m)
}

#[test]
fn committed_boot_programs_are_up_to_date() {
    let root = repo_root();
    for p in xtask::boot::PROGRAMS {
        let built = xtask::boot::build(&root, p).unwrap_or_else(|e| panic!("{e}"));
        assert!(built.warnings.is_empty(), "{:?}", built.warnings);
        let committed = std::fs::read(p.elf_path(&root)).unwrap();
        assert!(
            committed == built.elf,
            "{} is out of date: run `cargo xtask boot`",
            p.elf_name()
        );
    }
    // Grail's ROM is the committed loader of each width, and mkfs takes the
    // bootloader.
    assert_eq!(
        grail::boot::DISKBOOT_ELF,
        &std::fs::read(xtask::boot::DISKBOOT.elf_path(&root)).unwrap()[..]
    );
    assert_eq!(
        grail::boot::DISKBOOT_RV64_ELF,
        &std::fs::read(xtask::boot::DISKBOOT_RV64.elf_path(&root)).unwrap()[..]
    );
    let code = boot::boot_code(&xtask::boot::bootloader(&root).unwrap()).unwrap();
    assert!(code.len() <= boot::MAX_BOOT_CODE);
}

#[test]
fn the_commands_match_the_build() {
    assert_eq!(
        xtask::boot::BOOTLOADER.commands(),
        [
            "merlin -E -I include kernel/boot/boot.S -o target/boot/boot.s",
            "merlin-as target/boot/boot.s -o target/boot/boot.o",
            "merlin-ld -T kernel/boot/link.ld target/boot/boot.o -o kernel/boot/boot.elf"
        ]
    );
    assert_eq!(
        xtask::boot::DISKBOOT_RV64.commands(),
        [
            "merlin -E -march=rv64ima_zicsr_zifencei -I include crates/grail/rom/diskboot.S \
             -o target/boot/rv64/diskboot.s",
            "merlin-as -march=rv64ima_zicsr_zifencei target/boot/rv64/diskboot.s \
             -o target/boot/rv64/diskboot.o",
            "merlin-ld -T crates/grail/rom/link.ld target/boot/rv64/diskboot.o \
             -o crates/grail/rom/diskboot-rv64.elf"
        ]
    );
    // The command lines in the loader's comments are these, for both widths.
    let root = repo_root();
    for p in xtask::boot::PROGRAMS {
        let source = std::fs::read_to_string(root.join(p.dir).join(p.sources[0])).unwrap();
        for cmd in p.commands() {
            let cmd = cmd.replace(&format!("{}/", p.obj_dir()), "");
            assert!(
                source.contains(&format!("#   {cmd}\n")),
                "{}: {cmd}",
                p.sources[0]
            );
        }
    }
}

/// An RV64 boot block (#151's acceptance): built with merlin-as and merlin-ld,
/// sealed into sectors 0 and 1 of a disk, it boots an RV64 machine with no
/// program through the RV64 disk loader, and runs where the loader put it,
/// with what the loader passes in a0-a2 and the 64-bit registers of version
/// 2 of the devices (spec sections 7, 9.5 and 9.6).
const RV64_BOOT_BLOCK: &str = r#"
    .equ UART_BASE,    0x10000000
    .equ SYS_RAM_SIZE, 0x10             # 64-bit
    .equ BLK_CAPACITY, 0x00             # 64-bit
    .equ BOOT_AREA,    0x10000

    .text
    .globl _start
_start:
    auipc s0, 0                         # where the ROM put the block
    li    t0, BOOT_AREA                 # the boot area is the block's: a stack
    add   sp, s0, t0                    # at its end, the end of RAM
    mv    s1, a1                        # sysinfo
    mv    s2, a2                        # the boot disk's registers
    la    a0, s_at
    call  puts
    mv    a0, s0
    call  put_hex
    la    a0, s_hart
    call  puts
    csrr  a0, mhartid
    call  put_hex
    la    a0, s_ram
    call  puts
    ld    a0, SYS_RAM_SIZE(s1)
    call  put_hex
    la    a0, s_disk
    call  puts
    ld    a0, BLK_CAPACITY(s2)
    call  put_hex
    la    a0, s_sectors
    call  puts
    xcal.halt zero

# puts(a0 = NUL-terminated string)
puts:
    li    t0, UART_BASE
1:  lbu   t1, 0(a0)
    beqz  t1, 2f
    sb    t1, 0(t0)
    addi  a0, a0, 1
    j     1b
2:  ret

# put_hex(a0): "0x" and 16 hex digits.
put_hex:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    mv    s0, a0
    li    t0, UART_BASE
    li    t1, '0'
    sb    t1, 0(t0)
    li    t1, 'x'
    sb    t1, 0(t0)
    li    t2, 60                        # shift of the next digit
1:  srl   t1, s0, t2
    andi  t1, t1, 15
    li    t3, 10
    blt   t1, t3, 2f
    addi  t1, t1, 'a' - 10 - '0'
2:  addi  t1, t1, '0'
    sb    t1, 0(t0)
    addi  t2, t2, -4
    bgez  t2, 1b
    ld    s0, 0(sp)
    ld    ra, 8(sp)
    addi  sp, sp, 16
    ret

s_at:      .asciz "RV64 boot block at "
s_hart:    .asciz ", hart "
s_ram:     .asciz "\nRAM "
s_disk:    .asciz ", disk 0 of "
s_sectors: .asciz " sectors\n"
"#;

#[test]
fn an_rv64_boot_block_boots_through_the_rv64_rom() {
    // 4 GiB and 64 KiB of RAM: the boot area, the last 64 KiB, starts at
    // 0x1_8000_0000, past 4 GiB, which only a 64-bit SYS_RAM_SIZE, BLK_DMA and
    // pc reach.
    let ram: u64 = (4 << 30) + (64 << 10);
    let base = 0x8000_0000 + ram - u64::from(grail::boot::BOOT_AREA_SIZE);
    let obj = merlin_as::assemble_to_elf_with(
        RV64_BOOT_BLOCK,
        &merlin_as::Options {
            xlen: merlin_as::Xlen::Rv64,
        },
    )
    .unwrap();
    let opts = merlin_ld::Options {
        script: Some(merlin_ld::ScriptSource {
            name: "boot.ld".into(),
            text: format!("ENTRY(_start)\nSECTIONS {{ . = {base:#x}; .text : {{ *(.text*) }} }}\n"),
        }),
        entry: None,
    };
    let input = merlin_ld::Input {
        name: "boot.o".into(),
        data: obj,
    };
    let linked = merlin_ld::link(&[input], &opts).unwrap();
    assert!(linked.layout.warnings.is_empty(), "{:?}", linked.layout.warnings);
    let elf = grail::elf::parse(&linked.elf).unwrap();
    assert_eq!((elf.xlen, elf.entry), (grail::Xlen::Rv64, base));

    // The loaded bytes, at the start of a sealed boot block.
    let mut block = [0u8; grail::boot::BOOT_BLOCK_SIZE];
    for seg in &elf.segments {
        let at = (seg.paddr - base) as usize;
        assert!(
            at + seg.data.len() <= block.len() - 8,
            "the program fits the block"
        );
        block[at..at + seg.data.len()].copy_from_slice(&seg.data);
    }
    grail::boot::seal(&mut block);
    let mut disk = vec![0u8; 16 * 512];
    disk[..block.len()].copy_from_slice(&block);

    let config = MachineConfig {
        xlen: grail::Xlen::Rv64,
        ram_size: ram as usize,
        disks: vec![DiskConfig::memory(disk)],
        ..MachineConfig::default()
    };
    let mut m = Machine::new(config).unwrap();
    m.boot_from_disk().unwrap();
    m.cpu.bus.uart.capture = true;
    assert_eq!(m.run(Some(100_000), |_| {}), RunResult::Exited(0));
    assert_eq!(
        String::from_utf8_lossy(&m.cpu.bus.uart.captured),
        format!(
            "RV64 boot block at {base:#018x}, hart 0x0000000000000000\n\
             RAM {ram:#018x}, disk 0 of 0x0000000000000010 sectors\n"
        )
    );
}

#[test]
fn the_root_image_boots_from_disk_and_runs_programs_from_bin() {
    root_image_boots(Xlen::Rv32);
}

#[test]
fn the_rv64_root_image_boots_from_disk_and_runs_programs_from_bin() {
    root_image_boots(Xlen::Rv64);
}

/// The `xlen` root image, `cargo xtask fs`'s: its layout, the room left in
/// its filesystem, and a boot on an `xlen` machine of the default 4 MiB (RV64:
/// 6 MiB, on Camelot's firmware, which keeps 2 MiB),
/// whose shell runs programs from `/bin` and writes to the disk.
fn root_image_boots(xlen: Xlen) {
    let root = repo_root();
    let image = xtask::fs::build_bootable_for(&root, xtask::fs::IMAGE_SIZE, xlen).unwrap();
    let kernel = std::fs::read(root.join(xtask::fs::kernel_for(xlen))).unwrap();
    let opts = xtask::fs::bootable_layout(xtask::fs::IMAGE_SIZE, &kernel).unwrap();
    let plain = xtask::fs::build_for(&root, &opts, xlen).unwrap();

    // IMAGE_SIZE blocks in all: the filesystem, the same as without booting, and
    // the disk kernel after it, as much of it as the bootloader loads (no symbols).
    let size = xtask::fs::IMAGE_SIZE as usize;
    assert_eq!(image.len(), size * BSIZE);
    let loadable = boot::loadable(&kernel).unwrap();
    assert!(loadable.len() < kernel.len());
    assert_eq!(opts.size as usize, size - loadable.len().div_ceil(BSIZE));
    let info = boot::boot_info(&image).unwrap().expect("bootable");
    assert_eq!(info.kernel_sector, 2 * opts.size);
    assert_eq!(&image[BSIZE..plain.len()], &plain[BSIZE..]);
    assert_eq!(
        &boot::kernel_image(&image).unwrap()[..loadable.len()],
        &loadable[..]
    );
    let report = fsck::check(&image).unwrap();
    assert_eq!(report.files, fsck::check(&plain).unwrap().files);
    // Room for the shell to write files: at least a fifth of the filesystem free,
    // whatever the image's size, which is also far more than filetest's big
    // file (300 KiB, 300 blocks in one extent) needs. The
    // programs in /bin grow with every one added to kernel/user: when they take
    // too much, this says so, and xtask::fs::IMAGE_SIZE must grow.
    eprintln!(
        "the {} KiB {xlen:?} root image: a filesystem of {} blocks, {} free; a kernel of {} blocks",
        image.len() / 1024,
        report.sb.size,
        report.free_blocks,
        loadable.len().div_ceil(BSIZE)
    );
    assert!(
        report.free_blocks * 5 >= report.sb.size && report.free_blocks > 269,
        "the programs in /bin leave {} of the root filesystem's {} blocks free, less than a fifth:          make the image bigger (xtask::fs::IMAGE_SIZE)",
        report.free_blocks,
        report.sb.size
    );

    // The default machine: 4 MiB of RAM.
    assert_eq!(MachineConfig::default().ram_size, 4 << 20);
    let (result, out, disk, m) = boot_image_on(
        xlen,
        image,
        b"echo from the disk\nls /bin | wc > /count\ncat /count\nexit\n",
    );
    assert_eq!(result, RunResult::Exited(0), "{out}");
    assert!(
        out.starts_with(&format!(
            "boot: kernel at sector 0x{:08x}\n{}Camelot v0.1.0 booting...\n",
            info.kernel_sector,
            xtask::kernel::BANNER
        )),
        "{out}"
    );
    // RV64: the firmware's 2 MiB on top.
    let mib = match xlen {
        Xlen::Rv32 => 4,
        Xlen::Rv64 => 6,
    };
    assert!(
        out.contains(&format!("\nram: {mib} MiB at 0x80000000\n")),
        "{out}"
    );
    // On RV64 all but as and ld, which write RV32 code until #157.
    let programs: Vec<String> = xtask::userprogs::program_names(&xtask::userprogs::user_dir(&root))
        .unwrap()
        .into_iter()
        .filter(|p| xlen == Xlen::Rv32 || !xtask::userprogs::RV32_ONLY.contains(&p.as_str()))
        .collect();
    assert!(
        out.contains(&format!(
            "fs: /bin holds {} programs, the kernel none\n",
            programs.len()
        )),
        "{out}"
    );
    assert!(out.contains("fs self-test: 12/12 passed\n"), "{out}");
    // `ls /bin` lists `.`, `..` and the programs; its count went to a new file,
    // which is on the disk.
    let written = Fs::open(&disk).unwrap().read_file("/count").unwrap();
    let written = String::from_utf8(written).unwrap();
    assert!(
        written.starts_with(&format!("{} ", programs.len() + 2)),
        "{written}"
    );
    assert!(
        out.ends_with(&format!(
            "$ echo from the disk\nfrom the disk\n$ ls /bin | wc > /count\n$ cat /count\n{written}$ exit\n"
        )),
        "{out}"
    );
    // RV32: the bootloader pointed the ROM's entry word at the kernel's
    // physical entry point, so harts the kernel wakes with an IPI go there
    // from the park loop, as after an ELF boot. RV64: the bootloader ran in
    // S-mode, the firmware's next stage, and left the ROM alone (its entry
    // doubleword is the hand-off to the firmware, where the kernel's harts
    // wait for hart_start); the kernel is linked high and loaded at
    // 0x8020_0000.
    let k = elf::Elf::parse(&kernel).unwrap();
    let entry = k.header.entry;
    let entry = k
        .segments
        .iter()
        .find(|s| s.ty == 1 && (s.vaddr..s.vaddr + s.memsz).contains(&entry))
        .map(|s| s.paddr + (entry - s.vaddr))
        .unwrap();
    let lo = u64::from(m.cpu.bus.peek(grail::boot::BOOT_ENTRY_ADDR).unwrap());
    match xlen {
        Xlen::Rv32 => assert_eq!(lo, entry),
        Xlen::Rv64 => {
            let hi = u64::from(m.cpu.bus.peek(grail::boot::BOOT_ENTRY_ADDR + 4).unwrap());
            assert_eq!(lo | hi << 32, grail::boot::FW_HANDOFF_PC);
            assert_eq!(entry, grail::firmware::FW_JUMP_ADDR);
        }
    }
}

#[test]
fn the_image_size_is_the_whole_image_and_too_small_is_refused() {
    let root = repo_root();
    let kernel = std::fs::read(root.join(xtask::fs::KERNEL)).unwrap();
    let kernel_blocks = boot::loadable(&kernel).unwrap().len().div_ceil(BSIZE) as u32;
    // A bigger --size: a bigger filesystem, the same kernel.
    let image = xtask::fs::build_bootable(&root, 3000).unwrap();
    assert_eq!(image.len(), 3000 * BSIZE);
    assert_eq!(fsck::check(&image).unwrap().sb.size, 3000 - kernel_blocks);
    // The programs do not fit in 1 MiB, nor anything in the kernel's own size.
    let err = xtask::fs::build_bootable(&root, 1024).unwrap_err();
    assert!(
        err.starts_with(&format!(
            "the root filesystem does not fit in a 1024-block image, where the kernel leaves it {} blocks",
            1024 - kernel_blocks
        )) && err.ends_with("give a bigger --size"),
        "{err}"
    );
    let err = xtask::fs::build_bootable(&root, kernel_blocks).unwrap_err();
    assert_eq!(
        err,
        format!(
            "a {kernel_blocks}-block image has no room for a filesystem: the kernel alone takes \
             {kernel_blocks} blocks"
        )
    );
    // Too big is not cured by a bigger --size: the error says what is wrong
    // and does not suggest one.
    let err = xtask::fs::build_bootable(&root, u32::MAX).unwrap_err();
    assert!(
        err.starts_with(&format!("cannot build a {}-block root image: ", u32::MAX))
            && !err.contains("--size"),
        "{err}"
    );
}

/// The disk kernel has no programs of its own: with no shell in `/bin`, or no
/// root filesystem at all, it has nothing to run, and says so.
#[test]
fn the_disk_kernel_panics_without_a_shell() {
    let root = repo_root();
    let kernel = std::fs::read(root.join(xtask::fs::KERNEL)).unwrap();
    let boot = xtask::boot::bootloader(&root).unwrap();
    // A filesystem with an empty /bin.
    let mut fs = Mkfs::new(&Options {
        size: 200,
        ..Options::default()
    })
    .unwrap();
    fs.mkdir("/bin").unwrap();
    let mut image = fs.finish();
    boot::install(&mut image, &boot, &kernel).unwrap();
    let (result, out, _, _) = boot_image(image, b"exit\n");
    assert_eq!(result, RunResult::Exited(0xfe), "a panic:\n{out}");
    assert!(
        out.contains("fs: /bin holds 0 programs, the kernel none\n"),
        "{out}"
    );
    assert!(
        out.contains("fs self-test FAILED: /bin holds the programs\n"),
        "{out}"
    );
    assert!(
        out.ends_with(
            "cannot start /bin/sh: error 2\npanic: no shell in /bin, and no programs built into the kernel\n"
        ),
        "{out}"
    );

    // A /bin/sh that is there but is no program: the panic does not claim the
    // shell is missing, and the line before it gives exec's error.
    let mut fs = Mkfs::new(&Options {
        size: 200,
        ..Options::default()
    })
    .unwrap();
    fs.mkdir("/bin").unwrap();
    fs.add_file("/bin/sh", b"#!/not/an/elf\n").unwrap();
    let mut image = fs.finish();
    boot::install(&mut image, &boot, &kernel).unwrap();
    let (result, out, _, _) = boot_image(image, b"exit\n");
    assert_eq!(result, RunResult::Exited(0xfe), "a panic:\n{out}");
    assert!(
        out.ends_with(
            "cannot start /bin/sh: error 8\npanic: the shell in /bin cannot start, and no programs built into \
             the kernel\n"
        ),
        "{out}"
    );

    // Not a filesystem at all: the kernel is loaded as a program, with a blank disk.
    let config = MachineConfig {
        disks: vec![DiskConfig::memory(vec![0; 64 * 512])],
        ..MachineConfig::default()
    };
    let mut m = Machine::new(config).unwrap();
    m.cpu.bus.uart.capture = true;
    m.load_elf(&kernel).unwrap();
    m.set_uart_input(xtask::kernel::typed(b"exit\n"));
    let result = m.run(Some(BOOT_MAX_STEPS), |_| {});
    let out = String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
    assert_eq!(result, RunResult::Exited(0xfe), "{out}");
    assert!(
        out.ends_with(
            "fs: no filesystem on disk 0\ncannot start /bin/sh: error 2\npanic: no root filesystem, and no \
             programs built into the kernel\n"
        ),
        "{out}"
    );
}

/// The value of `.equ NAME, VALUE` in an assembly source.
fn equ(source: &str, name: &str) -> u32 {
    let line = source
        .lines()
        .find(|l| l.trim_start().starts_with(&format!(".equ {name},")))
        .unwrap_or_else(|| panic!("no .equ {name}"));
    let value = line.split(',').nth(1).unwrap().split('#').next().unwrap().trim();
    match value.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16).unwrap(),
        None => value.parse().unwrap(),
    }
}

/// Whether an assembly source has `.equ NAME, VALUE`.
fn has_equ(source: &str, name: &str) -> bool {
    source
        .lines()
        .any(|l| l.trim_start().starts_with(&format!(".equ {name},")))
}

/// The names the RV64 sources give version 2's device registers and their
/// values, and Grail's value of each: the sysinfo block's (spec section 9.5),
/// the disk's (9.6) and the network card's (9.8), with the bits and codes
/// read from them. Every name is defined by at least one RV64 source.
const V2_REGISTERS: &[(&str, u32)] = {
    use grail::block::{self, v2 as blk};
    use grail::net::{self, v2 as nic};
    use grail::sysinfo::{self, v2 as sys, DeviceType};
    &[
        ("SYSINFO_MAGIC", sysinfo::SYSINFO_MAGIC),
        ("SYS_MAGIC", sysinfo::REG_MAGIC),
        ("SYS_VERSION", sysinfo::REG_VERSION),
        ("SYS_RAM_BASE", sys::REG_RAM_BASE),
        ("SYS_RAM_SIZE", sys::REG_RAM_SIZE),
        ("SYS_TIMEBASE_HZ", sys::REG_TIMEBASE_HZ),
        ("SYS_CPU_HZ", sys::REG_CPU_HZ),
        ("SYS_FLAGS", sys::REG_FLAGS),
        ("SYS_DEV_COUNT", sys::REG_DEV_COUNT),
        ("SYS_HARTS", sys::REG_HARTS),
        ("SYS_DEV_TABLE", sysinfo::DEV_TABLE),
        ("DEV_ENTRY_SIZE", sys::DEV_ENTRY_SIZE),
        ("DEV_TYPE", sys::DEV_TYPE),
        ("DEV_IRQ", sys::DEV_IRQ),
        ("DEV_BASE", sys::DEV_BASE),
        ("DEV_SIZE", sys::DEV_SIZE),
        ("FLAG_REAL_TIME", sysinfo::FLAG_REAL_TIME),
        ("DEV_BLOCK", DeviceType::Block as u32),
        ("DEV_NET", DeviceType::Net as u32),
        ("BLK_CAPACITY", blk::REG_CAPACITY),
        ("BLK_SECTOR", blk::REG_SECTOR),
        ("BLK_DMA", blk::REG_DMA),
        ("BLK_COUNT", blk::REG_COUNT),
        ("BLK_FLAGS", blk::REG_FLAGS),
        ("BLK_CMD", blk::REG_CMD),
        ("BLK_STATUS", blk::REG_STATUS),
        ("BLK_ERROR", blk::REG_ERROR),
        ("BLK_CTRL", blk::REG_CTRL),
        ("CMD_READ", block::CMD_READ),
        ("CMD_WRITE", block::CMD_WRITE),
        ("CMD_FLUSH", block::CMD_FLUSH),
        ("STATUS_DONE", block::STATUS_DONE),
        ("FLAG_READ_ONLY", block::FLAG_READ_ONLY),
        ("ERR_OUT_OF_RANGE", block::error::OUT_OF_RANGE),
        ("SECTOR_SIZE", block::SECTOR_SIZE as u32),
        ("NET_MAC_LO", net::REG_MAC_LO),
        ("NET_MAC_HI", net::REG_MAC_HI),
        ("NET_STATUS", net::REG_STATUS),
        ("NET_IRQ_ENABLE", net::REG_IRQ_ENABLE),
        ("NET_IRQ_STATUS", net::REG_IRQ_STATUS),
        ("NET_TX_BASE", nic::REG_TX_BASE),
        ("NET_TX_SIZE", nic::REG_TX_SIZE),
        ("NET_TX_HEAD", nic::REG_TX_HEAD),
        ("NET_TX_TAIL", nic::REG_TX_TAIL),
        ("NET_RX_BASE", nic::REG_RX_BASE),
        ("NET_RX_SIZE", nic::REG_RX_SIZE),
        ("NET_RX_HEAD", nic::REG_RX_HEAD),
        ("NET_RX_TAIL", nic::REG_RX_TAIL),
        ("DESC_SIZE", nic::DESC_SIZE),
        ("IRQ_RX", net::IRQ_RX),
    ]
};

#[test]
fn the_rom_the_bootloader_mkfs_and_the_documents_agree() {
    let root = repo_root();
    let read = |path: &str| std::fs::read_to_string(root.join(path)).unwrap();
    // A `.S` source as merlin-as gets it at each width: preprocessed.
    let assembly = |path: &str, xlen| {
        xtask::examples::assembly_of(&root, path, &[xtask::examples::INCLUDE_DIR], xlen).unwrap()
    };
    let rom = assembly("crates/grail/rom/diskboot.S", Xlen::Rv32);
    let rom64 = assembly("crates/grail/rom/diskboot.S", Xlen::Rv64);
    let loader = assembly("kernel/boot/boot.S", Xlen::Rv32);
    let loader64 = assembly("kernel/boot/boot.S", Xlen::Rv64);
    let spec = read("docs/excalibur-spec.md");
    let fs_doc = read("docs/camelot-fs.md");

    // The boot block, as the ROM checks it and mkfs writes it.
    assert_eq!(grail::boot::BOOT_MAGIC, boot::BOOT_MAGIC);
    assert_eq!(equ(&rom, "BOOT_MAGIC"), boot::BOOT_MAGIC);
    assert_eq!(grail::boot::BOOT_BLOCK_SIZE, boot::BOOT_BLOCK_SIZE);
    assert_eq!(equ(&rom, "BOOT_SIZE") as usize, boot::BOOT_BLOCK_SIZE);
    assert_eq!(equ(&rom, "BOOT_SIG") as usize, boot::MAGIC_AT);
    assert_eq!(equ(&rom, "BOOT_AREA"), grail::boot::BOOT_AREA_SIZE);
    assert_eq!(equ(&rom, "EXIT_BOOT_FAILED"), grail::boot::EXIT_BOOT_FAILED);
    assert_eq!(u64::from(equ(&rom, "PARK_PC")), grail::bus::PARK_PC);
    // The two loaders agree on everything but the registers' layout.
    for name in [
        "PARK_PC",
        "RAM_BASE",
        "BOOT_AREA",
        "BOOT_SIZE",
        "BOOT_SECTORS",
        "BOOT_SIG",
        "BOOT_MAGIC",
        "EXIT_BOOT_FAILED",
        "UART_BASE",
        "SYSINFO_BASE",
        "DISK0",
        "CMD_READ",
        "STATUS_DONE",
    ] {
        assert_eq!(equ(&rom64, name), equ(&rom, name), "{name}");
    }
    // The RV64 loader's and the RV64 examples' device registers (their RV64
    // builds) are where version 2 of each interface has them (spec sections
    // 9.5, 9.6, 9.8).
    let mut rv64 = vec![("crates/grail/rom/diskboot.S".to_string(), rom64.clone())];
    for ex in xtask::examples::EXAMPLES {
        for s in ex.sources {
            let path = format!("{}/{s}", ex.dir_name());
            let text = assembly(&path, Xlen::Rv64);
            rv64.push((path, text));
        }
    }
    let mut unchecked: BTreeSet<&str> = V2_REGISTERS.iter().map(|&(name, _)| name).collect();
    for (path, source) in &rv64 {
        for &(name, value) in V2_REGISTERS {
            if has_equ(source, name) {
                assert_eq!(equ(source, name), value, "{path}: {name}");
                unchecked.remove(name);
            }
        }
    }
    assert!(unchecked.is_empty(), "no RV64 source defines {unchecked:?}");
    assert!(spec.contains("exit code 0xFD") && spec.contains("0x544F_4F42"));
    // The fields the bootloader reads are where mkfs writes them, at both
    // widths, and the ELF layouts each width's loader reads are the elf
    // crate's.
    for loader in [&loader, &loader64] {
        assert_eq!(equ(loader, "KERNEL_SECTOR") as usize, boot::KERNEL_SECTOR_AT);
        assert_eq!(equ(loader, "KERNEL_SECTORS") as usize, boot::KERNEL_SECTORS_AT);
        assert_eq!(equ(loader, "HEADERS_SIZE") as usize, boot::KERNEL_HEADERS);
        assert_eq!(u64::from(equ(loader, "BOOT_ENTRY")), grail::boot::BOOT_ENTRY_ADDR);
        assert_eq!(equ(loader, "RAM_BASE"), boot::RAM_BASE);
        assert_eq!(equ(loader, "EXIT_BOOT_FAILED"), 0xfc);
    }
    assert_eq!(equ(&loader, "PHDR_SIZE") as usize, elf::PHDR_SIZE);
    assert_eq!(equ(&loader64, "PHDR_SIZE") as usize, elf::PHDR64_SIZE);
    for (at, field) in [
        (boot::KERNEL_SECTOR_AT, "kernel_sector"),
        (boot::KERNEL_SECTORS_AT, "kernel_sectors"),
        (boot::CHECKSUM_AT, "checksum"),
        (boot::MAGIC_AT, "magic"),
    ] {
        assert!(fs_doc.contains(&format!("| {at} | `{field}` (u32) |")), "{field}");
    }
    assert!(fs_doc.contains(&format!("| 0 | code ({} bytes) |", boot::MAX_BOOT_CODE)));
    assert_eq!(equ(&loader, "EXIT_BOOT_FAILED"), 0xfc);
    assert!(fs_doc.contains("exit code 0xFC"));
}

#[test]
fn the_spec_gives_the_root_images_size() {
    // The root image's size is xtask::fs::IMAGE_SIZE, and the spec says it in
    // prose, once ("Root image"): every other document points there. The two
    // must agree.
    let spec = std::fs::read_to_string(repo_root().join("docs/excalibur-spec.md")).unwrap();
    let mib = spec
        .split("The image is **")
        .nth(1)
        .and_then(|rest| rest.split(" MiB in all**").next())
        .expect("the spec's \"Root image\" says: The image is **N MiB in all**");
    let blocks = xtask::fs::IMAGE_SIZE;
    assert_eq!(
        mib,
        format!("{}", blocks / 1024),
        "the spec says {mib} MiB, xtask::fs::IMAGE_SIZE is {blocks} blocks"
    );
    assert_eq!(blocks % 1024, 0, "a whole number of MiB");
}
