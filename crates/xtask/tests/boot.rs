//! Booting Camelot from its disk (issue #69, spec section 7, "Boot from disk"):
//! the committed boot programs are what `cargo xtask boot` builds, and the root
//! image of `cargo xtask fs`, made bootable, boots the kernel on Grail with no
//! program, through Camelot's firmware, whose shell then runs programs from the
//! disk's `/bin`. A machine without firmware boots a boot block built with
//! merlin-as and merlin-ld through the boot ROM's disk loader (#151). The image
//! (`xtask::fs::IMAGE_SIZE` blocks, the size the spec gives) holds the disk
//! kernel, which has no programs of its own (issue #108): it boots on a Camelot
//! VM's hardware (the firmware and `avalon_client::CAMELOT_RAM`, the default
//! 4 MiB and the firmware's 2 MiB), and panics when there is no shell to run.

use camelot_fs::mkfs::{Mkfs, Options};
use camelot_fs::{boot, fsck, read::Fs, BSIZE};
use grail::{DiskConfig, Machine, MachineConfig, RunResult};
use std::collections::BTreeSet;
use xtask::kernel::BOOT_MAX_STEPS;
use xtask::repo_root;

/// A Camelot VM's machine (Pendragon's and `cargo xtask up`'s): Camelot's
/// firmware, which boots the disk, and its 2 MiB on top of the default 4 MiB.
fn camelot_vm(disks: Vec<DiskConfig>) -> MachineConfig {
    MachineConfig {
        bios: grail::Bios::Default,
        ram_size: avalon_client::CAMELOT_RAM as usize,
        disks,
        ..MachineConfig::default()
    }
}

/// Boots `image` on a Camelot VM's machine ([`camelot_vm`]) with no program,
/// `input` typed for the shell. Returns how it ended, what it printed, and the
/// disk.
fn boot_image(image: Vec<u8>, input: &[u8]) -> (RunResult, String, Vec<u8>, Machine) {
    let mut m = Machine::new(camelot_vm(vec![DiskConfig::memory(image)])).unwrap();
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
    // Grail's ROM is the committed loader, its firmware the committed
    // firmware, and mkfs takes the bootloader.
    assert_eq!(
        grail::boot::DISKBOOT_ELF,
        &std::fs::read(xtask::boot::DISKBOOT.elf_path(&root)).unwrap()[..]
    );
    assert_eq!(
        grail::firmware::FIRMWARE_ELF,
        &std::fs::read(xtask::boot::FIRMWARE.elf_path(&root)).unwrap()[..]
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
        xtask::boot::DISKBOOT.commands(),
        [
            "merlin -E -I include crates/grail/rom/diskboot.S -o target/boot/diskboot.s",
            "merlin-as target/boot/diskboot.s -o target/boot/diskboot.o",
            "merlin-ld -T crates/grail/rom/link.ld target/boot/diskboot.o -o crates/grail/rom/diskboot.elf"
        ]
    );
    // The command lines in the programs' comments are these.
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

/// A boot block (#151's acceptance): built with merlin-as and merlin-ld,
/// sealed into sectors 0 and 1 of a disk, it boots a machine with no program
/// and no firmware through the boot ROM's disk loader, and runs where the
/// loader put it, with what the loader passes in a0-a2 and the devices'
/// 64-bit registers (spec sections 7, 9.5 and 9.6).
const BOOT_BLOCK: &str = r#"
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

s_at:      .asciz "boot block at "
s_hart:    .asciz ", hart "
s_ram:     .asciz "\nRAM "
s_disk:    .asciz ", disk 0 of "
s_sectors: .asciz " sectors\n"
"#;

#[test]
fn a_boot_block_boots_through_the_rom() {
    // 4 GiB and 64 KiB of RAM: the boot area, the last 64 KiB, starts at
    // 0x1_8000_0000, past 4 GiB, which only a 64-bit SYS_RAM_SIZE, BLK_DMA and
    // pc reach.
    let ram: u64 = (4 << 30) + (64 << 10);
    let base = 0x8000_0000 + ram - u64::from(grail::boot::BOOT_AREA_SIZE);
    let obj = merlin_as::assemble_to_elf(BOOT_BLOCK).unwrap();
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
    assert_eq!(elf.entry, base);

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
            "boot block at {base:#018x}, hart 0x0000000000000000\n\
             RAM {ram:#018x}, disk 0 of 0x0000000000000010 sectors\n"
        )
    );
}

