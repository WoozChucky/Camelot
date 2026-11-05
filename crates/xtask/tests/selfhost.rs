//! Self-hosting, step 1 (#135): the assembler that runs on Camelot
//! (`kernel/user/as`, `/bin/as`) writes, for every file of a corpus, the same
//! object the host's merlin-as writes, byte for byte. The test boots the disk
//! kernel on an image with `/bin/sh`, `/bin/as` and the corpus in `/src`, has
//! the shell run `as` on each file into `/out`, then reads the objects back
//! on the host and compares them. The corpus: merlin-as's golden encodings,
//! every hand-written `.s` of the repository, the line table corpus
//! (`crates/merlin-as/tests/debug_line`, whose objects are clang's), the
//! mapping symbol corpus (`crates/merlin-as/tests/mapping`, clang's too), and
//! merlin's assembly for a C file, with `-g` (`.file`, `.loc` and a DWARF
//! compile unit) and without, data of every size (`DATA`) and sections of
//! every kind (`SECTIONS`). An ignored test adds merlin's `-g` assembly for
//! two big C files, the shell and the kernel's vm.c.
//!
//! The machines boot the disk kernel, `kernel/kernel-disk.elf`, on
//! Camelot's firmware; `/bin/as` writes ELF64 objects, as merlin-as does,
//! and every object `/bin/ld` links is one.

use camelot_fs::mkfs::{Mkfs, Options};
use camelot_fs::read::Fs;
use grail::block::MemDisk;
use grail::{Machine, MachineConfig, RunResult};
use std::path::PathBuf;
use xtask::{kernel, repo_root, toolchain, userprogs};

const PROMPT: &str = "$ ";

/// The corpus: (name on the image, text); `big` adds merlin's assembly for
/// the shell and the kernel's vm.c. A `.S` source (the examples, the boot
/// ROM) goes in as merlin's preprocessor makes it, as `cargo xtask
/// examples` gives it to merlin-as (Camelot has no preprocessor), and is
/// named `.s` on the image. Every value of merlin-as's `li` golden
/// (`tests/golden/li.golden`) is one more `li a0, VALUE` line, in files of
/// [`LI_LINES`]: the values merlin-as's encode_golden.rs holds RISCVMatInt
/// to, so that /bin/as's port of it is held to them too.
fn corpus(big: bool) -> Vec<(String, String)> {
    let root = repo_root();
    // Assembly sources, not the preprocessed linker script (kernel.lds.S).
    let assembly = |p: &std::path::Path| {
        p.extension().is_some_and(|x| x == "s" || x == "S")
            && !p
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(".lds.S"))
    };
    let mut files: Vec<PathBuf> = vec![root.join("crates/merlin-as/tests/golden/encode.s")];
    let dirs = [
        "kernel",
        "kernel/user",
        "kernel/boot",
        "crates/grail/rom",
        "crates/merlin-as/tests/debug_line",
        "crates/merlin-as/tests/mapping",
    ];
    for dir in dirs {
        let mut more: Vec<PathBuf> = std::fs::read_dir(root.join(dir))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| assembly(p))
            .collect();
        more.sort();
        files.extend(more);
    }
    let mut examples: Vec<PathBuf> = Vec::new();
    for e in std::fs::read_dir(root.join("examples")).unwrap() {
        let dir = e.unwrap().path();
        if dir.is_dir() {
            for f in std::fs::read_dir(&dir).unwrap() {
                let p = f.unwrap().path();
                if assembly(&p) {
                    examples.push(p);
                }
            }
        }
    }
    examples.sort();
    files.extend(examples);
    let mut out: Vec<(String, String)> = files
        .iter()
        .map(|p| {
            let rel = p
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            // The kernel's own sources also include its headers
            // (kernel/include), as `cargo xtask kernel` builds them.
            let kernel_dirs = [xtask::examples::INCLUDE_DIR, "kernel/include"];
            let dirs: &[&str] = if rel.starts_with("kernel/") {
                &kernel_dirs
            } else {
                &[xtask::examples::INCLUDE_DIR]
            };
            let text = xtask::examples::assembly_of(&root, &rel, dirs).unwrap();
            let name = rel.strip_suffix(".S").map_or(rel.clone(), |s| format!("{s}.s"));
            (name.replace('/', "_"), text)
        })
        .collect();
    // merlin's assembly, as the kernel and the programs are built (-g), and
    // without debug information.
    let user = root.join("kernel/user");
    let plain = toolchain::without_debug_info(|| toolchain::compile_c(&user, "hello.c", &[], "kernel/user"));
    out.push(("kernel_user_hello.c.s".into(), plain.unwrap()));
    let mut c_files = vec![("kernel/user", "hello.c", &[][..])];
    if big {
        c_files.extend([
            ("kernel/user", "sh.c", &[][..]),
            ("kernel", "vm.c", &["include"][..]),
        ]);
    }
    for (dir, file, includes) in c_files {
        let text = toolchain::compile_c(&root.join(dir), file, includes, dir).unwrap();
        assert!(text.contains(".loc "), "{file} has no line information");
        out.push((format!("{}_{file}.g.s", dir.replace('/', "_")), text));
    }
    out.push(("data.s".into(), DATA.into()));
    out.push(("sections.s".into(), SECTIONS.into()));
    out.extend(li_corpus());
    out
}

/// How many `li` lines a file of the li golden's values has.
const LI_LINES: usize = 250;

/// The values of merlin-as's `li` golden (each line `VALUE | WORDS`, `#` a
/// comment), as `li a0, VALUE` lines in files of [`LI_LINES`].
fn li_corpus() -> Vec<(String, String)> {
    let golden = "crates/merlin-as/tests/golden/li.golden";
    let text = std::fs::read_to_string(repo_root().join(golden)).unwrap();
    let lines: Vec<String> = text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| format!("    li a0, {}\n", l.split('|').next().unwrap().trim()))
        .collect();
    assert_eq!(lines.len(), 2246, "{golden}");
    let name = golden.replace('/', "_");
    lines
        .chunks(LI_LINES)
        .enumerate()
        .map(|(k, chunk)| (format!("{name}.{k}.s"), chunk.concat()))
        .collect()
}

/// `.section` with flags, every way of writing the type, mergeable sections
/// and their entry sizes, names with no flags of their own, LLVM's own
/// sections, code sections with and without `.text`'s alignment, and going
/// back to a section (merlin-as's tests/sections.rs has clang's headers).
const SECTIONS: &str = "
        .text
        nop
        .section .text.start
        nop
        .section .myx, \"ax\", @progbits
        nop
        .p2align 3
        nop
        .section .rodata.str1.1, \"aMS\", @progbits, 1
        .asciz \"hi\"
        .section .rodata.cst8x, \"aM\", %progbits, 8
        .dword 1
        .section .mystuff
        .word 1
        .section .sbss
        .word 0
        .section .mybss, \"aw\", @nobits
        .space 12
        .section .init_array, \"aw\"
        .word 0
        .section .note.camelot, \"a\", \"note\"
        .word 4
        .section .flags, \"0x3\"
        .section .num, \"a\", %8
        .section .proc, \"a\", @0x70000001
        .section .debug_str
        .asciz \"s\"
        .section .tdata, \"awT\", @progbits
        .word 7
        .section .text.start
        nop
        .section .rodata.str1.1, \"aMS\", @progbits, 1
        .asciz \"x\"
";

/// Data of every size at the edges of its range, `>>` (logical) and the
/// relocations of `.word` and `.dword`, in a section and in debug
/// information, and every escape a string may hold: what no file of the
/// corpus has all of.
const DATA: &str = "
        .text
f:      nop
.Lend:  nop
        .data
        .byte 255, -128, -16 >> 62
        .half -32768, 0xffff
        .word 0xffffffff, -0x80000000, 0x80000000 >> 31
        .equ BIG, 0x7fffffffffffffff
        .dword -16 >> 2, 1 << 63 >> 63, BIG + 1
        .quad -1
x:      .8byte x + 8, ., f + BIG + 1
        .word x, ., 1 + (BIG + f)
        .space 3, 0x1ff
        .ascii \"\\b\\f\\n\\r\\t\\\"\\\\\\0\\12\\377\\x41\"
        .size x, 0x100000003
        .size f, -4
        .section .debug_info,\"\",@progbits
        .quad .Lend
        .dword f + 2, .Lend - f
