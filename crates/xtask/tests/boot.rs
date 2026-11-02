//! Booting Camelot from its disk (issue #69, spec section 7, "Boot from disk"):
//! the committed boot programs are what `cargo xtask boot` builds, and the root
//! image of `cargo xtask fs`, made bootable, boots the kernel on Grail with no
//! program, whose shell then runs programs from the disk's `/bin`.

use camelot_fs::{boot, fsck, BSIZE};
use grail::{DiskConfig, Machine, MachineConfig, RunResult};
use xtask::kernel::BOOT_MAX_STEPS;
use xtask::repo_root;

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
    // Grail's ROM is the committed loader, and mkfs takes the bootloader.
    assert_eq!(
        grail::boot::DISKBOOT_ELF,
        &std::fs::read(xtask::boot::DISKBOOT.elf_path(&root)).unwrap()[..]
    );
    let code = boot::boot_code(&xtask::boot::bootloader(&root).unwrap()).unwrap();
    assert!(code.len() <= boot::MAX_BOOT_CODE);
}

#[test]
fn the_commands_match_the_build() {
    assert_eq!(
        xtask::boot::BOOTLOADER.commands(),
        [
            "merlin-as kernel/boot/boot.s -o target/boot/boot.o",
            "merlin-ld -T kernel/boot/link.ld target/boot/boot.o -o kernel/boot/boot.elf"
        ]
    );
}

#[test]
fn the_root_image_boots_from_disk_and_runs_programs_from_bin() {
    let root = repo_root();
    let opts = camelot_fs::mkfs::Options::default();
    let image = xtask::fs::build_bootable(&root, &opts).unwrap();
    let plain = xtask::fs::build(&root, &opts).unwrap();
    let kernel = std::fs::read(root.join(xtask::fs::KERNEL)).unwrap();

    // The filesystem is the same as without booting; the kernel follows it.
    let info = boot::boot_info(&image).unwrap().expect("bootable");
    assert_eq!(info.kernel_sector, 2 * opts.size);
    assert_eq!(&image[BSIZE..plain.len()], &plain[BSIZE..]);
    assert_eq!(&boot::kernel_image(&image).unwrap()[..kernel.len()], &kernel[..]);
    assert_eq!(
        fsck::check(&image).unwrap().files,
        fsck::check(&plain).unwrap().files
    );

    let config = MachineConfig {
        disks: vec![DiskConfig::memory(image)],
        ..MachineConfig::default()
    };
    let mut m = Machine::new(config).unwrap();
    m.boot_from_disk().unwrap();
    m.cpu.bus.uart.capture = true;
    m.set_uart_input(xtask::kernel::typed(b"echo from the disk\nexit\n"));
    let result = m.run(Some(2 * BOOT_MAX_STEPS), |_| {});
    let out = String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
    assert_eq!(result, RunResult::Exited(0), "{out}");
    assert!(
        out.starts_with(&format!(
            "boot: kernel at sector 0x00001000\n{}Camelot v0.1.0 booting...\n",
            xtask::kernel::BANNER
        )),
        "{out}"
    );
    assert!(out.contains("fs: /bin holds "), "{out}");
    assert!(
        out.ends_with("$ echo from the disk\nfrom the disk\n$ exit\n"),
        "{out}"
    );
    // The bootloader pointed the ROM's entry word at the kernel, so harts the kernel
    // wakes with an IPI go there from the park loop, as after an ELF boot.
    let entry = elf32::Elf::parse(&kernel).unwrap().header.entry;
    assert_eq!(m.cpu.bus.peek(grail::boot::BOOT_ENTRY_ADDR), Some(entry));
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

#[test]
fn the_rom_the_bootloader_mkfs_and_the_documents_agree() {
    let root = repo_root();
    let read = |path: &str| std::fs::read_to_string(root.join(path)).unwrap();
    let rom = read("crates/grail/rom/diskboot.s");
    let loader = read("kernel/boot/boot.s");
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
    assert_eq!(equ(&rom, "PARK_PC"), grail::bus::PARK_PC);
    assert!(spec.contains("exit code 0xFD") && spec.contains("0x544F_4F42"));
    // The fields the bootloader reads are where mkfs writes them.
    assert_eq!(equ(&loader, "KERNEL_SECTOR") as usize, boot::KERNEL_SECTOR_AT);
    assert_eq!(equ(&loader, "KERNEL_SECTORS") as usize, boot::KERNEL_SECTORS_AT);
    assert_eq!(equ(&loader, "HEADERS_SIZE") as usize, boot::KERNEL_HEADERS);
    assert_eq!(equ(&loader, "BOOT_ENTRY"), grail::boot::BOOT_ENTRY_ADDR);
    assert_eq!(equ(&loader, "RAM_BASE"), boot::RAM_BASE);
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