#[test]
fn the_root_image_boots_from_disk_and_runs_programs_from_bin() {
    // The root image, `cargo xtask fs`'s: its layout, the room left in its
    // filesystem, and a boot on a Camelot VM's machine (6 MiB, on Camelot's
    // firmware, which keeps 2 MiB), whose shell runs programs from `/bin` and
    // writes to the disk.
    let root = repo_root();
    let image = xtask::fs::build_bootable(&root, xtask::fs::Profile::Server, xtask::fs::IMAGE_SIZE).unwrap();
    let kernel = std::fs::read(root.join(xtask::fs::KERNEL)).unwrap();
    let opts = xtask::fs::bootable_layout(xtask::fs::IMAGE_SIZE, &kernel).unwrap();
    let plain = xtask::fs::build_profile(&root, &opts, xtask::fs::Profile::Server).unwrap();

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
        "the {} KiB root image: a filesystem of {} blocks, {} free; a kernel of {} blocks",
        image.len() / 1024,
        report.sb.size,
        report.free_blocks,
        loadable.len().div_ceil(BSIZE)
    );
    assert!(
        report.free_blocks * 5 >= report.sb.size && report.free_blocks > 269,
        "the programs in /bin leave {} of the root filesystem's {} blocks free, less than a fifth: \
         make the image bigger (xtask::fs::IMAGE_SIZE)",
        report.free_blocks,
        report.sb.size
    );

    // The default machine's 4 MiB of RAM, and the firmware's 2 MiB.
    assert_eq!(MachineConfig::default().ram_size, 4 << 20);
    assert_eq!(avalon_client::CAMELOT_RAM, 6 << 20);
    let (result, out, disk, m) = boot_image(
        image,
        b"echo from the disk\nls /bin | wc > /count\ncat /count\nexit\n",
    );
    assert_eq!(result, RunResult::Exited(0), "{out}");
    // Init ran the server's /etc/rc: the shell, whose exit ends the machine, and
    // shd, which can't run on this machine without a network card, says so
    // once (wherever its line lands among the shell's) and is left.
    let shd = "shd: socket: no network card\n";
    assert_eq!(out.matches(shd).count(), 1, "{out}");
    assert!(!out.contains("init: /bin/shd"), "{out}");
    let out = out.replacen(shd, "", 1);
    assert!(
        out.starts_with(&format!(
            "boot: kernel at sector 0x{:08x}\n{}Camelot v0.1.0 booting...\n",
            info.kernel_sector,
            xtask::kernel::BANNER
        )),
        "{out}"
    );
    assert!(out.contains("\nram: 6 MiB at 0x80000000\n"), "{out}");
    // Every program.
    let programs: Vec<String> = xtask::userprogs::program_names(&xtask::userprogs::user_dir(&root)).unwrap();
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
    // The bootloader ran in S-mode, the firmware's next stage, and left the
    // ROM alone (its entry doubleword is the hand-off to the firmware, where
    // the kernel's harts wait for hart_start); the kernel is linked high and
    // loaded at 0x8020_0000.
    let k = elf::Elf::parse(&kernel).unwrap();
    let entry = k.header.entry;
    let entry = k
        .segments
        .iter()
        .find(|s| s.ty == 1 && (s.vaddr..s.vaddr + s.memsz).contains(&entry))
        .map(|s| s.paddr + (entry - s.vaddr))
        .unwrap();
    let lo = u64::from(m.cpu.bus.peek(grail::boot::BOOT_ENTRY_ADDR).unwrap());
    let hi = u64::from(m.cpu.bus.peek(grail::boot::BOOT_ENTRY_ADDR + 4).unwrap());
    assert_eq!(lo | hi << 32, grail::boot::FW_HANDOFF_PC);
    assert_eq!(entry, grail::firmware::FW_JUMP_ADDR);
}