";

/// The name a file of the corpus has on the image: short enough for a
/// directory entry (DIRSIZ, kernel/include/fs.h).
fn short(i: usize) -> String {
    format!("f{i}")
}

/// An image with the shell and the assembler in /bin, `inputs` in /src
/// (named by `short`), and an empty /out.
fn image(inputs: &[(String, String)]) -> Vec<u8> {
    let files: Vec<(String, Vec<u8>)> = inputs
        .iter()
        .enumerate()
        .map(|(i, (_, text))| (format!("/src/{}.s", short(i)), text.as_bytes().to_vec()))
        .collect();
    image_of(&["as"], &["/src"], &files)
}

/// An image with the shell and `bins` in /bin (the user programs of those
/// names), the directories `dirs`, `files` (path, contents) and an empty
/// /out.
fn image_of(bins: &[&str], dirs: &[&str], files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let bytes: usize = files.iter().map(|(_, t)| t.len()).sum();
    let blocks = 2048 + 3 * bytes / camelot_fs::BSIZE;
    let mut fs = Mkfs::new(&Options {
        size: blocks as u32,
        ..Options::default()
    })
    .unwrap();
    fs.mkdir("/dev").unwrap();
    fs.add_device("/dev/console", xtask::fs::CONSOLE_MAJOR, 0)
        .unwrap();
    fs.mkdir("/bin").unwrap();
    let user = userprogs::user_dir(&repo_root());
    for name in std::iter::once(&"sh").chain(bins) {
        let prog = userprogs::build_named(&user, name).unwrap();
        fs.add_file(&format!("/bin/{name}"), &prog.elf).unwrap();
    }
    for d in dirs.iter().chain(&["/out"]) {
        fs.mkdir(d).unwrap();
    }
    for (path, data) in files {
        fs.add_file(path, data).unwrap();
    }
    fs.finish()
}

/// The machine: Camelot's firmware, the kernel's SBI, with the firmware's
/// 2 MiB on top of the default 4 MiB of RAM.
fn machine_config() -> MachineConfig {
    MachineConfig {
        bios: grail::Bios::Default,
        ram_size: grail::DEFAULT_RAM_SIZE + grail::firmware::FW_SIZE as usize,
        ..MachineConfig::default()
    }
}

/// Boots the disk kernel on `image`, types `input` (ending with `exit`).
/// Returns what it printed and the disk afterwards. The kernel has 4 MiB of
/// RAM besides the firmware's, which the assembler is written to fit in.
fn boot(image: Vec<u8>, input: &str, max_steps: u64) -> (String, Vec<u8>) {
    let config = machine_config();
    let disk = MemDisk::from_image(image).unwrap();
    let mut m = Machine::with_backends(config, vec![Box::new(disk)]).unwrap();
    m.cpu.bus.uart.capture = true;
    let elf = std::fs::read(repo_root().join("kernel").join(kernel::DISK_KERNEL)).unwrap();
    m.load_elf(&elf).unwrap();
    m.set_uart_input(kernel::typed(input.as_bytes()));
    let r = m.run(Some(max_steps), |_| {});
    let out = String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
    assert_eq!(r, RunResult::Exited(0), "{out}");
    let d = &mut m.cpu.bus.disks[0];
    let mut data = vec![0; d.capacity() as usize * camelot_fs::SECTOR_SIZE];
    d.backend().read(0, &mut data).unwrap();
    (out, data)
}

/// What the shell printed after `command`, up to its next prompt.
fn shell_output<'a>(out: &'a str, command: &str) -> &'a str {
    let typed = format!("{PROMPT}{command}\n");
    let start = out
        .find(&typed)
        .unwrap_or_else(|| panic!("no `{command}` in:\n{out}"))
        + typed.len();
    let end = out[start..].find(PROMPT).map_or(out.len(), |e| start + e);
    &out[start..end]
}

#[test]
fn the_assembler_on_camelot_writes_what_merlin_as_writes() {
    assemble_corpus(false);
}

/// The same with merlin's assembly for the shell and vm.c too: a few
/// minutes.
#[test]
#[ignore]
fn the_assembler_on_camelot_assembles_big_c_files_as_merlin_as_does() {
    assemble_corpus(true);
}

/// Machines the corpus is split over, run side by side.
const MACHINES: usize = 3;

