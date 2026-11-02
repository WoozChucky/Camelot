//! Booting Camelot from its disk (issue #69, spec section 7, "Boot from disk"):
//! the committed boot programs are what `cargo xtask boot` builds, and the root
//! image of `cargo xtask fs`, made bootable, boots the kernel on Grail with no
//! program, whose shell then runs programs from the disk's `/bin`. The image
//! (`xtask::fs::IMAGE_SIZE` blocks, the size the spec gives) holds the disk kernel, which has no programs of its own (issue #108):
//! it boots in a machine of the default 4 MiB, and panics when there is no shell
//! to run.

use camelot_fs::mkfs::{Mkfs, Options};
use camelot_fs::{boot, fsck, read::Fs, BSIZE};
use grail::{DiskConfig, Machine, MachineConfig, RunResult};
use xtask::kernel::BOOT_MAX_STEPS;
use xtask::repo_root;

/// Boots `image` on a machine of the default hardware with no program, `input`
/// typed for the shell. Returns how it ended, what it printed, and the disk.
fn boot_image(image: Vec<u8>, input: &[u8]) -> (RunResult, String, Vec<u8>, Machine) {
    let config = MachineConfig {
        disks: vec![DiskConfig::memory(image)],
        ..MachineConfig::default()
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
    let image = xtask::fs::build_bootable(&root, xtask::fs::IMAGE_SIZE).unwrap();
    let kernel = std::fs::read(root.join(xtask::fs::KERNEL)).unwrap();
    let opts = xtask::fs::bootable_layout(xtask::fs::IMAGE_SIZE, &kernel).unwrap();
    let plain = xtask::fs::build(&root, &opts).unwrap();

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
    // whatever the image's size, which is also far more than filetest's largest
    // file (274 432 bytes, 269 blocks with its indirect block) needs. The
    // programs in /bin grow with every one added to kernel/user: when they take
    // too much, this says so, and xtask::fs::IMAGE_SIZE must grow.
    eprintln!(
        "the {} KiB root image: a filesystem of {} blocks, {} free; a kernel of {} blocks",
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
    let (result, out, disk, m) = boot_image(
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
    assert!(out.contains("\nram: 4 MiB at 0x80000000\n"), "{out}");
    let programs = xtask::userprogs::program_names(&xtask::userprogs::user_dir(&root)).unwrap();
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
    // The bootloader pointed the ROM's entry word at the kernel, so harts the kernel
    // wakes with an IPI go there from the park loop, as after an ELF boot.
    let entry = elf32::Elf::parse(&kernel).unwrap().header.entry;
    assert_eq!(m.cpu.bus.peek(grail::boot::BOOT_ENTRY_ADDR), Some(entry));
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