#[test]
fn the_root_image_holds_the_header_crt0_and_libc_the_programs_are_linked_from() {
    // #240: /usr/include has the header the programs and the library are
    // compiled with, and /usr/lib the start code, the C library and the
    // linker script every program of /bin is linked from: the host's
    // objects stripped of their debug information, as distributions
    // install a C library (elf::strip), and libc.a the stripped members in
    // an archive (llvm-ar's format, elf::archive), named as a C library
    // names them.
    let root = repo_root();
    let user = xtask::userprogs::user_dir(&root);
    let image = xtask::fs::build_bootable(&root, xtask::fs::Profile::Server, xtask::fs::IMAGE_SIZE).unwrap();
    let fs = Fs::open(&image).unwrap();
    let read = |path: &str| fs.read_file(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert_eq!(
        read("/usr/include/user.h"),
        std::fs::read(user.join("user.h")).unwrap()
    );
    assert_eq!(
        read("/usr/lib/user.ld"),
        std::fs::read(user.join("user.ld")).unwrap()
    );
    let cc = xtask::userprogs::CCompiler::Merlin;
    let rt = xtask::userprogs::build_runtime(&user, cc).unwrap();
    assert_eq!(rt.objects.len(), 1);
    let has_debug = |obj: &[u8]| {
        let e = elf::Elf::parse(obj).unwrap();
        e.sections.iter().any(|s| elf::strip::is_debug_section(&s.name))
    };
    let crt0 = read("/usr/lib/crt0.o");
    assert_eq!(crt0, elf::strip::strip_debug(&rt.objects[0].data).unwrap());
    let libc = read("/usr/lib/libc.a");
    let archive = elf::archive::parse(&libc).unwrap();
    let names: Vec<&str> = archive.members.iter().map(|m| m.name.as_str()).collect();
    let sources: Vec<String> = xtask::userprogs::library(&user)
        .unwrap()
        .iter()
        .map(xtask::userprogs::member_name)
        .collect();
    assert_eq!(names, sources);
    for want in ["printf.o", "malloc.o", "usys.o", "environ.o", "memcpy.o"] {
        assert!(names.contains(&want), "{want} not in libc.a: {names:?}");
    }
    for (m, o) in archive.members.iter().zip(&rt.library) {
        assert!(!has_debug(m.data), "{} keeps debug information", m.name);
        assert_eq!(
            m.data,
            &elf::strip::strip_debug(&o.data).unwrap()[..],
            "{}",
            m.name
        );
    }
    assert!(has_debug(
        &rt.library.iter().find(|m| m.name == "printf.o").unwrap().data
    ));
    assert_eq!(libc, rt.installed().archive);
    // The symbol table says where printf and the system calls are.
    let defines = |sym: &str| {
        archive
            .symbols
            .iter()
            .find(|(s, _)| s == sym)
            .map(|&(_, m)| archive.members[m].name.as_str())
    };
    assert_eq!(defines("printf"), Some("printf.o"));
    assert_eq!(defines("write"), Some("usys.o"));
    assert_eq!(defines("main"), None);
    // Every program in /bin is the link of the image's crt0.o, its objects
    // and the image's libc.a, without its line tables.
    let script = String::from_utf8(read("/usr/lib/user.ld")).unwrap();
    let opts = merlin_ld::Options {
        script: Some(merlin_ld::ScriptSource {
            name: "user.ld".into(),
            text: script,
        }),
        entry: None,
    };
    let progs = xtask::userprogs::programs(&user).unwrap();
    assert!(progs.len() > 30, "{}", progs.len());
    for prog in &progs {
        let mut inputs = vec![merlin_ld::Input {
            name: "/usr/lib/crt0.o".into(),
            data: crt0.clone(),
        }];
        inputs.extend(xtask::userprogs::program_objects(&user, prog, cc).unwrap());
        inputs.push(merlin_ld::Input {
            name: "/usr/lib/libc.a".into(),
            data: libc.clone(),
        });
        let files: Vec<(&merlin_ld::Input, bool)> = inputs.iter().map(|i| (i, false)).collect();
        let linked = merlin_ld::link_files(&files, &opts).unwrap();
        let stripped = merlin_ld::strip_debug(&linked.elf).unwrap();
        assert!(
            stripped == read(&format!("/bin/{}", prog.stem)),
            "/bin/{} is not the link of /usr/lib's files",
            prog.stem
        );
    }
}

/// The objects of /usr/lib are what `llvm-strip -g` makes of the host's,
/// every one of them, byte for byte (`elf::strip`; its fixtures hold four,
/// crates/elf/tests/strip). Needs llvm-strip, found next to clang or on
/// PATH.
#[test]
#[ignore]
fn the_installed_objects_are_what_llvm_strip_makes() {
    let strip = xtask::tools::find("llvm-strip").expect("llvm-strip not found");
    let user = xtask::userprogs::user_dir(&repo_root());
    let rt = xtask::userprogs::build_runtime(&user, xtask::userprogs::CCompiler::Merlin).unwrap();
    let dir = std::env::temp_dir().join(format!("xtask-strip-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for o in rt.objects.iter().chain(&rt.library) {
        let (input, output) = (dir.join(&o.name), dir.join(format!("{}.stripped", o.name)));
        std::fs::write(&input, &o.data).unwrap();
        let status = std::process::Command::new(&strip)
            .arg("-g")
            .arg(&input)
            .arg("-o")
            .arg(&output)
            .status()
            .unwrap();
        assert!(status.success(), "llvm-strip -g {}", o.name);
        let want = std::fs::read(&output).unwrap();
        assert!(
            elf::strip::strip_debug(&o.data).unwrap() == want,
            "{} is not llvm-strip's",
            o.name
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_image_size_is_the_whole_image_and_too_small_is_refused() {
    let root = repo_root();
    let kernel = std::fs::read(root.join(xtask::fs::KERNEL)).unwrap();
    let kernel_blocks = boot::loadable(&kernel).unwrap().len().div_ceil(BSIZE) as u32;
    // A bigger --size: a bigger filesystem, the same kernel.
    let bigger = xtask::fs::IMAGE_SIZE + 1024;
    let image = xtask::fs::build_bootable(&root, xtask::fs::Profile::Server, bigger).unwrap();
    assert_eq!(image.len(), bigger as usize * BSIZE);
    assert_eq!(fsck::check(&image).unwrap().sb.size, bigger - kernel_blocks);
    // The programs do not fit in 1 MiB, nor anything in the kernel's own size.
    let err = xtask::fs::build_bootable(&root, xtask::fs::Profile::Server, 1024).unwrap_err();
    assert!(
        err.starts_with(&format!(
            "the server root filesystem does not fit in a 1024-block image, where the kernel leaves it {} \
             blocks",
            1024 - kernel_blocks
        )) && err.ends_with("give a bigger --size"),
        "{err}"
    );
    let err = xtask::fs::build_bootable(&root, xtask::fs::Profile::Server, kernel_blocks).unwrap_err();
    assert_eq!(
        err,
        format!(
            "a {kernel_blocks}-block image has no room for a filesystem: the kernel alone takes \
             {kernel_blocks} blocks"
        )
    );
    // Too big is not cured by a bigger --size: the error says what is wrong
    // and does not suggest one.
    let err = xtask::fs::build_bootable(&root, xtask::fs::Profile::Server, u32::MAX).unwrap_err();
    assert!(
        err.starts_with(&format!("cannot build a {}-block server root image: ", u32::MAX))
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
    let mut m = Machine::new(camelot_vm(vec![DiskConfig::memory(vec![0; 64 * 512])])).unwrap();
    m.cpu.bus.uart.capture = true;
    m.load_elf(&kernel).unwrap();
    m.set_uart_input(xtask::kernel::typed(b"exit\n"));
    let result = m.run(Some(BOOT_MAX_STEPS), |_| {});
    let out = String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
    assert_eq!(result, RunResult::Exited(0xfe), "{out}");
    // The kernel stacks' self-test and their deepest use come between.
    assert!(
        out.contains("fs: no filesystem on disk 0\nkstack self-test: 3/3 passed\nkstack: "),
        "{out}"
    );
    assert!(
        out.ends_with(
            " bytes used at most\ncannot start /bin/sh: error 2\npanic: no root filesystem, and no programs \
             built into the kernel\n"
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

/// The names the assembly sources give the device registers and their values,
/// and Grail's value of each: the sysinfo block's (spec section 9.5), the
/// disk's (9.6) and the network card's (9.8), with the bits and codes read
/// from them. Every name is defined by at least one source.
const REGISTERS: &[(&str, u32)] = {
    use grail::sysinfo::{self, DeviceType};
    use grail::{block, net};
    &[
        ("SYSINFO_MAGIC", sysinfo::SYSINFO_MAGIC),
        ("SYS_MAGIC", sysinfo::REG_MAGIC),
        ("SYS_VERSION", sysinfo::REG_VERSION),
        ("SYS_RAM_BASE", sysinfo::REG_RAM_BASE),
        ("SYS_RAM_SIZE", sysinfo::REG_RAM_SIZE),
        ("SYS_TIMEBASE_HZ", sysinfo::REG_TIMEBASE_HZ),
        ("SYS_CPU_HZ", sysinfo::REG_CPU_HZ),
        ("SYS_FLAGS", sysinfo::REG_FLAGS),
        ("SYS_DEV_COUNT", sysinfo::REG_DEV_COUNT),
        ("SYS_HARTS", sysinfo::REG_HARTS),
        ("SYS_DEV_TABLE", sysinfo::DEV_TABLE),
        ("DEV_ENTRY_SIZE", sysinfo::DEV_ENTRY_SIZE),
        ("DEV_TYPE", sysinfo::DEV_TYPE),
        ("DEV_IRQ", sysinfo::DEV_IRQ),
        ("DEV_BASE", sysinfo::DEV_BASE),
        ("DEV_SIZE", sysinfo::DEV_SIZE),
        ("FLAG_REAL_TIME", sysinfo::FLAG_REAL_TIME),
        ("DEV_BLOCK", DeviceType::Block as u32),
        ("DEV_NET", DeviceType::Net as u32),
        ("BLK_CAPACITY", block::REG_CAPACITY),
        ("BLK_SECTOR", block::REG_SECTOR),
        ("BLK_DMA", block::REG_DMA),
        ("BLK_COUNT", block::REG_COUNT),
        ("BLK_FLAGS", block::REG_FLAGS),
        ("BLK_CMD", block::REG_CMD),
        ("BLK_STATUS", block::REG_STATUS),
        ("BLK_ERROR", block::REG_ERROR),
        ("BLK_CTRL", block::REG_CTRL),
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
        ("NET_TX_BASE", net::REG_TX_BASE),
        ("NET_TX_SIZE", net::REG_TX_SIZE),
        ("NET_TX_HEAD", net::REG_TX_HEAD),
        ("NET_TX_TAIL", net::REG_TX_TAIL),
        ("NET_RX_BASE", net::REG_RX_BASE),
        ("NET_RX_SIZE", net::REG_RX_SIZE),
        ("NET_RX_HEAD", net::REG_RX_HEAD),
        ("NET_RX_TAIL", net::REG_RX_TAIL),
        ("DESC_SIZE", net::DESC_SIZE),
        ("IRQ_RX", net::IRQ_RX),
    ]
};

/// A `#define NAME VALUE` of a C header, the value a number.
fn define(header: &str, name: &str) -> u64 {
    let line = header
        .lines()
        .find(|l| l.split_whitespace().take(2).eq(["#define", name]))
        .unwrap_or_else(|| panic!("no #define {name}"));
    let value = line
        .split_whitespace()
        .nth(2)
        .unwrap()
        .trim_matches(|c| c == '(' || c == ')');
    match value.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).unwrap(),
        None => value.parse().unwrap(),
    }
}

#[test]
fn the_firmware_grail_the_kernel_and_the_documents_agree() {
    // The firmware's layout and hand-off (spec section 7, "Firmware"), as
    // Grail lays the machine out, the firmware's sources and the bootloader
    // and kernel assume it.
    let root = repo_root();
    let read = |path: &str| std::fs::read_to_string(root.join(path)).unwrap();
    let fw = read("firmware/fw.h");
    use grail::firmware as gf;
    assert_eq!(define(&fw, "FW_BASE"), gf::FW_BASE);
    assert_eq!(define(&fw, "FW_SIZE"), gf::FW_SIZE);
    assert_eq!(define(&fw, "FW_JUMP_ADDR"), gf::FW_JUMP_ADDR);
    assert_eq!(define(&fw, "FW_DYNAMIC_INFO_MAGIC"), gf::FW_DYNAMIC_INFO_MAGIC);
    assert_eq!(
        define(&fw, "FW_DYNAMIC_INFO_NEXT_MODE_S"),
        gf::FW_DYNAMIC_INFO_NEXT_MODE_S
    );
    assert!(define(&fw, "FW_DYNAMIC_INFO_VERSION_MIN") <= gf::FW_DYNAMIC_INFO_VERSION);
    assert_eq!(define(&fw, "FW_MAX_HARTS") as usize, grail::MAX_HARTS);
    assert_eq!(
        define(&fw, "EXIT_BOOT_FAILED"),
        u64::from(grail::boot::EXIT_BOOT_FAILED)
    );
    assert_eq!(define(&fw, "CLINT_BASE"), grail::bus::CLINT_BASE);
    assert_eq!(define(&fw, "UART_BASE"), grail::bus::UART_BASE);
    assert_eq!(define(&fw, "SYSINFO_BASE"), grail::bus::SYSINFO_BASE);
    assert_eq!(define(&fw, "DISK0"), grail::bus::BLK_BASE);
    use grail::sysinfo;
    assert_eq!(
        define(&fw, "SYS_RESERVED_BASE"),
        u64::from(sysinfo::REG_RESERVED_BASE)
    );
    assert_eq!(
        define(&fw, "SYS_RESERVED_SIZE"),
        u64::from(sysinfo::REG_RESERVED_SIZE)
    );
    assert_eq!(define(&fw, "SYS_HARTS"), u64::from(sysinfo::REG_HARTS));
    for (name, reg) in [
        ("BLK_CAPACITY", grail::block::REG_CAPACITY),
        ("BLK_SECTOR", grail::block::REG_SECTOR),
        ("BLK_DMA", grail::block::REG_DMA),
        ("BLK_COUNT", grail::block::REG_COUNT),
        ("BLK_CMD", grail::block::REG_CMD),
        ("BLK_STATUS", grail::block::REG_STATUS),
        ("BLK_ERROR", grail::block::REG_ERROR),
    ] {
        assert_eq!(define(&fw, name), u64::from(reg), "{name}");
    }
    // The kernel reads the reserved range where sysinfo has it.
    let sysinfo_h = read("kernel/include/sysinfo.h");
    assert_eq!(
        define(&sysinfo_h, "SYS_RESERVED_BASE"),
        u64::from(sysinfo::REG_RESERVED_BASE)
    );
    assert_eq!(
        define(&sysinfo_h, "SYS_RESERVED_SIZE"),
        u64::from(sysinfo::REG_RESERVED_SIZE)
    );
    // The firmware checks a boot block as the ROM does.
    let assembly =
        |path: &str| xtask::examples::assembly_of(&root, path, &[xtask::examples::INCLUDE_DIR]).unwrap();
    let disk = assembly("firmware/disk.S");
    assert_eq!(equ(&disk, "BOOT_MAGIC"), grail::boot::BOOT_MAGIC);
    assert_eq!(equ(&disk, "BOOT_SIZE") as usize, grail::boot::BOOT_BLOCK_SIZE);
    assert_eq!(equ(&disk, "BOOT_SIG") as usize, boot::MAGIC_AT);
    // The bootloader runs where the firmware starts its next stage, and the
    // kernel loads there.
    let loader = assembly("kernel/boot/boot.S");
    assert_eq!(u64::from(equ(&loader, "FW_JUMP_ADDR")), gf::FW_JUMP_ADDR);
    assert_eq!(equ(&loader, "BOOT_AREA"), grail::boot::BOOT_AREA_SIZE);
    assert_eq!(equ(&loader, "BOOT_SIZE") as usize, grail::boot::BOOT_BLOCK_SIZE);
    assert!(read("kernel/kernel.lds.S").contains("AT(0x80200000)"));
    // And the spec says so.
    let spec = read("docs/excalibur-spec.md");
    for s in [
        "0x8000_0000 to 0x801F_FFFF",
        "starts at 0x8020_0000 (OpenSBI's `FW_JUMP_ADDR`)",
        "`SYS_RESERVED_BASE`",
        "the magic 0x4942_534F (\"OSBI\"), version 2",
        "`csrr a0, mhartid; lui t0, 1; ld a1, 0x18(t0); addi a2, t0, 0x128; ld t0, 0x120(t0); jr t0`",
    ] {
        assert!(spec.contains(s), "the spec does not say {s}");
    }
    assert_eq!(grail::boot::FDT_ADDR_ADDR, 0x1018);
    assert_eq!(grail::boot::FW_ENTRY_ADDR, 0x1120);
    assert_eq!(grail::boot::FW_DYNAMIC_INFO_ADDR, 0x1128);
    assert_eq!(grail::boot::FW_HANDOFF_PC, 0x1100);
}

#[test]
fn the_rom_the_bootloader_mkfs_and_the_documents_agree() {
    let root = repo_root();
    let read = |path: &str| std::fs::read_to_string(root.join(path)).unwrap();
    // A `.S` source as merlin-as gets it: preprocessed.
    let assembly =
        |path: &str| xtask::examples::assembly_of(&root, path, &[xtask::examples::INCLUDE_DIR]).unwrap();
    let rom = assembly("crates/grail/rom/diskboot.S");
    let loader = assembly("kernel/boot/boot.S");
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
    // The machine's layout, as Grail has it.
    assert_eq!(u64::from(equ(&rom, "RAM_BASE")), grail::bus::RAM_BASE);
    assert_eq!(
        equ(&rom, "BOOT_SECTORS") as usize,
        boot::BOOT_BLOCK_SIZE / grail::block::SECTOR_SIZE
    );
    assert_eq!(u64::from(equ(&rom, "UART_BASE")), grail::bus::UART_BASE);
    assert_eq!(u64::from(equ(&rom, "SYSINFO_BASE")), grail::bus::SYSINFO_BASE);
    assert_eq!(u64::from(equ(&rom, "DISK0")), grail::bus::BLK_BASE);
    // The loader's and the examples' device registers are where Grail has
    // them (spec sections 9.5, 9.6, 9.8).
    let mut sources = vec![("crates/grail/rom/diskboot.S".to_string(), rom.clone())];
    for ex in xtask::examples::EXAMPLES {
        for s in ex.sources {
            let path = format!("{}/{s}", ex.dir_name());
            let text = assembly(&path);
            sources.push((path, text));
        }
    }
    let mut unchecked: BTreeSet<&str> = REGISTERS.iter().map(|&(name, _)| name).collect();
    for (path, source) in &sources {
        for &(name, value) in REGISTERS {
            if has_equ(source, name) {
                assert_eq!(equ(source, name), value, "{path}: {name}");
                unchecked.remove(name);
            }
        }
    }
    assert!(unchecked.is_empty(), "no source defines {unchecked:?}");
    assert!(spec.contains("exit code 0xFD") && spec.contains("0x544F_4F42"));
    // The fields the bootloader reads are where mkfs writes them, and the
    // ELF64 layout it reads is the elf crate's.
    assert_eq!(equ(&loader, "KERNEL_SECTOR") as usize, boot::KERNEL_SECTOR_AT);
    assert_eq!(equ(&loader, "KERNEL_SECTORS") as usize, boot::KERNEL_SECTORS_AT);
    assert_eq!(equ(&loader, "HEADERS_SIZE") as usize, boot::KERNEL_HEADERS);
    assert_eq!(equ(&loader, "PHDR_SIZE") as usize, elf::PHDR64_SIZE);
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
    // And the desktop's, once too: The desktop's is **N MiB in all**.
    let mib = spec
        .split("The desktop's is **")
        .nth(1)
        .and_then(|rest| rest.split(" MiB in all**").next())
        .expect("the spec's \"Root image\" says: The desktop's is **N MiB in all**");
    let blocks = xtask::fs::DESKTOP_IMAGE_SIZE;
    assert_eq!(
        mib,
        format!("{}", blocks / 1024),
        "the spec says {mib} MiB, xtask::fs::DESKTOP_IMAGE_SIZE is {blocks} blocks"
    );
    assert_eq!(blocks % 1024, 0, "a whole number of MiB");
    for profile in xtask::fs::Profile::ALL {
        assert_eq!(spec.matches(" MiB in all**").count(), 2, "one size a profile");
        assert!(
            spec.contains(&format!("**{}**", profile.name())),
            "{}",
            profile.name()
        );
    }
}

#[test]
fn the_profiles_images_differ_in_their_etc_rc_and_fonts() {
    // Issue #182: each profile's directory of kernel/profiles goes over
    // kernel/fs byte for byte (its /etc/rc); the desktop's has the console's
    // font and its licence in /usr/share/fonts; both have every program.
    let root = repo_root();
    let programs = xtask::userprogs::program_names(&xtask::userprogs::user_dir(&root)).unwrap();
    for profile in xtask::fs::Profile::ALL {
        let opts = Options {
            size: profile.image_size(),
            ..Options::default()
        };
        let image = xtask::fs::build_profile(&root, &opts, profile).unwrap();
        let fs = Fs::open(&image).unwrap();
        let rc = root
            .join(xtask::fs::PROFILES_DIR)
            .join(profile.name())
            .join("etc/rc");
        assert_eq!(
            fs.read_file("/etc/rc").unwrap(),
            std::fs::read(rc).unwrap(),
            "{}",
            profile.name()
        );
        let bin: BTreeSet<String> = fs
            .list("/bin")
            .unwrap()
            .into_iter()
            .map(|e| String::from_utf8_lossy(&e.name).into_owned())
            .collect();
        for p in &programs {
            assert!(bin.contains(p), "{}: no /bin/{p}", profile.name());
        }
        let font = fs.read_file("/usr/share/fonts/spleen/8x16.psfu");
        match profile {
            xtask::fs::Profile::Server => assert!(font.is_err(), "the server has no fonts"),
            xtask::fs::Profile::Desktop => {
                let fonts = root.join("kernel/fonts");
                assert_eq!(
                    font.unwrap(),
                    std::fs::read(fonts.join("spleen-8x16.psfu")).unwrap()
                );
                assert_eq!(
                    fs.read_file("/usr/share/fonts/spleen/LICENSE").unwrap(),
                    std::fs::read(fonts.join("LICENSE.spleen")).unwrap()
                );
            }
        }
    }
    // The tests' image has no profile: no /etc/rc, so the shell is pid 1.
    let opts = Options {
        size: xtask::fs::IMAGE_SIZE,
        ..Options::default()
    };
    let plain = Fs::open(&xtask::fs::build(&root, &opts).unwrap()).unwrap();
    assert!(plain.read_file("/etc/rc").is_err());
}

#[test]
fn the_desktop_image_boots_and_init_starts_its_shell_again() {
    // The desktop's /etc/rc keeps the console's shell running: one that
    // ends is started again, even at once (but not five times in a row), so
    // two Ctrl-Ds or `exit`s don't lose it. shd can't run on a machine with
    // no network card: it says so, and init leaves it.
    let root = repo_root();
    let image =
        xtask::fs::build_bootable(&root, xtask::fs::Profile::Desktop, xtask::fs::DESKTOP_IMAGE_SIZE).unwrap();
    let mut m = Machine::new(MachineConfig {
        ram_size: avalon_client::DESKTOP_RAM as usize,
        ..camelot_vm(vec![DiskConfig::memory(image)])
    })
    .unwrap();
    m.boot_from_disk().unwrap();
    m.cpu.bus.uart.capture = true;
    m.set_uart_input(xtask::kernel::typed(b"exit\nexit\nps\n"));
    // The machine runs on: run it until the third shell's ps is done.
    let printed = |m: &Machine| String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
    let done = |out: &str| out.find("$ ps\n").is_some_and(|at| out[at + 5..].contains("$ "));
    let mut steps = 0;
    while !done(&printed(&m)) {
        assert!(steps < 2 * BOOT_MAX_STEPS, "{}", printed(&m));
        let result = m.run(Some(1_000_000), |_| {});
        assert!(
            !matches!(result, RunResult::Exited(_)),
            "{result:?}: {}",
            printed(&m)
        );
        steps += 1_000_000;
    }
    let out = printed(&m);
    assert!(out.contains("\nram: 64 MiB at 0x80000000\n"), "{out}");
    assert_eq!(
        out.matches("Type help for the list of commands.\n").count(),
        3,
        "{out}"
    );
    assert_eq!(out.matches("shd: socket: no network card\n").count(), 1, "{out}");
    assert!(!out.contains("init: "), "{out}");
    // The third shell is init's child too, pid 1's.
    let ps = &out[out.find("$ ps\n").unwrap_or_else(|| panic!("{out}"))..];
    let processes: Vec<Vec<&str>> = ps
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>())
        .filter(|w| w.len() == 5)
        .collect();
    assert!(
        processes.iter().any(|w| w[..2] == ["1", "0"] && w[4] == "init"),
        "{out}"
    );
    assert!(processes.iter().any(|w| w[1] == "1" && w[4] == "sh"), "{out}");
}