fn assemble_corpus(big: bool) {
    let inputs = corpus(big);
    assert!(inputs.len() > 30, "{}", inputs.len());
    // Each machine gets every MACHINES-th file, so the big ones spread out.
    let shares: Vec<Vec<(String, String)>> = (0..MACHINES)
        .map(|k| inputs.iter().skip(k).step_by(MACHINES).cloned().collect())
        .collect();
    let wrong: Vec<String> = std::thread::scope(|scope| {
        let runs: Vec<_> = shares
            .iter()
            .map(|share| scope.spawn(move || assemble_on_camelot(share)))
            .collect();
        runs.into_iter().flat_map(|r| r.join().unwrap()).collect()
    });
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Boots a machine that assembles `inputs` with /bin/as. Returns what came
/// out different from merlin-as's objects.
fn assemble_on_camelot(inputs: &[(String, String)]) -> Vec<String> {
    let commands: Vec<String> = (0..inputs.len())
        .map(|i| format!("as /src/{0}.s -o /out/{0}.o", short(i)))
        .collect();
    let input = format!("{}\nexit 0\n", commands.join("\n"));
    let (out, disk) = boot(image(inputs), &input, 20_000_000_000);
    let fs = Fs::open(&disk).unwrap();
    let mut wrong = Vec::new();
    for (i, (name, text)) in inputs.iter().enumerate() {
        let said = shell_output(&out, &commands[i]);
        let want = merlin_as::assemble_to_elf(text).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        match fs.read_file(&format!("/out/{}.o", short(i))) {
            Ok(got) if got == want && said.is_empty() => {}
            Ok(got) => wrong.push(format!(
                "{name}: {} bytes, merlin-as's {} (first difference at {:?}){}",
                got.len(),
                want.len(),
                got.iter().zip(&want).position(|(a, b)| a != b),
                if said.is_empty() {
                    String::new()
                } else {
                    format!("; it said: {said}")
                }
            )),
            Err(e) => wrong.push(format!("{name}: no object ({e}); it said: {said}")),
        }
    }
    wrong
}

/// Inputs merlin-as refuses for RV64's sake: what only RV32 has (the high
/// halves of the counters and of the 64-bit CSRs), and RV64's own
/// instructions given wrong operands.
const RV64_ERRORS: &[&str] = &[
    "    rdcycleh a0\n    rdtimeh\n    rdinstreth a0, a1\n    csrr a0, cycleh\n    csrw mstatush, a0\n\
     csrr a0, pmpcfg1\n    csrr a0, 0xc80\n",
    "    slli a0, a1, 64\n    slliw a0, a1, 32\n    srai a0, a1, -1\n    addiw a0, a1, 2048\n    sext.w a0\n\
     negw a0, 5\n    zext.w a0, a1, a2\n",
    "    lr.d a0, 4(a1)\n    sc.d a0, a1\n    amoswap.d.aq a0, a1, 8(a2)\n    sd a0, sym\n    ld a0, %hi(x)(a1)\n\
     lwu a0, 4096(a1)\n",
    "    mulw a0, a1\n    remuw a0, a1, 5\n    li a0, x\n    .equ x, 3\n    addw a0, a1, x99\n",
];

#[test]
fn the_assembler_on_camelot_reports_merlin_as_errors() {
    // Bad inputs: the messages, file and line merlin-as gives.
    let common = [
        "    nop\n    frobnicate a0\n",
        "    addi a0, a1, 5000\n    beq a0, a1, 3\n",
        "    .word \"x\"\n    lw a0, %hi(sym)(a1)\n",
        "1:  nop\n    j 2b\n",
        "    sgt a0, a1\n    sgtu a0, a1, 5\n    sgt a0, x99, a1\n",
        "    .equ x, 1 / 0\n",
        "    addi a0, a1,\n    .bogus 1\n",
        "    .ascii \"\\q\"\n",
        // Data must fit its size; only .word and .dword hold an address.
        "    .byte 1, 256\n    .half x\n    .word -16 >> 2\n    .4byte -0x80000001\n",
        // Line information (merlin-as's line_information_errors, and more).
        ".loc 0 1 0\nnop\n",
        ".file 1 \"a.c\"\n.loc 1 1 0\nnop\n",
        ".file 0 \"d\" \"a.c\"\n.file 2 \"b.h\"\n.loc 2 1 0\nnop\n",
        ".file 0 \"d\" \"a.c\"\n.loc 0 1 0 frobnicate\nnop\n.loc 0 2 0 is_stmt 2\n.loc 0 1 0 isa\n",
        ".file 1 \"a.c\"\n.file 1 \"b.c\"\n.file 0 \"d\" \"a.c\" source \"int x;\"\n.file 0 \"d\" \"a.c\" md5\n",
        ".file 0 \"d\" \"a.c\" md5 \"x\"\n.file\n.file \"a\" \"b\"\n.file 1\n.loc\n.loc 0 x\n",
        ".section .debug_info\nnop\n",
        // An alias's address wraps around in 64 bits, then must be in its
        // section.
        ".data\n.word 0\nx: .word 0\n.equ BIG, 0x7fffffffffffffff\n.set y, x + BIG\n.set z, x - 8\n",
        // .space: no negative count, no section of 4 GiB or more.
        ".data\n.space -1\n.skip -5, 7\n.space 0x100000003\n.word 1\n.space 2, x\n",
        ".bss\n.space 0xffffffff\n.byte 0\n.lcomm b, 1\n",
        // .comm and .lcomm: LLVM's messages, merlin-as's alignment limit, and
        // a size past 32 bits.
        ".local x\n.comm x, 4, 3\n.comm x, -1\n.lcomm y, 16, 8\n.lcomm y, -1\n.comm x, 4, 0x20000\n\
         .lcomm z\n.lcomm w, 0x100000000\n",
        // .section: going back with other attributes (LLVM's messages, the
        // numbers in upper-case hexadecimal), and lines LLVM rejects.
        ".section .foo, \"a\"\n.section .foo, \"aw\"\n.section .foo, \"a\", @nobits\n\
         .section .rodata.cst4, \"a\"\n.section .debug_line_str, \"\", @progbits\n\
         .section .bss, \"aw\", @progbits\n.section .m, \"aM\", @progbits, 4\n\
         .section .m, \"aM\", @progbits, 8\n.section .x, \"0xa\"\n.section .x, \"a\"\n",
        ".section\n.section .foo, a\n.section .foo, \"aq\"\n.section .foo, \"a \"\n\
         .section .foo, \"a\\377\"\n.section .foo, \"aG\", @progbits, grp\n\
         .section .foo, \"ao\", @progbits, sym\n.section .foo, \"a\", @weird\n\
         .section .foo, \"a\", %weird\n.section .foo, \"a\", foo\n.section .foo, \"aM\"\n\
         .section .foo, \"aM\", @progbits\n.section .foo, \"aM\", @progbits, 0\n\
         .section .foo, \"aM\", %progbits, -1\n.section .foo, \"aM\", \"progbits\", 0x100000001\n\
         .section .foo, \"aM\", @progbits, x\n.section .foo, \"a\", @progbits, 1\n\
         .section .foo, \"aM\", @progbits, 1, 2\n",
        ".section .foo, \"a\", %\nnop\n",
        ".section .bss.x\nnop\n.section .b, \"aw\", @nobits\n.byte 1\n.section .debug_x\nnop\n",
        // Another ISA than -march's (the same is accepted: lines 1 to 6 and
        // the attribute 4), RV32's among them, and LLVM's own errors.
        ".option arch, rv64ima_zicsr_zifencei\n.option arch, +m, -c\n.option arch, -a\n\
         .attribute arch, \"rv64i2p1_m2p0_a2p1_zicsr2p0_zifencei2p0_zmmul1p0_zaamo1p0_zalrsc1p0\"\n\
         .attribute 5, \"rv64im_zicsr_zifencei_zaamo_zalrsc\"\n.option norvc\n.attribute 4, 16\n\
         .option arch, +zicond\n.option arch, -m\n.option arch, rv32ima_zicsr_zifencei\n.option rvc\n\
         .option arch, rv32ima2p1_zicsr\n.attribute arch, \"rv32i2p1_m2p0\"\n.attribute 5, \"rv32gc\"\n\
         .option arch, -zmmul\n.option arch, +m2p0\n.option arch, m\n.option arch\n.option\n\
         .attribute arch, 5\n",
        // The line table's files: a number given twice, the main file under
        // a number (which leaves it unassigned), a gap, a table of .file N
        // without .file 0 and without a .loc.
        ".file 0 \"d\" \"a.c\"
.file 1 \"x\" \"a.c\"
.file 2 \"b.c\"
.file 2 \"b.c\"
.loc 1 3
nop
.loc 2 4
nop
",
        ".file 1 \"a.c\"
nop
",
        // Alignment: fills and maxima LLVM takes, and those it does not.
        ".p2align 2, 0x55, 0\n.balign 8,, -1\n.p2align 3,,\n.p2align 2,\n.balign 3\n.p2align 2, 1, 2, 3\n\
         .p2align 2, x\n.balign 8, 0, x\n.p2align 17\n",
    ];
    let inputs: Vec<(String, String)> = common
        .iter()
        .chain(RV64_ERRORS)
        .enumerate()
        .map(|(i, c)| (format!("case{i}"), c.to_string()))
        .collect();
    let mut commands: Vec<String> = (0..inputs.len())
        .map(|i| format!("as /src/{0}.s -o /out/{0}.o", short(i)))
        .collect();
    // The command line: ISAs merlin-as does not take, RV32's among them, as
    // merlin-as says it (its main.rs, with `as` for `merlin-as` before the
    // message), and `-march` without `=`, an unknown option, as in clang.
    let bad_marches = [
        ("rv64gc", "as -march=rv64gc /src/f0.s -o /out/x.o"),
        (
            "rv32ima_zicsr_zifencei",
            "as -march=rv32ima_zicsr_zifencei /src/f0.s -o /out/y.o",
        ),
    ];
    let bare_march = "as -march rv64ima_zicsr_zifencei /src/f0.s";
    commands.extend(bad_marches.iter().map(|(_, c)| c.to_string()));
    commands.push(bare_march.into());
    let input = format!("{}\nexit 0\n", commands.join("\n"));
    let (out, disk) = boot(image(&inputs), &input, 2_000_000_000);
    for (isa, command) in bad_marches {
        assert_eq!(
            shell_output(&out, command).replace("\r", ""),
            format!("as: unsupported -march '{isa}' (merlin-as takes rv64ima_zicsr_zifencei)\nas: exit 2\n")
        );
    }
    let fs = Fs::open(&disk).unwrap();
    assert!(fs.read_file("/out/x.o").is_err() && fs.read_file("/out/y.o").is_err());
    assert!(
        shell_output(&out, bare_march)
            .replace("\r", "")
            .starts_with("as: unknown option -march\nusage: as [-march=ISA] [-o output.o] input.s\n"),
        "{}",
        shell_output(&out, bare_march)
    );
    for (i, (_, text)) in inputs.iter().enumerate() {
        let file = format!("/src/{}.s", short(i));
        let errors = merlin_as::assemble_to_elf(text).unwrap_err();
        let want: String = errors
            .iter()
            .map(|e| match e.line {
                0 => format!("{file}: error: {}\n", e.msg),
                n => format!("{file}:{n}: error: {}\n", e.msg),
            })
            .collect();
        let got = shell_output(&out, &commands[i]);
        let got = got.split("as: exit").next().unwrap();
        assert_eq!(got.replace("\r", ""), want, "case {i}: {text:?}");
    }
}

#[test]
fn the_c_tools_tables_are_the_host_tools() {
    // kernel/user/as/tables.[ch] and kernel/user/ld/tables.[ch] are generated
    // from merlin-as's, excalibur's and the elf crate's tables
    // (crates/xtask/src/c_tables.rs) by `cargo xtask kernel`.
    for (file, want) in xtask::c_tables::files() {
        let path = repo_root().join(file);
        let committed = std::fs::read_to_string(&path).unwrap().replace('\r', "");
        assert!(
            committed == want,
            "{} is out of date: run `cargo xtask kernel` and commit it",
            path.display()
        );
    }
}

/// The second step (#136): the linker that runs on Camelot (`kernel/user/ld`,
/// `/bin/ld`) links every user program from the host's objects into the
/// executable merlin-ld makes, byte for byte, without the line tables (`-S`,
/// as `/bin` holds them); and a few with them too, as `cargo xtask kernel`
/// writes them to target/kernel/obj/user (the debug sections, their
/// relocations and headers): programs of C with the library's members, and
/// `as`, of several files.
#[test]
fn the_linker_on_camelot_links_every_user_program_as_merlin_ld_does() {
    link_every_program(&|name| ["hello", "cat", "grep", "as"].contains(&name));
}

/// Every program with the line tables too: twice the links, and the bigger
/// files.
#[test]
#[ignore]
fn the_linker_on_camelot_keeps_the_line_tables_as_merlin_ld_does() {
    link_every_program(&|_| true);
}

/// Links every user program on Camelot with `-S`, and without it those
/// `with_debug` names, and compares the executables with merlin-ld's. The objects go in `/o`, each program's
/// command line in a response file in `/a` (a link line is longer than a
/// console line), the results in `/out`.
fn link_every_program(with_debug: &(dyn Fn(&str) -> bool + Sync)) {
    use userprogs::CCompiler;
    let dir = userprogs::user_dir(&repo_root());
    let rt = userprogs::build_runtime(&dir, CCompiler::Merlin).unwrap();
    let progs = userprogs::programs(&dir).unwrap();
    // What every machine has: the script, the runtime's objects and the
    // library's members.
    let script = std::fs::read(dir.join(userprogs::SCRIPT)).unwrap();
    let mut shared: Vec<(String, Vec<u8>)> = vec![("/user.ld".into(), script)];
    let mut runtime = Vec::new();
    for (i, o) in rt.objects.iter().enumerate() {
        runtime.push(format!("/o/r{i}.o"));
        shared.push((format!("/o/r{i}.o"), o.data.clone()));
    }
    let mut library = Vec::new();
    for (i, o) in rt.library.iter().enumerate() {
        library.push(format!("/o/l{i}.o"));
        shared.push((format!("/o/l{i}.o"), o.data.clone()));
    }
    // Per program: its own objects, its response file, and what merlin-ld
    // makes of them.
    struct Job {
        name: String,
        files: Vec<(String, Vec<u8>)>,
        want: userprogs::Program,
    }
    let jobs: Vec<Job> = progs
        .iter()
        .enumerate()
        .map(|(k, prog)| {
            let objects = userprogs::program_objects(&dir, prog, CCompiler::Merlin, &rt).unwrap();
            let own = &objects[..objects.len() - rt.objects.len()];
            let mut files = Vec::new();
            let mut args = format!("-T /user.ld -o /out/{k}");
            for (j, o) in own.iter().enumerate() {
                let path = format!("/o/p{k}_{j}.o");
                args.push_str(&format!(" {path}"));
                files.push((path, o.data.clone()));
            }
            args.push_str(&format!(" {}\n--start-lib", runtime.join(" ")));
            for l in &library {
                args.push_str(&format!("\n  {l}"));
            }
            args.push_str("\n--end-lib\n");
            files.push((format!("/a/{k}"), args.into_bytes()));
            Job {
                name: prog.stem.clone(),
                files,
                want: userprogs::link_program(&dir, prog, CCompiler::Merlin, &rt).unwrap(),
            }
        })
        .collect();
    assert!(jobs.len() > 30, "{}", jobs.len());
    // Every MACHINES-th program on each machine.
    let wrong: Vec<String> = std::thread::scope(|scope| {
        let runs: Vec<_> = (0..MACHINES)
            .map(|m| {
                let (jobs, shared) = (&jobs, &shared);
                scope.spawn(move || {
                    let mine: Vec<(usize, &Job)> =
                        jobs.iter().enumerate().skip(m).step_by(MACHINES).collect();
                    let mut files = shared.clone();
                    let mut commands = Vec::new();
                    // (command, output, merlin-ld's executable) for each link.
                    let mut checks: Vec<(String, String, &Job, &Vec<u8>)> = Vec::new();
                    for (k, job) in &mine {
                        files.extend(job.files.iter().cloned());
                        let stripped = format!("/out/{k}s");
                        checks.push((
                            format!("ld @/a/{k} -S -o {stripped}"),
                            stripped,
                            job,
                            &job.want.elf,
                        ));
                        if with_debug(&job.name) {
                            checks.push((format!("ld @/a/{k}"), format!("/out/{k}"), job, &job.want.debug));
                        }
                    }
                    for (c, ..) in &checks {
                        commands.push(c.clone());
                    }
                    let input = format!("{}\nexit 0\n", commands.join("\n"));
                    let (out, disk) = boot(image_of(&["ld"], &["/o", "/a"], &files), &input, 20_000_000_000);
                    let fs = Fs::open(&disk).unwrap();
                    let mut wrong = Vec::new();
                    for (command, file, job, want) in &checks {
                        let said = shell_output(&out, command);
                        match fs.read_file(file) {
                            Ok(got) if got == **want && said.is_empty() => {}
                            Ok(got) => wrong.push(format!(
                                "{} ({command}): {} bytes, merlin-ld's {} (first difference at {:?}){}",
                                job.name,
                                got.len(),
                                want.len(),
                                got.iter().zip(want.iter()).position(|(a, b)| a != b),
                                if said.is_empty() {
                                    String::new()
                                } else {
                                    format!("; it said: {said}")
                                }
                            )),
                            Err(e) => wrong.push(format!(
                                "{} ({command}): no executable ({e}); it said: {said}",
                                job.name
                            )),
                        }
                    }
                    wrong
                })
            })
            .collect();
        runs.into_iter().flat_map(|r| r.join().unwrap()).collect()
    });
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// #23's acceptance: `hello` assembled and linked inside Camelot, by `/bin/as`
/// and `/bin/ld`, is the host's `examples/hello/hello.elf` byte for byte, and
/// it runs (on a machine of its own: it is a bare-metal program). Camelot has
/// no preprocessor: `/bin/as` gets hello.S as merlin's makes it on the host,
/// the text the host's merlin-as assembles, with `-march` given as clang
/// takes it.
#[test]
fn hello_assembled_and_linked_on_camelot_is_the_hosts_and_runs() {
    let root = repo_root();
    let ex = xtask::examples::find("hello").unwrap();
    let host = xtask::examples::build(&root, ex).unwrap();
    assert_eq!(host.elf, std::fs::read(ex.elf_path(&root)).unwrap());
    let files: Vec<(String, Vec<u8>)> = vec![
        ("/src/hello.s".into(), host.assembly[0].text.clone().into_bytes()),
        (
            "/src/link.ld".into(),
            std::fs::read(ex.dir(&root).join("link.ld")).unwrap(),
        ),
    ];
    let commands = [
        "as -march=rv64ima_zicsr_zifencei /src/hello.s -o /out/hello.o".to_string(),
        "ld -T /src/link.ld /out/hello.o -o /out/hello".to_string(),
    ];
    let input = format!("{}\nexit 0\n", commands.join("\n"));
    let (out, disk) = boot(image_of(&["as", "ld"], &["/src"], &files), &input, 2_000_000_000);
    for c in &commands {
        assert_eq!(shell_output(&out, c), "", "{c}");
    }
    let elf = Fs::open(&disk).unwrap().read_file("/out/hello").unwrap();
    assert!(elf == host.elf, "/out/hello is not {}", ex.elf_name());
    // Bare metal: in M-mode, with no firmware.
    let bare = MachineConfig {
        bios: grail::Bios::None,
        ..machine_config()
    };
    let mut m = Machine::new(bare).unwrap();
    m.cpu.bus.uart.capture = true;
    m.load_elf(&elf).unwrap();
    assert_eq!(m.run(Some(10_000_000), |_| {}), RunResult::Exited(0));
    let said = String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned();
    assert!(said.contains("Hello"), "{said:?}");
}

/// An object from merlin-as.
fn object(src: &str) -> Vec<u8> {
    merlin_as::assemble_to_elf(src).unwrap_or_else(|e| panic!("{e:?}\n{src}"))
}

/// A section header field: its offset in an Elf64_Shdr (each of these 8
/// bytes).
#[derive(Clone, Copy)]
struct ShdrField(usize);
const SH_FLAGS: ShdrField = ShdrField(8);
const SH_SIZE: ShdrField = ShdrField(32);
const SH_ADDRALIGN: ShdrField = ShdrField(48);
const SH_ENTSIZE: ShdrField = ShdrField(56);

/// `obj` with field `field` of section `name`'s header set to `f` of its
/// value.
fn shdr_field(mut obj: Vec<u8>, name: &str, field: ShdrField, f: impl Fn(u64) -> u64) -> Vec<u8> {
    let at = {
        let elf = elf::Elf::parse(&obj).unwrap();
        let i = elf.sections.iter().position(|s| s.name == name).unwrap();
        elf.header.shoff as usize + elf::SHDR64_SIZE * i + field.0
    };
    let v = u64::from_le_bytes(obj[at..at + 8].try_into().unwrap());
    obj[at..at + 8].copy_from_slice(&f(v).to_le_bytes());
    obj
}

/// `obj` with its section `name` made a mergeable string section
/// (`SHF_MERGE | SHF_STRINGS`, entries of one byte), as clang's
/// `.rodata.str1.1` is: merlin-as writes none.
fn mergeable(obj: Vec<u8>, name: &str) -> Vec<u8> {
    let obj = shdr_field(obj, name, SH_FLAGS, |flags| {
        flags | elf::SHF_MERGE | elf::SHF_STRINGS
    });
    shdr_field(obj, name, SH_ENTSIZE, |_| 1)
}

/// A program for [`the_linker_on_camelot_links_scripts_as_merlin_ld_does`]:
/// code, read-only data, data with `.dword` (R_RISCV_64) and `.word`
/// pointers, a section the scripts throw away, and debug information
/// pointing at all of them (R_RISCV_64 and R_RISCV_32 in a section that is
/// not loaded, at discarded code with a positive addend too, at a global
/// [`FAR`] defines in discarded code and at `wu`, made a weak reference
/// nobody defines: tombstones, 1 in `.debug_ranges` and `.debug_loc`).
const SCRIPTED: &str = "
        .text
        .globl _start
_start: la a0, table
        lw a1, 0(a0)
        call far
        nop
        .section .text.gone, \"ax\"
gone:   nop
        nop
        .rodata
msg:    .asciz \"hi\"
        .section .rodata.two, \"a\"
        .word 5
        .data
        .p2align 3
table:  .dword _start
        .dword _start - 1
        .dword table + 0x7fffffff
        .dword far + 4
        .word msg
        .bss
        .space 16
        .section .debug_info, \"\", @progbits
        .dword _start
        .dword table + 4
        .quad far
        .word _start
        .dword gone + 4
        .word gone + 4
        .dword fargone + 4
        .word wu + 4
        .section .debug_ranges, \"\", @progbits
        .word gone + 4
        .dword fargone + 4
        .section .debug_loc, \"\", @progbits
        .dword wu + 4
        .word fargone
";

/// The other object of [`SCRIPTED`]'s links, with a global in code the
/// scripts throw away, which [`SCRIPTED`]'s debug information refers to.
const FAR: &str = "
        .text
        .globl far
far:    ret
        .section .text.gone, \"ax\"
        .globl fargone
fargone: nop
";

/// `obj` with its relocations, in section order, given the types and
/// offsets of `relocs`: merlin-as emits no ADD/SUB/SET, 32_PCREL or ALIGN.
/// An Elf64_Rela is r_offset, r_info (the type in its low 32 bits) and
/// r_addend, 8 bytes each.
fn retype(mut obj: Vec<u8>, relocs: &[(u8, u32)]) -> Vec<u8> {
    let (size, word) = (24, 8);
    let at: Vec<usize> = {
        let elf = elf::Elf::parse(&obj).unwrap();
        elf.sections
            .iter()
            .filter(|s| s.ty == elf::SHT_RELA)
            .flat_map(|s| (0..s.size / size).map(move |i| (s.offset + i * size) as usize))
            .collect()
    };
    assert_eq!(at.len(), relocs.len());
    for (&e, &(ty, off)) in at.iter().zip(relocs) {
        obj[e..e + word].fill(0);
        obj[e..e + 4].copy_from_slice(&off.to_le_bytes());
        obj[e + word] = ty;
    }
    obj
}

/// merlin-ld's tests/common's `DIFFS`: label differences and set values in
/// `.data` (ADD/SUB pairs of every width, 32_PCREL, SET32/16/8/6, SUB6),
/// and in `.debug_ranges` against code `/DISCARD/` throws away, once
/// [`retype`] has given its relocations [`DIFFS_RELOCS`].
const DIFFS: &str = "
        .text
        .globl _start
_start: nop
        .section .text.gone, \"ax\"
gone:   nop
        .data
d0:     .byte 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
        .byte 0, 0, 0, 0, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0xff, 0xc3, 0xff
        .zero 3
        .word g, d0, g, d0 + 8, g, d0 + 12, g, d0 + 14
        .word g + 4, g + 4, g + 4, g + 4, g + 4, g + 4, g
        .section .debug_ranges, \"\", @progbits
        .zero 12
        .word gone, _start, gone + 4
";
const DIFFS_RELOCS: [(u8, u32); 18] = [
    (36, 0),
    (40, 0),
    (35, 8),
    (39, 8),
    (34, 12),
    (38, 12),
    (33, 14),
    (37, 14),
    (57, 15),
    (56, 19),
    (55, 23),
    (54, 25),
    (53, 26),
    (52, 27),
    (33, 28),
    (36, 0),
    (40, 0),
    (56, 8),
];

/// `obj` with its global symbol `name` made weak (merlin-as has no
/// `.weak`).
fn weaken(mut obj: Vec<u8>, name: &str) -> Vec<u8> {
    // An Elf64_Sym's st_info is right after st_name.
    let (size, info) = (24, 4);
    let at = {
        let elf = elf::Elf::parse(&obj).unwrap();
        let symtab = elf.sections.iter().find(|s| s.ty == elf::SHT_SYMTAB).unwrap();
        let strtab = elf.sections[symtab.link as usize].offset as usize;
        (0..symtab.size / size)
            .map(|i| (symtab.offset + i * size) as usize)
            .find(|&e| {
                let n = strtab + u32::from_le_bytes(obj[e..e + 4].try_into().unwrap()) as usize;
                obj[n..].split(|&b| b == 0).next() == Some(name.as_bytes())
            })
            .unwrap()
    };
    obj[at + info] = (elf::STB_WEAK << 4) | (obj[at + info] & 0xf);
    obj
}

/// Links that succeed, linked on Camelot as merlin-ld links them, byte for
/// byte, with the debug information and without it (`-S`): load addresses
/// (`AT` on the first section, in the middle with `ADDR` and a NOLOAD
/// section after it, `AT(.)` on a section with the permissions of the one
/// before, an empty section kept because `ADDR` names it), where the headers
/// go (a page in, loaded in a segment of their own, in a read-only first
/// section's, in a MEMORY region, without `.riscv.attributes`, without any
/// section), a segment aligned past the page, a section without inputs
/// after the code, a run of RELRO sections, empty orphans (placed as ld.lld
/// places them), and the file offsets of NOBITS sections and of an empty
/// section whose segment is dropped. The scripts' OUTPUT_FORMAT names the
/// objects' class.
#[test]
fn the_linker_on_camelot_links_scripts_as_merlin_ld_does() {
    let class = "elf64";
    let objects = [weaken(object(SCRIPTED), "wu"), object(FAR)];
    // `.text`, `more`, the rest from `start`.
    let rest = ".rodata : { *(.rodata) } .two : { *(.rodata.two) } .data : { *(.data) } .bss : { *(.bss) } \
                /DISCARD/ : { *(.text.gone) }";
    let from = |start: &str, more: &str| {
        format!("ENTRY(_start) SECTIONS {{ . = {start}; .text : {{ *(.text) }} {more} {rest} }}")
    };
    let scripts: Vec<String> = vec![
        // AT on the first section: the whole image is loaded 1 MiB lower.
        format!("SECTIONS {{ . = 0x80100000; .text : AT(0x80000000) {{ *(.text) }} {rest} }}"),
        // AT in the middle, by ADDR, aligned, with a NOLOAD section after it
        // and an empty one an ADDR keeps.
        "SECTIONS { . = 0x80000000; .text : { *(.text) } .rodata : { *(.rodata) } \
         .two : { *(.rodata.two) } .data : AT(ADDR(.text) + 0x8000) ALIGN(16) { *(.data) } \
         .bss (NOLOAD) : { *(.bss) . += 16; } .keep : { *(.nothing) } here = ADDR(.keep); \
         /DISCARD/ : { *(.text.gone) } }"
            .into(),
        // AT(.) on a read-only section right after another.
        "SECTIONS { . = 0x80000000; .text : { *(.text) } .rodata : { *(.rodata) } \
         .two : AT(.) { *(.rodata.two) } .data : { *(.data) } .bss : { *(.bss) } \
         /DISCARD/ : { *(.text.gone) } }"
            .into(),
        // The headers: no room below the first section (a page in), room
        // (their own segment), a read-only first section (theirs), and a
        // MEMORY region (their own again).
        from("0x80000100", ""),
        from("0x80000400", ""),
        "SECTIONS { . = 0x80000400; .rodata : { *(.rodata) } .text : { *(.text) } \
         .two : { *(.rodata.two) } .data : { *(.data) } .bss : { *(.bss) } \
         /DISCARD/ : { *(.text.gone) } }"
            .into(),
        "MEMORY { RAM (rwx) : ORIGIN = 0x80000400, LENGTH = 1M } SECTIONS { \
         .rodata : { *(.rodata) *(.rodata.two) } > RAM .text : { *(.text) } > RAM \
         .data : { *(.data) } > RAM .bss : { *(.bss) } > RAM /DISCARD/ : { *(.text.gone) } }"
            .into(),
        // Without .riscv.attributes ld.lld counts one program header less,
        // and 0xf4 bytes fit below 0x800000f4.
        "SECTIONS { . = 0x800000f4; .text : { *(.text) } .rodata : { *(.rodata) *(.rodata.two) } \
         .data : { *(.data) } .bss : { *(.bss) } /DISCARD/ : { *(.text.gone) *(.riscv.attributes) } }"
            .into(),
        // A section of only an assignment after the code, which is not
        // executable, and a segment aligned to 8 KiB.
        from("0x80000000", ".marker : { . += 4; } .big : ALIGN(8K) { . += 8; }"),
        // A RELRO section, which ends its segment and counts a program
        // header in the room.
        "SECTIONS { . = 0x80000400; .text : { *(.text) } .rodata : { *(.rodata) } .two : { *(.rodata.two) } \
         .data.rel.ro : { *(.data) } .bss : { *(.bss) } /DISCARD/ : { *(.text.gone) } }"
            .into(),
        // An empty section alone in its run, which is dropped, and a gap
        // before .bss.
        "SECTIONS { . = 0x80000000; .text : { *(.text) } .x : { sym = .; } .data : { *(.data) } \
         . = . + 0x100; .bss : { *(.bss) } .rodata : { *(.rodata) *(.rodata.two) } \
         /DISCARD/ : { *(.text.gone) } }"
            .into(),
        // A top-level `.` moved backward: the code above the rest; with
        // OUTPUT_FORMAT's three arguments, a -freebsd name, and ADDR of
        // /DISCARD/, which is 0.
        format!(
            "OUTPUT_FORMAT(\"{class}-littleriscv-freebsd\", \"{class}-littleriscv\", \"{class}-littleriscv\") \
             ENTRY(_start) SECTIONS {{ . = 0x80100000; .text : {{ *(.text) }} . = 0x80000000; \
             gone = ADDR(/DISCARD/); {rest} }}"
        ),
        // Names with dashes, unquoted (`-` is a character of a name outside
        // expressions, an operator in them), and ALIGN(0), which is 1.
        format!(
            "OUTPUT_FORMAT({class}-littleriscv) ENTRY(_start) SECTIONS {{ . = 0x80000000; . = ALIGN(0); \
             .text-x : ALIGN(0) {{ *(.text .text-none) }} a-b = ADDR(.text-x)-4; {rest} }}"
        ),
        // Empty sections kept because their AT or ALIGN uses a symbol.
        from(
            "0x80000000",
            "lma = 0x80100000; al = 16; .kept : AT(lma) { *(.nothing) } .kept2 : ALIGN(al) { *(.nothing) }",
        ),
        // An empty section at exactly 4 GiB: above the others, so the
        // headers are still loaded below them. It follows the code, so it is
        // not executable and its segment of its own is dropped (after a
        // writable section it would join that one's segment, which would
        // then reach 4 GiB, a 2 GiB file in ld.lld and merlin-ld alike).
        "ENTRY(_start) SECTIONS { . = 0x80000400; .rodata : { *(.rodata) } .two : { *(.rodata.two) } \
         .data : { *(.data) } .bss : { *(.bss) } .text : { *(.text) } /DISCARD/ : { *(.text.gone) } \
         . = 0x100000000; .end : { . += 0; } }"
            .into(),
    ];
    let mut links: Vec<(Vec<Vec<u8>>, String)> = scripts.into_iter().map(|s| (objects.to_vec(), s)).collect();
    // Empty orphans of every rank, one of a script section's name, without
    // MEMORY and with it.
    let orphans = object(
        ".section .ex, \"ax\"
.p2align 4
.section .er, \"a\"
.section .ctors, \"aw\"
         .section .sdata, \"aw\"
.section .eb, \"aw\", @nobits
.section .two, \"a\"
",
    );
    for script in [
        from("0x80000001", "foo = .;"),
        "MEMORY { RAM (rwx) : ORIGIN = 0x80000000, LENGTH = 1M } SECTIONS { .text : { *(.text) } > RAM \
         .data : { *(.data) *(.rodata) *(.rodata.two) *(.bss) } > RAM /DISCARD/ : { *(.text.gone) } }"
            .into(),
    ] {
        links.push((
            vec![objects[0].clone(), objects[1].clone(), orphans.clone()],
            script,
        ));
    }
    // Label differences and set values.
    links.push((
        vec![retype(object(DIFFS), &DIFFS_RELOCS)],
        "SECTIONS { g = 0x100; . = 0x80000000; .text : { *(.text) } .data : { *(.data) } \
         /DISCARD/ : { *(.text.gone) } }"
            .into(),
    ));
    // No section at all: the headers in the last page.
    links.push((
        vec![object("    .globl _start\n    .set _start, 0x80000000\n")],
        "SECTIONS { /DISCARD/ : { *(.text) } }".into(),
    ));

    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut commands = Vec::new();
    // (command, output, merlin-ld's file) for each link.
    let mut checks: Vec<(String, String, Vec<u8>)> = Vec::new();
    for (k, (objects, script)) in links.iter().enumerate() {
        let mut inputs = Vec::new();
        let mut args = format!("-T /s/{k}.ld");
        for (j, data) in objects.iter().enumerate() {
            let path = format!("/s/{k}_{j}.o");
            args.push_str(&format!(" {path}"));
            files.push((path.clone(), data.clone()));
            inputs.push(merlin_ld::Input {
                name: path,
                data: data.clone(),
            });
        }
        files.push((format!("/s/{k}.ld"), script.as_bytes().to_vec()));
        let opts = merlin_ld::Options {
            script: Some(merlin_ld::ScriptSource {
                name: format!("/s/{k}.ld"),
                text: script.clone(),
            }),
            entry: None,
        };
        let linked = merlin_ld::link_with_library(&inputs, &[], &opts)
            .unwrap_or_else(|e| panic!("link {k}: {e:?}\n{script}"));
        assert!(
            linked.layout.warnings.is_empty(),
            "link {k}: {:?}",
            linked.layout.warnings
        );
        let stripped = merlin_ld::strip_debug(&linked.elf).unwrap();
        checks.push((format!("ld {args} -o /out/{k}"), format!("/out/{k}"), linked.elf));
        checks.push((
            format!("ld {args} -S -o /out/{k}s"),
            format!("/out/{k}s"),
            stripped,
        ));
    }
    for (c, ..) in &checks {
        commands.push(c.clone());
    }
    let input = format!("{}\nexit 0\n", commands.join("\n"));
    let (out, disk) = boot(image_of(&["ld"], &["/s"], &files), &input, 2_000_000_000);
    let fs = Fs::open(&disk).unwrap();
    for (command, file, want) in &checks {
        assert_eq!(shell_output(&out, command), "", "{command}");
        let got = fs.read_file(file).unwrap_or_else(|e| panic!("{command}: {e}"));
        assert!(
            got == *want,
            "{command}: {} bytes, merlin-ld's {} (first difference at {:?})",
            got.len(),
            want.len(),
            got.iter().zip(want.iter()).position(|(a, b)| a != b)
        );
    }
}

#[test]
fn the_linker_on_camelot_reports_merlin_ld_errors_and_warnings() {
    // Links that fail, and links that succeed with a warning: what merlin-ld
    // says, word for word, the exit status, and the file (none after an
    // error, merlin-ld's after a warning), and links of 4 GiB that do not
    // fit in memory. Each case is its inputs, a script, and more arguments:
    // `--start-lib` makes the last input a library member. The address
    // space's limit is 2^64, and the class an OUTPUT_FORMAT must name is the
    // objects', ELF64 (ELF32's is refused).
    let obj = object;
    let (class, other) = ("elf64", "elf32");
    let script = "ENTRY(_start)\nSECTIONS\n{\n    . = 0x1000;\n    .text : { *(.text) }\n    .rodata : { *(.rodata) }\n    .data : { *(.data) }\n}\n";
    let start = obj(".text\n.globl _start\n_start: nop\n");
    let truncated = start[..start.len() - 50].to_vec();
    // EI_CLASS 3: neither ELFCLASS32 nor ELFCLASS64.
    let mut bad_class = start.clone();
    bad_class[4] = 3;
    let strings = ".rodata\n.asciz \"hello\"\n.asciz \"world\"\n";
    let with_rodata = obj(".text\n.globl _start\n_start: nop\n.rodata\n.word 1\n");
    // The address `n` bytes below the top of the address space, 2^64.
    let below_top = |n: u64| format!("{:#x}", 0u64.wrapping_sub(n));
    let mut cases: Vec<(Vec<Vec<u8>>, String, &str)> = vec![
        // Errors.
        (
            vec![obj(".text\n.globl _start\n_start: call missing\n    call gone\n")],
            script.into(),
            "",
        ),
        (vec![start.clone(), start.clone()], script.into(), ""),
        (vec![bad_class], script.into(), ""),
        (
            vec![start.clone()],
            "SECTIONS { .text 0x100 : { *(.text) } }".into(),
            "",
        ),
        (
            vec![obj(".text\n.globl _start\n_start: nop\n.section .extra, \"aw\"\n.word 1\n")],
            script.into(),
            "",
        ),
        (
            vec![
                obj(".text\n.globl _start\n_start: beq a0, a1, far\n"),
                obj(".text\n.zero 8192\n.globl far\nfar: nop\n"),
            ],
            script.into(),
            "",
        ),
        // A branch out of range and odd (two errors), R_RISCV_ALIGN, and a
        // SET32 out of range.
        (
            vec![obj(".text\n.globl _start\n_start: beq a0, a1, odd\n")],
            "SECTIONS { . = 0x1000; .text : { *(.text) } odd = 0x1000 + 4097; }".into(),
            "",
        ),
        (
            vec![retype(
                obj(".text\n.globl _start\n_start: nop\n.data\n.word _start\n.word big\n"),
                &[(43, 0), (56, 4)],
            )],
            "SECTIONS { . = 0x1000; .text : { *(.text) } .data : { *(.data) } big = 0x80000000; }".into(),
            "",
        ),
        (vec![start.clone()], script.into(), "-e nosuch"),
        (vec![start.clone()], script.into(), "-e 0x+10"),
        (
            vec![start.clone()],
            "SECTIONS { . = 0x1000; .text : { *(.text) . = . - 0x10; } x = nothere; }".into(),
            "",
        ),
        // A top-level `.` moved backward: the sections over each other.
        (
            vec![with_rodata.clone()],
            "SECTIONS { . = 0x1000; .text : { *(.text) } . = 0x1000; .rodata : { *(.rodata) } }".into(),
            "",
        ),
        // Past the top, and the next section placed after it nonetheless.
        (
            vec![with_rodata.clone()],
            format!(
                "SECTIONS {{ . = {}; .text : {{ *(.text) . += 8; }} .rodata : {{ *(.rodata) }} }}",
                below_top(8)
            ),
            "",
        ),
        (
            vec![obj(".text\n.globl _start\n_start: nop\n.zero 64\n")],
            "MEMORY { RAM : ORIGIN = 0x1000, LENGTH = 32 }\nSECTIONS { .text : { *(.text) } > RAM }".into(),
            "",
        ),
        (
            vec![start.clone()],
            format!(
                "SECTIONS {{ . = {}; .text : {{ *(.text) . += 0x20; }} }}",
                below_top(0x10)
            ),
            "",
        ),
        (
            vec![start.clone()],
            "MEMORY { a : o = 0, l = 1 b : o = 1, l = 1 }".into(),
            "",
        ),
        (
            vec![obj(
                ".text\n.globl _start\n_start: la a0, x\n.data\n.globl x\nx: .word _start\n",
            )],
            "SECTIONS { /DISCARD/ : { *(.data) } .text : { *(.text) } }".into(),
            "",
        ),
        (vec![b"not an object".to_vec()], script.into(), ""),
        (vec![start.clone(), truncated.clone()], script.into(), ""),
        // An alignment that is not a power of two, a symbol table whose size
        // is not a multiple of a symbol's.
        (
            vec![shdr_field(start.clone(), ".text", SH_ADDRALIGN, |_| 12)],
            script.into(),
            "",
        ),
        (
            vec![shdr_field(start.clone(), ".symtab", SH_SIZE, |size| size + 4)],
            script.into(),
            "",
        ),
        // Load addresses: two sections loaded over each other, a load range
        // past the top, AT after ALIGN, /DISCARD/ with an ALIGN not a power
        // of two (its NOLOAD and AT mean nothing, as in ld.lld).
        (
            vec![with_rodata.clone()],
            "SECTIONS { . = 0x1000; .text : { *(.text) } .rodata : AT(0x1000) { *(.rodata) } }".into(),
            "",
        ),
        (
            vec![with_rodata.clone()],
            format!(
                "SECTIONS {{ . = 0x1000; .text : AT({}) {{ *(.text) }} .rodata : AT(0 - 4) {{ *(.rodata) }} }}",
                below_top(2)
            ),
            "",
        ),
        (
            vec![start.clone()],
            "SECTIONS { .text : ALIGN(4) AT(0x100) { *(.text) } }".into(),
            "",
        ),
        (
            vec![start.clone()],
            "SECTIONS { /DISCARD/ (NOLOAD) : AT(nowhere) ALIGN(3) { *(.comment) } }".into(),
            "",
        ),
        // The headers in the read-only first section's segment, whose load
        // address leaves no room below it for them.
        (
            vec![with_rodata.clone()],
            "SECTIONS { . = 0x80000400; .rodata : AT(0x10) { *(.rodata) } .text : { *(.text) } }".into(),
            "",
        ),
        // ADDR: of a section not placed yet, in its own ALIGN, of none.
        (
            vec![start.clone()],
            "SECTIONS { . = 0x1000; x = ADDR(.text); .text : ALIGN(ADDR(.text)) { *(.text) } \
             y = ADDR(.nope); }"
                .into(),
            "",
        ),
        // ALIGN: not a power of two, in an expression and on a section, and
        // a section's above 2^31.
        (
            vec![with_rodata.clone()],
            "SECTIONS { . = 0x1001; . = ALIGN(3); .text : ALIGN(24) { *(.text) } \
             .rodata : ALIGN(0x100000000) { *(.rodata) } }"
                .into(),
            "",
        ),
        // OUTPUT_FORMAT: an unknown name, and ELF32's, a class merlin-ld does
        // not write, with a library member among the files.
        (
            vec![start.clone()],
            format!("OUTPUT_FORMAT(\"{class}-littleriscv\", \"elf64-bigriscv\")"),
            "",
        ),
        (
            vec![start.clone(), obj(".text\n.globl f\nf: nop\n")],
            format!("OUTPUT_FORMAT(\"{other}-littleriscv\")\nSECTIONS {{ . = 0x1000; .text : {{ *(.text) }} }}"),
            "--start-lib",
        ),
        // ld.lld's order: the script read first, its errors before the
        // objects'; then the files, a library member among them, in
        // command-line order.
        (
            vec![start.clone(), truncated.clone()],
            format!("OUTPUT_FORMAT({other}-littleriscv) SECTIONS {{"),
            "",
        ),
        (
            vec![start.clone(), obj(".text\n.globl f\nf: nop\n"), start.clone()],
            format!(
                "OUTPUT_FORMAT({other}-littleriscv) SECTIONS {{ . = 0xffffffffffffffff; \
                 MEMORY_TOP = 0x8000000000000000; }}"
            ),
            "--start-lib 1",
        ),
        // Warnings: no entry symbol (the first code section's address is
        // used), and duplicate strings in mergeable sections.
        (
            vec![obj(".text\nmain: nop\n")],
            "SECTIONS { . = 0x1000; .text : { *(.text) } }".into(),
            "",
        ),
        // The same, loaded elsewhere, and with the headers loaded below it.
        (
            vec![obj(".text\nmain: nop\n")],
            "SECTIONS { . = 0x1400; .text : AT(0x800) { *(.text) } }".into(),
            "",
        ),
        (
            vec![
                mergeable(
                    obj(&format!("{}{strings}", ".text\n.globl _start\n_start: nop\n")),
                    ".rodata",
                ),
                mergeable(obj(strings), ".rodata"),
            ],
            script.into(),
            "",
        ),
    ];
    // The cases whose executable is 4 GiB or more (below), which
    // merlin-ld writes from an image that size in memory. /bin/ld computes
    // the same sizes and offsets, but holds no such image (this machine has
    // 4 MiB), so it stops out of memory, as merlin-ld does when it cannot
    // allocate the file: exit status 2 and no file, where cutting them to
    // 32 bits wrote a file whose headers point past its end.
    let mut huge = cases.len()..cases.len();
    // An R_RISCV_32 whose value does not fit in 32 bits, and
    // a script that reads a 64-bit address past 2^63 for the location
    // counter, then for a section's load address: a link that succeeds,
    // with the warning of no entry symbol, so that its file is compared.
    cases.push((
        vec![obj(
            ".text\n.globl _start\n_start: nop\n.data\n.word _start\n.word far\n",
        )],
        "SECTIONS { . = 0x1000; .text : { *(.text) } .data : { *(.data) } far = 0x100000000; }".into(),
        "",
    ));
    cases.push((
        vec![obj(".text\nmain: nop\n.data\n.dword main\n.word -1\n")],
        "SECTIONS { . = 0x8000000000000000; .text : { *(.text) } .data : AT(0xfffffffffffffff0) \
         { *(.data) } }"
            .into(),
        "",
    ));
    // RV64's hi/lo pairs reach -2^31 - 0x800 to 2^31 - 0x801: an
    // address at 4 GiB is out of range for each kind, HI20 (its LO12_I
    // with it), PCREL_HI20 (la) and CALL_PLT, the value reported the
    // hi part.
    cases.push((
        vec![obj(
            ".text\n.globl _start\n_start: lui a0, %hi(far)\n addi a0, a0, %lo(far)\n la a1, far\n \
             call far\n",
        )],
        "SECTIONS { . = 0x1000; .text : { *(.text) } far = 0x100000000; }".into(),
        "",
    ));
    // 4 GiB into a segment: .text2, code above .text, shares its
    // PT_LOAD, 0x100000004 bytes into it; and an output section of
    // 4 GiB with file contents.
    huge.start = cases.len();
    cases.push((
        vec![obj(
            ".text\n.globl _start\n_start: nop\n.section .text2, \"ax\"\nnop\n",
        )],
        "SECTIONS { . = 0x10000; .text : { *(.text) } . += 0x100000000; .text2 : { *(.text2) } }".into(),
        "",
    ));
    cases.push((
        vec![obj(".text\n.globl _start\n_start: nop\n.data\n.word 1\n")],
        "SECTIONS { . = 0x1000; .text : { *(.text) } .data : { . += 0x100000000; *(.data) } }".into(),
        "",
    ));
    huge.end = cases.len();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut commands = Vec::new();
    // What merlin-ld says and writes for each case.
    enum Want {
        Errors(String),
        Warnings(String, Vec<u8>),
        OutOfMemory,
    }
    let mut wants: Vec<Want> = Vec::new();
    for (k, (objects, script, extra)) in cases.iter().enumerate() {
        // (input, whether it is a library member), in command-line order.
        let mut inputs = Vec::new();
        let mut args = format!("ld -T /e/{k}.ld -o /out/{k}");
        let member = match *extra {
            "--start-lib" => Some(objects.len() - 1),
            e => e.strip_prefix("--start-lib ").map(|j| j.parse().unwrap()),
        };
        for (j, data) in objects.iter().enumerate() {
            let path = format!("/e/{k}_{j}.o");
            let input = merlin_ld::Input {
                name: path.clone(),
                data: data.clone(),
            };
            if member == Some(j) {
                args.push_str(&format!(" --start-lib {path} --end-lib"));
            } else {
                args.push_str(&format!(" {path}"));
            }
            inputs.push((input, member == Some(j)));
            files.push((path, data.clone()));
        }
        files.push((format!("/e/{k}.ld"), script.as_bytes().to_vec()));
        let mut opts = merlin_ld::Options {
            script: Some(merlin_ld::ScriptSource {
                name: format!("/e/{k}.ld"),
                text: script.to_string(),
            }),
            entry: None,
        };
        if let Some(e) = extra.strip_prefix("-e ") {
            opts.entry = Some(e.to_string());
            args.push_str(&format!(" {extra}"));
        }
        commands.push(args);
        if huge.contains(&k) {
            // merlin-ld's layout, without the 4 GiB image it would write: a
            // segment whose file part is past 4 GiB, and nothing to say.
            let plain: Vec<merlin_ld::Input> = inputs.into_iter().map(|(i, _)| i).collect();
            let layout = merlin_ld::layout(&plain, &opts).unwrap_or_else(|e| panic!("case {k}: {e:?}"));
            assert!(layout.warnings.is_empty(), "case {k}: {:?}", layout.warnings);
            let (_, segs) = merlin_ld::write::segments(&layout);
            assert!(
                segs.iter().any(|s| s.filesz > u32::MAX.into()),
                "case {k} fits in 4 GiB"
            );
            wants.push(Want::OutOfMemory);
            continue;
        }
        // As merlin-ld's command line links (/bin/ld's too).
        let inputs: Vec<(&merlin_ld::Input, bool)> = inputs.iter().map(|(i, lib)| (i, *lib)).collect();
        wants.push(match merlin_ld::link_files(&inputs, &opts) {
            Ok(linked) => {
                assert!(!linked.layout.warnings.is_empty(), "case {k} links cleanly");
                let said = linked
                    .layout
                    .warnings
                    .iter()
                    .map(|w| format!("ld: warning: {w}\n"));
                Want::Warnings(said.collect(), linked.elf)
            }
            Err(errors) => Want::Errors(errors.iter().map(|e| format!("ld: error: {e}\n")).collect()),
        });
    }
    let input = format!("{}\nexit 0\n", commands.join("\n"));
    let (out, disk) = boot(image_of(&["ld"], &["/e"], &files), &input, 2_000_000_000);
    let fs = Fs::open(&disk).unwrap();
    for (k, want) in wants.iter().enumerate() {
        let got = shell_output(&out, &commands[k]).replace("\r", "");
        let written = fs.read_file(&format!("/out/{k}")).ok();
        match want {
            // An error: the messages, exit status 1, and no file.
            Want::Errors(said) => {
                assert_eq!(got, format!("{said}ld: exit 1\n"), "case {k}: {}", commands[k]);
                assert!(written.is_none(), "case {k} wrote /out/{k}");
            }
            // More than a Camelot process holds: exit status 2, no file.
            Want::OutOfMemory => {
                assert_eq!(
                    got, "ld: out of memory\nld: exit 2\n",
                    "case {k}: {}",
                    commands[k]
                );
                assert!(written.is_none(), "case {k} wrote /out/{k}");
            }
            // A warning: the messages, success, and merlin-ld's file.
            Want::Warnings(said, elf) => {
                assert_eq!(got, *said, "case {k}: {}", commands[k]);
                assert!(
                    written.as_ref() == Some(elf),
                    "case {k}: /out/{k} is not merlin-ld's"
                );
            }
        }
    }
}
