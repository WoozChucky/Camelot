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
//!
//! The C compiler (#241, `kernel/user/cc`, `/bin/cc1`) comes in stages; its
//! first is the preprocessor: `cc1 -E` on Camelot writes what the host's
//! merlin writes, and gives merlin's diagnostics, for every C and `.S` file
//! of the repository, the diagnostics table's rows of the lexer and the
//! preprocessor, and inputs of every directive and macro form
//! (`PP_INPUTS`). The second is the compiler, `cc1 FILE.c`, the third its
//! `-g`: the same assembly as the host's merlin, line tables and DWARF
//! compile unit included (`G_INPUTS` names files every way `-g` does).

use camelot_fs::mkfs::{Mkfs, Options};
use camelot_fs::read::Fs;
use grail::block::MemDisk;
use grail::{Machine, MachineConfig, RunResult};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use xtask::{kernel, repo_root, toolchain, userprogs};

// The rows of merlin's diagnostics table that preprocessing gives (the
// lexer's and the preprocessor's), for `cc1 -E`: their sources, headers and
// `-D`/`-U` options. The table's builder, for those files: what they say a
// row's diagnostics are is merlin's compilation's, not `-E`'s, so only the
// inputs are kept; the host's merlin gives what `-E` says.
#[path = "../../merlin/tests/diagnostics/lexer.rs"]
mod lexer_rows;
#[path = "../../merlin/tests/diagnostics/preprocessor.rs"]
mod preprocessor_rows;
// And those of the rest of the compiler, for `cc1` compiling (#241's second
// stage): their sources are what counts, compiled by the host's merlin too.
#[path = "../../merlin/tests/diagnostics/codegen.rs"]
mod codegen_rows;
#[path = "../../merlin/tests/diagnostics/parser.rs"]
mod parser_rows;
#[path = "../../merlin/tests/diagnostics/sema.rs"]
mod sema_rows;
#[path = "../../merlin/tests/diagnostics/warnings.rs"]
mod warnings_rows;

/// A row of merlin's diagnostics table: a source, the headers it includes
/// (by name next to it, and text) and its options.
pub struct Case {
    src: String,
    files: Vec<(&'static str, &'static str)>,
    args: Vec<&'static str>,
}

/// A row; its diagnostics are merlin's compilation's, which `-E` does not
/// ask for.
pub fn case(src: impl Into<String>, _want: &[&str]) -> Case {
    Case {
        src: src.into(),
        files: Vec::new(),
        args: Vec::new(),
    }
}

impl Case {
    pub fn files(self, files: &[(&'static str, &'static str)]) -> Case {
        Case {
            files: files.to_vec(),
            ..self
        }
    }

    pub fn args(self, args: &[&'static str]) -> Case {
        Case {
            args: args.to_vec(),
            ..self
        }
    }

    /// What clang says after merlin stops, why a diagnostic is merlin's own,
    /// why the notes are some of clang's: nothing to `-E`.
    pub fn then(self, _: &[&str]) -> Case {
        self
    }

    pub fn merlin(self, _: &'static str) -> Case {
        self
    }

    pub fn partial(self, _: &'static str) -> Case {
        self
    }
}

const PROMPT: &str = "$ ";

/// The corpus: (name on the image, text); `big` adds merlin's assembly for
/// the shell and the kernel's vm.c. A `.S` source (the examples, the boot
/// ROM) goes in as merlin's preprocessor makes it, as `cargo xtask
/// examples` gives it to merlin-as (what `/bin/cc1 -E` writes too), and is
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
    image_with_inodes(bins, dirs, files, Options::default().ninodes)
}

/// [`image_of`], with `ninodes` inodes.
fn image_with_inodes(bins: &[&str], dirs: &[&str], files: &[(String, Vec<u8>)], ninodes: u32) -> Vec<u8> {
    image_with_room(bins, dirs, files, ninodes, 0)
}

/// [`image_with_inodes`], with `room` more bytes free (for what the
/// programs write).
fn image_with_room(
    bins: &[&str],
    dirs: &[&str],
    files: &[(String, Vec<u8>)],
    ninodes: u32,
    room: usize,
) -> Vec<u8> {
    let bytes: usize = files.iter().map(|(_, t)| t.len()).sum();
    let blocks = 2048 + (3 * bytes + room) / camelot_fs::BSIZE;
    let mut fs = Mkfs::new(&Options {
        size: blocks as u32,
        ninodes,
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
    boot_on(machine_config(), image, input, max_steps)
}

/// [`boot`] on a machine of `config`.
fn boot_on(config: MachineConfig, image: Vec<u8>, input: &str, max_steps: u64) -> (String, Vec<u8>) {
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
    // kernel/user/as/tables.[ch], kernel/user/ld/tables.[ch] and
    // kernel/user/cc/tables.[ch] are generated from merlin-as's, excalibur's,
    // the elf crate's and merlin's tables (crates/xtask/src/c_tables.rs) by
    // `cargo xtask kernel`. The kernels embed none of cc1, so this is what
    // tells a stale cc/tables.c.
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
/// as `/bin` holds them); and a few with them too (the debug sections,
/// their relocations and headers): programs of C with the library's
/// members, and `as`, of several files. Each is linked as a C program is on
/// Linux, from the root image's `/usr/lib` (#240): `crt0.o`, the program's
/// objects, then `-L/usr/lib -lc`, which /bin/ld finds and takes the
/// members of `libc.a` it needs from, in ld.lld's order, as merlin-ld
/// does. The installed objects are stripped, so a link without `-S` has
/// the program's own line tables, not the library's.
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

/// The linker's machine: [`machine_config`]'s with 8 MiB of RAM for the
/// kernel instead of 4. `ld` holds a link's objects and the image it builds
/// in memory, as merlin-ld does, and cc1's (1.7 MB of objects, a 1 MB
/// program) take more than 4 MiB leaves the program: on 6 MiB its
/// mappings' pages run out.
fn link_machine_config() -> MachineConfig {
    MachineConfig {
        ram_size: (8 << 20) + grail::firmware::FW_SIZE as usize,
        ..machine_config()
    }
}

/// Links every user program on Camelot with `-S`, and without it those
/// `with_debug` names, and compares the executables with merlin-ld's. The
/// objects go in `/o`, each program's command line in a response file in
/// `/a` (a link line can be longer than a console line), the results in
/// `/out`; the start code, the library and the script are the root image's
/// `/usr` ([`usr_image_files`]).
fn link_every_program(with_debug: &(dyn Fn(&str) -> bool + Sync)) {
    use userprogs::CCompiler;
    let dir = userprogs::user_dir(&repo_root());
    let rt = userprogs::build_runtime(&dir, CCompiler::Merlin).unwrap();
    let progs = userprogs::programs(&dir).unwrap();
    // What every machine has: the root image's /usr.
    let shared = usr_image_files(&rt);
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
            let own = userprogs::program_objects(&dir, prog, CCompiler::Merlin).unwrap();
            let mut files = Vec::new();
            let mut args = format!("-T {USR_LIB}/user.ld -o /out/{k} {USR_LIB}/crt0.o");
            for (j, o) in own.iter().enumerate() {
                let path = format!("/o/p{k}_{j}.o");
                args.push_str(&format!(" {path}"));
                files.push((path, o.data.clone()));
            }
            args.push_str(&format!("\n-L{USR_LIB} -lc\n"));
            files.push((format!("/a/{k}"), args.into_bytes()));
            Job {
                name: prog.stem.clone(),
                files,
                want: installed_link(&dir, prog, &rt),
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
                    let dirs = [USR_DIRS, &["/o", "/a"]].concat();
                    let (out, disk) = boot_on(
                        link_machine_config(),
                        image_of(&["ld"], &dirs, &files),
                        &input,
                        20_000_000_000,
                    );
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

/// `prog` as merlin-ld links it from the root image's `/usr/lib`, the
/// installed (stripped) start code and library: with `-S` it is what `/bin`
/// holds, which this checks, and without it it has the program's own line
/// tables only.
fn installed_link(
    dir: &std::path::Path,
    prog: &userprogs::Source,
    rt: &userprogs::Runtime,
) -> userprogs::Program {
    use userprogs::CCompiler;
    let want = userprogs::link_program(dir, prog, CCompiler::Merlin, rt.installed()).unwrap();
    let host = userprogs::link_program(dir, prog, CCompiler::Merlin, rt).unwrap();
    assert!(want.elf == host.elf, "{}: not /bin's program", prog.stem);
    want
}

/// The root image's `/usr/lib`.
const USR_LIB: &str = xtask::fs::LIB;
/// The directories of the root image's `/usr`, made before its files.
const USR_DIRS: &[&str] = &["/usr", xtask::fs::INCLUDE, xtask::fs::LIB];

/// What the root image's `/usr` holds for the runtime `rt`
/// (`xtask::fs::usr_files`): the header, `crt0.o`, `libc.a` and `user.ld`.
fn usr_image_files(rt: &userprogs::Runtime) -> Vec<(String, Vec<u8>)> {
    xtask::fs::usr_files(&userprogs::user_dir(&repo_root()), rt).unwrap()
}

/// #23's acceptance: `hello` assembled and linked inside Camelot, by `/bin/as`
/// and `/bin/ld`, is the host's `examples/hello/hello.elf` byte for byte, and
/// it runs (on a machine of its own: it is a bare-metal program).
/// `/bin/as` gets hello.S as merlin's preprocessor makes it on the host,
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

/// #240's acceptance: inside Camelot, C programs assembled by `/bin/as`
/// from the assembly the host's merlin wrote (`-g`, as the host builds
/// them) and linked by `/bin/ld` from the root image's `/usr/lib`,
///
/// ```text
/// as -o /o/PROG.o /src/PROG.s
/// ld -T /usr/lib/user.ld -o /out/PROG /usr/lib/crt0.o /o/PROG.o -L/usr/lib -lc
/// ```
///
/// are the host's programs byte for byte: with `-S` what `/bin` holds, and
/// without it merlin-ld's link of the same files, the program's own line
/// tables kept (the installed library has none). Then they run, from `/out`. A program
/// of several files (`/bin/ar`) is linked from its objects in order.
#[test]
fn c_programs_assembled_and_linked_on_camelot_are_the_hosts_and_run() {
    use userprogs::CCompiler;
    let dir = userprogs::user_dir(&repo_root());
    let rt = userprogs::build_runtime(&dir, CCompiler::Merlin).unwrap();
    let mut files = usr_image_files(&rt);
    files.push(("/src/words".into(), b"one two\nthree\n".to_vec()));
    let mut commands: Vec<String> = Vec::new();
    // (program, output, merlin-ld's executable)
    let mut checks: Vec<(String, String, Vec<u8>)> = Vec::new();
    let progs = userprogs::programs(&dir).unwrap();
    for name in ["hello", "cat", "grep", "printenv", "ar"] {
        let prog = progs.iter().find(|p| p.stem == name).unwrap();
        let want = installed_link(&dir, prog, &rt);
        let mut objects = Vec::new();
        for (j, (_, text)) in userprogs::program_assembly(&dir, prog, CCompiler::Merlin)
            .unwrap()
            .into_iter()
            .enumerate()
        {
            let (src, obj) = (format!("/src/{name}{j}.s"), format!("/o/{name}{j}.o"));
            commands.push(format!("as -o {obj} {src}"));
            files.push((src, text.into_bytes()));
            objects.push(obj);
        }
        let link = format!(
            "-T {USR_LIB}/user.ld {USR_LIB}/crt0.o {} -L{USR_LIB} -lc",
            objects.join(" ")
        );
        files.push((format!("/a/{name}"), link.into_bytes()));
        commands.push(format!("ld @/a/{name} -S -o /out/{name}"));
        commands.push(format!("ld @/a/{name} -o /out/{name}.g"));
        checks.push((name.into(), format!("/out/{name}"), want.elf));
        checks.push((name.into(), format!("/out/{name}.g"), want.debug));
    }
    let runs = [
        ("/out/hello", "hello from user mode\n"),
        ("/out/cat /src/words", "one two\nthree\n"),
        ("/out/grep -n three /src/words", "2:three\n"),
        ("/out/printenv HOME", "/\n"),
        ("/out/ar t /usr/lib/libc.a environ.o", "environ.o\n"),
    ];
    for (run, _) in &runs {
        commands.push(run.to_string());
    }
    let input = format!("{}\nexit 0\n", commands.join("\n"));
    let dirs = [USR_DIRS, &["/src", "/o", "/a"]].concat();
    let (out, disk) = boot(image_of(&["as", "ld"], &dirs, &files), &input, 20_000_000_000);
    for c in &commands[..commands.len() - runs.len()] {
        assert_eq!(shell_output(&out, c), "", "{c}");
    }
    let fs = Fs::open(&disk).unwrap();
    for (name, file, want) in &checks {
        let got = fs.read_file(file).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert!(
            got == *want,
            "{name}: {file} is {} bytes, the host's {} (first difference at {:?})",
            got.len(),
            want.len(),
            got.iter().zip(want).position(|(a, b)| a != b)
        );
    }
    for (run, said) in runs {
        assert_eq!(shell_output(&out, run).replace('\r', ""), said, "{run}");
    }
}

/// #240's acceptance: `/bin/ar` writes, for the same members, the archives
/// llvm-ar wrote (the elf crate's fixtures, crates/elf/tests/archive, whose
/// names fit a Camelot directory or are already in an archive), and the
/// root image's `libc.a` from the library's objects, a response file of
/// them or batches added with `r`. `t` lists, `x` takes members out, and a
/// file that is not an archive or a member that is missing is an error.
#[test]
fn the_archiver_on_camelot_writes_what_llvm_ar_writes() {
    let golden = repo_root().join("crates/elf/tests/archive");
    let read = |f: &str| std::fs::read(golden.join(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
    let rt =
        userprogs::build_runtime(&userprogs::user_dir(&repo_root()), userprogs::CCompiler::Merlin).unwrap();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for f in ["a.o", "b.o", "c.o", "nosyms.o", "notes.txt", "more.txt"] {
        files.push((format!("/src/{f}"), read(f)));
    }
    files.push(("/src/new/a.o".into(), read("new/a.o")));
    for f in ["objs.a", "longnames.a", "nosyms.a", "short.a"] {
        files.push((format!("/g/{f}"), read(f)));
    }
    let members: Vec<String> = rt.library.iter().map(|m| format!("/m/{}", m.name)).collect();
    for (path, m) in members.iter().zip(&rt.library) {
        files.push((path.clone(), m.data.clone()));
    }
    files.push((
        "/a/libc".into(),
        format!("rcs /out/libc.a {}\n", members.join("\n")).into_bytes(),
    ));
    // (command, what it prints)
    let mut commands: Vec<(String, String)> = vec![
        // Made from files.
        (
            "ar rcs /out/short.a /src/a.o /src/nosyms.o /src/b.o /src/c.o /src/notes.txt".into(),
            "".into(),
        ),
        ("ar -rcs /out/nosyms.a /src/nosyms.o".into(), "".into()),
        ("ar rcs /out/empty.a".into(), "".into()),
        // Text files only: no symbol table, as llvm-ar writes it.
        (
            "ar rcs /out/text.a /src/notes.txt /src/more.txt".into(),
            "".into(),
        ),
        // Two members of one name: each name given stands for one, the
        // first not yet taken, as in llvm-ar.
        ("ar rcs /out/dup.a /src/a.o /src/new/a.o".into(), "".into()),
        ("ar t /out/dup.a a.o".into(), "a.o\n".into()),
        ("ar t /out/dup.a a.o a.o".into(), "a.o\na.o\n".into()),
        (
            "ar t /out/dup.a a.o a.o a.o".into(),
            "a.o\na.o\nar: error: 'a.o' was not found\nar: exit 1\n".into(),
        ),
        ("mkdir /z".into(), "".into()),
        ("cd /z".into(), "".into()),
        ("ar x /out/dup.a a.o".into(), "".into()),
        ("cd /".into(), "".into()),
        (
            "ar r /out/loud.a /src/c.o".into(),
            "ar: creating /out/loud.a\n".into(),
        ),
        // Members replaced in place and added, in an archive with long names.
        ("cp /g/objs.a /out/replaced.a".into(), "".into()),
        ("ar rcs /out/replaced.a /src/new/a.o /src/c.o".into(), "".into()),
        // Archives llvm-ar wrote, written again as they are, and without
        // their symbol table.
        ("cp /g/longnames.a /out/longnames.a".into(), "".into()),
        ("ar rcs /out/longnames.a".into(), "".into()),
        ("cp /g/longnames.a /out/nosymtab.a".into(), "".into()),
        ("ar rcS /out/nosymtab.a".into(), "".into()),
        ("cp /g/objs.a /out/objs.a".into(), "".into()),
        ("ar rcs /out/objs.a".into(), "".into()),
        // libc.a: from a response file, and in batches.
        ("ar @/a/libc".into(), "".into()),
    ];
    for batch in members.chunks(6) {
        commands.push((format!("ar rcs /out/libc2.a {}", batch.join(" ")), "".into()));
    }
    let names: String = rt.library.iter().map(|m| format!("{}\n", m.name)).collect();
    commands.extend([
        ("ar t /out/libc.a".to_string(), names),
        (
            "ar t /g/objs.a b.o a_very_long_member_name.o".into(),
            "b.o\na_very_long_member_name.o\n".into(),
        ),
        ("mkdir /x".into(), "".into()),
        ("cd /x".into(), "".into()),
        ("ar x /out/libc.a printf.o usys.o".into(), "".into()),
        ("cd /".into(), "".into()),
        ("mkdir /y".into(), "".into()),
        ("cd /y".into(), "".into()),
        ("ar x /g/short.a".into(), "".into()),
        ("cd /".into(), "".into()),
        (
            "ar t /src/notes.txt".into(),
            "ar: error: '/src/notes.txt': not an archive (no !<arch> magic)\nar: exit 1\n".into(),
        ),
        (
            "ar x /out/libc.a nope.o".into(),
            "ar: error: 'nope.o' was not found\nar: exit 1\n".into(),
        ),
        (
            "ar rcs /out/bad.a /src/none.o".into(),
            "ar: error: '/src/none.o': no such file or directory\nar: exit 1\n".into(),
        ),
        // /bin/ld says why it cannot read a file too.
        (
            "ld /src/none.o".into(),
            "ld: cannot read /src/none.o: no such file or directory\nld: exit 2\n".into(),
        ),
    ]);
    let input = format!(
        "{}\nexit 0\n",
        commands
            .iter()
            .map(|(c, _)| c.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    let dirs = ["/src", "/src/new", "/g", "/m", "/a"];
    let (out, disk) = boot(
        image_of(&["ar", "cp", "ld", "mkdir"], &dirs, &files),
        &input,
        4_000_000_000,
    );
    for (c, said) in &commands {
        assert_eq!(shell_output(&out, c).replace('\r', ""), *said, "{c}");
    }
    let fs = Fs::open(&disk).unwrap();
    let mut wants: Vec<(&str, Vec<u8>)> = [
        "short.a",
        "nosyms.a",
        "empty.a",
        "replaced.a",
        "longnames.a",
        "nosymtab.a",
        "objs.a",
        "text.a",
    ]
    .into_iter()
    .map(|f| (f, read(f)))
    .collect();
    let c_only = [elf::archive::NewMember {
        name: "c.o",
        data: &read("c.o"),
    }];
    wants.push(("loud.a", elf::archive::write(&c_only).unwrap()));
    let (a, new_a) = (read("a.o"), read("new/a.o"));
    let dup = [
        elf::archive::NewMember {
            name: "a.o",
            data: &a,
        },
        elf::archive::NewMember {
            name: "a.o",
            data: &new_a,
        },
    ];
    wants.push(("dup.a", elf::archive::write(&dup).unwrap()));
    wants.push(("libc.a", rt.archive.clone()));
    wants.push(("libc2.a", rt.archive.clone()));
    for (f, want) in &wants {
        let got = fs
            .read_file(&format!("/out/{f}"))
            .unwrap_or_else(|e| panic!("/out/{f}: {e}"));
        assert!(
            got == *want,
            "/out/{f}: {} bytes, llvm-ar's {} (first difference at {:?})",
            got.len(),
            want.len(),
            got.iter().zip(want).position(|(a, b)| a != b)
        );
    }
    let member = |n: &str| &rt.library.iter().find(|m| m.name == n).unwrap().data;
    assert_eq!(&fs.read_file("/x/printf.o").unwrap(), member("printf.o"));
    assert_eq!(&fs.read_file("/x/usys.o").unwrap(), member("usys.o"));
    assert!(fs.read_file("/x/malloc.o").is_err());
    assert_eq!(fs.read_file("/z/a.o").unwrap(), a, "x takes the first a.o");
    for f in ["a.o", "nosyms.o", "b.o", "c.o", "notes.txt"] {
        assert_eq!(fs.read_file(&format!("/y/{f}")).unwrap(), read(f), "/y/{f}");
    }
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
    // Archives (#240), which an input is when its bytes are one, whatever
    // its name: a member that is not a relocatable object (a text file, an
    // executable) is left out with a warning, a member's error names it
    // lib.a(m.o), an archive alone links what the entry symbol extracts
    // (nothing here: no code, and the entry's warning), a corrupt archive
    // and a thin one are errors, a symbol table pointing between members
    // is never read, the entry symbol extracts the member that defines it
    // and so does a symbol the script names (each with a text member
    // besides, which warns).
    let ar = |members: &[(&str, &[u8])]| {
        let members: Vec<elf::archive::NewMember> = members
            .iter()
            .map(|&(name, data)| elf::archive::NewMember { name, data })
            .collect();
        elf::archive::write(&members).unwrap()
    };
    let calls_f = obj(".text\n.globl _start\n_start: call f\n");
    let f = obj(".text\n.globl f\nf: nop\n");
    let f_undefined = obj(".text\n.globl f\nf: call nowhere\n");
    let lib = ar(&[("notes.txt", b"text"), ("f.o", &f)]);
    let mut bad_table = lib.clone();
    // The first symbol's member offset, past the count: off by two.
    bad_table[8 + 60 + 7] += 2;
    let truncated_ar = lib[..lib.len() - 3].to_vec();
    let mut exe = f.clone();
    exe[16] = 2;
    let with_exe = ar(&[("exe.o", &exe), ("f.o", &f)]);
    let names_g = format!("{}    g = f;\n}}\n", script.trim_end().strip_suffix('}').unwrap());
    cases.extend([
        (vec![calls_f.clone(), lib.clone()], script.to_string(), ""),
        (
            vec![calls_f.clone(), ar(&[("f_undefined.o", &f_undefined)])],
            script.into(),
            "",
        ),
        (vec![lib.clone()], script.into(), ""),
        (vec![calls_f.clone(), truncated_ar], script.into(), ""),
        (vec![calls_f.clone(), bad_table], script.into(), ""),
        (
            vec![f.clone(), ar(&[("readme", b"odd"), ("start.o", &calls_f)])],
            script.into(),
            "",
        ),
        (vec![calls_f.clone(), with_exe], script.into(), ""),
        (vec![b"!<thin>\n".to_vec()], script.into(), ""),
        (vec![start.clone(), lib.clone()], names_g, ""),
        (vec![calls_f.clone(), f.clone()], script.into(), "-lnope"),
    ]);
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
        if let Some(name) = extra.strip_prefix("-l") {
            // A library no -L directory has (there is none): ld says so
            // before it reads a file.
            args.push_str(&format!(" -L/e {extra}"));
            commands.push(args);
            let e = merlin_ld::search_library(name, &["/e".into()], |_| false).unwrap_err();
            wants.push(Want::Errors(format!("ld: error: {e}\n")));
            continue;
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

// ---- The C compiler, stage 1: the preprocessor (#241) ----

/// Where the repository's sources are on the image of the preprocessor's
/// test: under `/r`, each at its path in the repository ([`image_path`]).
const REPO: &str = "/r";

/// The longest name a Camelot directory entry holds (DIRSIZ,
/// kernel/include/fs.h).
const DIRSIZ: usize = 14;

/// A name of a path on the image: the repository's, or for one longer than
/// [`DIRSIZ`] (merlin's test cases have some) its first 9 bytes, `_` and 4
/// hex digits of its hash. No file includes one of those by name.
fn image_name(name: &str) -> String {
    if name.len() <= DIRSIZ {
        return name.to_string();
    }
    let hash = name
        .bytes()
        .fold(2166136261u32, |h, b| (h ^ u32::from(b)).wrapping_mul(16777619));
    format!("{}_{:04x}", &name[..9], hash & 0xffff)
}

/// The image path of `rel`, a path relative to the repository's root.
fn image_path(rel: &str) -> String {
    let parts: Vec<String> = rel.split('/').map(image_name).collect();
    format!("{REPO}/{}", parts.join("/"))
}

/// The repository's C files, headers and `.S` files, by path from the root
/// with `/`, in order: all but those in `target/` and hidden directories
/// (`.git`, worktrees), and the firmware's (`firmware/`), whose headers'
/// names are longer than a Camelot directory entry holds.
fn repo_sources() -> Vec<String> {
    fn walk(root: &std::path::Path, dir: &std::path::Path, found: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            if p.is_dir() {
                if name != "target" && name != "firmware" && !name.starts_with('.') {
                    walk(root, &p, found);
                }
            } else if p.extension().is_some_and(|x| x == "c" || x == "h" || x == "S") {
                found.push(p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let root = repo_root();
    let mut found = Vec::new();
    walk(&root, &root, &mut found);
    found.sort();
    found
}

/// One `cc1 -E`: the input file (an image path), the options before it, as
/// cc1 is given them, and as merlin is.
struct PpJob {
    input: String,
    args: Vec<String>,
    opts: merlin::Options,
}

impl PpJob {
    /// `input` of language `lang`, with these `-I` directories, `-D` and
    /// `-U` options (`-DNAME=VALUE`, `-UNAME`), and `-P` if not
    /// `line_markers`.
    fn new(
        input: String,
        lang: merlin::Lang,
        includes: &[String],
        macros: &[&str],
        line_markers: bool,
    ) -> PpJob {
        let mut args = Vec::new();
        if !line_markers {
            args.push("-P".to_string());
        }
        for d in includes {
            args.push(format!("-I{d}"));
        }
        let mut cmd = Vec::new();
        for m in macros {
            args.push(m.to_string());
            cmd.push(match (m.strip_prefix("-D"), m.strip_prefix("-U")) {
                (Some(d), _) => merlin::CmdMacro::Define(d.to_string()),
                (_, Some(u)) => merlin::CmdMacro::Undef(u.to_string()),
                _ => panic!("{m}: -D or -U"),
            });
        }
        // A file whose name says no language is given one.
        if lang != merlin::Lang::of_file(&input) {
            args.push(match lang {
                merlin::Lang::C => "-xc".to_string(),
                merlin::Lang::AssemblerWithCpp => "-xassembler-with-cpp".to_string(),
            });
        }
        let opts = merlin::Options {
            include_dirs: includes.iter().map(PathBuf::from).collect(),
            macros: cmd,
            lang,
            line_markers,
            ..merlin::Options::default()
        };
        PpJob { input, args, opts }
    }

    fn command(&self, out: &str) -> String {
        format!("cc1 -E {} {} -o {out}", self.args.join(" "), self.input).replace("  ", " ")
    }
}

/// What `merlin -E` makes of `job` on the host, reading `files` (image path,
/// contents) for its input and headers: the text it writes, if it
/// succeeds, and what it prints on stderr.
fn merlin_e(job: &PpJob, files: &BTreeMap<String, Vec<u8>>) -> (Option<String>, String) {
    let text = |p: &str| files.get(p).and_then(|b| String::from_utf8(b.clone()).ok());
    // merlin's paths are the host's (`\` on Windows); the image's are `/`.
    let load = |p: &std::path::Path| text(&p.to_string_lossy().replace('\\', "/"));
    let src = text(&job.input).unwrap_or_else(|| panic!("{}: no such file", job.input));
    let render = |errors: &[merlin::Error]| -> String {
        errors
            .iter()
            .map(|e| format!("{}\n", e.render(&job.input)))
            .collect()
    };
    if job.opts.lang == merlin::Lang::AssemblerWithCpp {
        match merlin::preprocess_asm(&job.input, &src, &job.opts, &load) {
            Ok(t) => (Some(t), String::new()),
            Err(e) => (None, render(&e)),
        }
    } else {
        match merlin::preprocess(&job.input, &src, &job.opts, &load) {
            Ok(pp) => (
                Some(merlin::preprocess::spell(&pp.tokens)),
                render(&pp.diagnostics),
            ),
            Err(e) => (None, render(&e)),
        }
    }
}

/// Inputs of every directive and macro form, written for this test, with
/// their headers (by name next to the input), their `-I`, `-D` and `-U`
/// options (`{dir}` the input's directory), and whether they are assembly. Those that end in an error are
/// there for the diagnostics.
#[allow(clippy::type_complexity)]
const PP_INPUTS: &[(&str, &[(&str, &str)], &[&str], bool)] = &[
    // Rescanning, hide sets, recursion, variadic macros, GNU's `args...`.
    (
        "#define ID(x) x\n#define TWICE(x) x x\n#define SELF SELF + 1\n#define A B\n#define B A\n\
         ID(TWICE(1)) SELF A B ID(ID)(2) ID\n(3) ID (4)\n#define CALL(f, ...) f(__VA_ARGS__)\n\
         CALL(ID, 7) CALL(TWICE, a, b) CALL(ID)\n#define GNU(args...) [args]\nGNU() GNU(1, 2)\n\
         #define L(x) x + L\nL(L)(1) L(L(1))\n#define PAREN (\n#define F(x) <x>\nF PAREN 1)\n",
        &[],
        &[],
        false,
    ),
    // Stringizing and pasting: spaces, quotes, backslashes, placemarkers.
    (
        "#define S(x) #x\n#define XS(x) S(x)\n#define CAT(a, b) a ## b\n#define XCAT(a, b) CAT(a, b)\n\
         S(  a  \"b\\n\"  'c'  ) S() XS(__LINE__) S(a\\\\) CAT(x, 1) CAT(, y) CAT(z, ) CAT(,)\n\
         XCAT(CAT(1, 2), 3) CAT(<, <=) CAT(-, >) CAT(., 5) CAT(L, 'a') XS(CAT(a b, c d))\n\
         #define V(fmt, ...) f(fmt, ## __VA_ARGS__)\nV(1) V(1, 2)\nS(\\)\n",
        &[],
        &[],
        false,
    ),
    // Pasting that forms no token: an error in C.
    ("#define CAT(a, b) a ## b\nCAT(+, -) CAT(/, /)\n", &[], &[], false),
    // Spacing: what would read as other tokens gets a space.
    (
        "#define M -\n#define E\n#define P +\n-M M- -E- P+ +P a/**/b x/E/y 1 E.2 .E 3 L E\"s\" u8 E\"t\"\n\
         M>M a E= b <E< x E## y 0x E1 1E+ E2 .. E. #E# %E> <E: & E&\n",
        &[],
        &[],
        false,
    ),
    // #if arithmetic, with clang's warnings.
    (
        "#if 1 + 2 * 3 == 7 && (1 ? 2 : 3) == 2 && -1 < 0 && -1 > 0u\nok1\n#endif\n\
         #if (2 || 1 / 0) && !(0 && 1 / 0) && (1, 2) == 2 && (0 ? 1 / 0 : 4) == 4\nok2\n#elif 1\nno\n#else\nno\n#endif\n\
         #if defined(X) || defined Y || __LINE__ == 0\nno\n#elif 'a' == 97 && '\\377' == 255 && 'ab' == 24930\nok3\n#endif\n\
         #if 0x7fffffffffffffff + 1 < 0 && 18446744073709551615 == -1 && 1 << 63 < 0\nok4\n#endif\n\
         #if 7 / -2 == -3 && 7 % -2 == 1 && -8 >> 1 == -4 && 1 << 64 == 0 && -9223372036854775807 - 1 < 0\nok5\n#endif\n\
         #if -0x8000000000000000 && 0x8000000000000000 / -1 && ~0u == 18446744073709551615u && 10ULL > 9ll\nok6\n#endif\n\
         #if 01 == 1 && 0x10 == 16 && 010 == 8 && 'abcde' && '\\x41' == 65\nok7\n#endif\n",
        &[],
        &[],
        false,
    ),
    // #if's errors.
    ("#if f(1)\n#endif\n#if 1 :\n#endif\n#if (1 ? 2)\n#endif\n#if 1.5e3\n#endif\n#if 08\n#endif\n", &[], &[], false),
    // __FILE__, __LINE__, #line and line markers.
    (
        "__FILE__ __LINE__\n#line 100\n__LINE__\n#line 7 \"renamed.c\"\n__FILE__ __LINE__\n\
         # 20 \"marker.h\" 1\n__FILE__ __LINE__\n# 30 \"back.c\" 2\n__LINE__\n#line 010\n__LINE__\n\
         #define N 40\n#line N \"x\\\\y.c\"\n__FILE__\n# 50 \"z.c\" 1 3 4\n__LINE__\n",
        &[],
        &[],
        false,
    ),
    // Headers: guarded, once, next to the including file, through a macro,
    // angled through -I, angled found only as quoted (an error).
    (
        "#include \"g.h\"\n#include \"g.h\"\n#include \"once.h\"\n#include \"once.h\"\n#include \"sub/s.h\"\n\
         #define H \"g.h\"\n#include H\n#define AH <inc.h>\n#include AH\n#include <inc.h> // again\n\
         #include \"./sub/s.h\"\nend __FILE__\n",
        &[
            ("g.h", "#ifndef G_H\n#define G_H\ng __FILE__\n#endif\n"),
            ("once.h", "#pragma once\nonce __LINE__\n"),
            ("sub/s.h", "s __FILE__\n#include \"t.h\"\n"),
            ("sub/t.h", "t __FILE__ __LINE__\n"),
            ("inc/inc.h", "#if !defined(INC_H)\n#define INC_H\ninc __FILE__\n#endif\n"),
        ],
        &["-I{dir}/inc"],
        false,
    ),
    ("#include <g.h>\n", &[("g.h", "g\n")], &[], false),
    // Pragmas and #ident pass on; the null directive; #warning.
    (
        "#pragma weird  a   b\n#pragma once\n#define V \"v1\"\n#ident V\n#sccs \"v2\" x\n#\n# /* nothing */\n\
         int x;\n#warning careful /* c */ here \\\n  more\n",
        &[],
        &[],
        false,
    ),
    ("#error stop  here \\\n and here\n", &[], &[], false),
    // Trigraphs, line splices, comments.
    (
        "??=define T ??( ??) ??< ??> ??! ??' ??-\nT\nin\\\nt x = 1;\n#define SP 1 \\  \n + 2\nSP\n\
         // comment ??/\nstill comment\n\"??/\"\" '??''\n/* a /* b */ c\n/\\\n* split */ d\n",
        &[],
        &[],
        false,
    ),
    // A byte-order mark, whitespace past ASCII, characters that are no token.
    ("\u{feff}int a;\u{a0}int b;\u{3000}c \u{e9} @ ` \\ $\n'x \"y\n'' \n", &[], &[], false),
    // A macro's arguments across lines and directives; an expansion to
    // nothing hands its spacing on.
    (
        "#define F(x) [x]\nF(1\n#define Y 2\nY)\nF\n(3)\n#define E\nE int a;\n  E b\nE\n x E\n",
        &[],
        &[],
        false,
    ),
    ("#define F(x) [x]\nF(1\n#include \"x.h\"\n)\n", &[], &[], false),
    // `defined` from an expansion.
    (
        "#define D defined(D)\n#if D\nyes\n#endif\n#define FD(x) defined(x)\n#if FD(FD)\nyes\n#endif\n",
        &[],
        &[],
        false,
    ),
    // -D and -U, in order.
    (
        "ONE TWO F(3) EMPTY __STDC_VERSION__ __STDC_HOSTED__ __riscv_xlen __merlin__ __SIZE_TYPE__\n",
        &[],
        &["-DONE", "-DTWO=2", "-DF(x)=(x+1)", "-UONE", "-DEMPTY="],
        false,
    ),
    ("x\n", &[], &["-D1=2", "-DF(=x", "-U__FILE__"], false),
    // Redefinitions, builtins.
    (
        "#define R 1\n#define R 1\n#define R  2\n#define R(x) x\n#undef __FILE__\n#define __LINE__ 5\n__LINE__\n\
         #undef __STDC__\n#define __STDC_HOSTED__ 2\n",
        &[],
        &[],
        false,
    ),
    // An unterminated call at the end of the file.
    ("#define F(x) x\nF(1, 2", &[], &[], false),
    // NUL bytes in literals, in what is stringized and passed on, in a
    // name #line gives (__FILE__, a diagnostic's file) and in a warning.
    (
        "#if '\0' == 0\nzero\n#endif\n#if 'a\0' == 0x6100\nmulti\n#endif\n#define S(x) #x\nS(p\0q) S(\"\0\")\n\
         #pragma x\0y\n#ident \"a\0b\"\n#line 1 \"a\0b\"\n__FILE__ __LINE__\n#warning w\0arn\n",
        &[],
        &[],
        false,
    ),
    // ... in an #error, in what pasting forms, in a header's name.
    (
        "#error a\0b\n#define CAT(a, b) a ## b\nCAT(x, \0)\n#include \"g.h\0x\"\n",
        &[("g.h", "g\n")],
        &[],
        false,
    ),
    // ... after a backslash: an escape C has not, in a character constant
    // and in a name #line gives.
    ("#if '\\\0' == 0\nz\n#endif\n#line 1 \"\\\0\"\nx\n", &[], &[], false),
    // ... in assembly's line markers.
    ("  nop\n#line 5 \"n\0m.S\"\n  nop\n", &[], &[], true),
    // The bytes that stand around a file's number in merlin's messages
    // (FILE_MARK, U+0001), in a message that quotes the source: the place
    // named, a number no file has, a mark without its pair.
    (
        "#warning x\u{1}y\n#warning a\u{1}0\u{1}b\u{1}7\u{1}c\n#error a\u{1}+0\u{1}b\u{1}\n",
        &[],
        &[],
        false,
    ),
    // Assembly: comments, pasting that forms no token, `#` before anything.
    (
        "# a comment\n#if 1\n  li a0, 1 # trailing\n#endif\n#define CALL(f) call f ## @plt\n\
         #define DOT(n) . ## n\n#define HASH(x) # x y\n\tCALL(foo)\n\tDOT(text)\n\tHASH(z)\n\
         $x: .word 'c', '\\n'\n\\rs1 ??= 0\n/* examples/*/ x */\n# 2^64 - 1\n#define EMPTY\nEMPTY li a0, 2\n\
         \n\n\n\n\n\n\n\n\n\n\nfar: nop\n",
        &[],
        &[],
        true,
    ),
    // Assembly's line markers: headers, blank lines, #line.
    (
        "start:\n#include \"h.S\"\n  after\n#include \"sub/i.S\"\n\n\n\n  later\n#line 40 \"other.S\"\n  renamed\n\
         \n\n\n\n\n\n\n\n\n\n  far __LINE__ __FILE__\n",
        &[
            ("h.S", "\n  in_h\n#define HM 1\n"),
            ("sub/i.S", "  in_i HM\n#include \"j.S\"\n  back_in_i\n"),
            ("sub/j.S", "#ifndef J\n#define J\n  in_j\n#endif\n"),
        ],
        &[],
        true,
    ),
];

/// A source to preprocess in a directory of its own: its text, its headers
/// (name, text), its options and whether it is assembly.
type PpInput = (String, Vec<(&'static str, &'static str)>, Vec<&'static str>, bool);

/// Every `cc1 -E` the preprocessor's test runs, and the files the image
/// holds for them (image path, contents): every C and `.S` file of the
/// repository ([`repo_sources`]) as the builds preprocess them (the kernel's
/// with `-I kernel/include`, `.S` files with `-I include` too, the linker
/// script also with `-P`, a multi-file test case with its `include/`),
/// `include/asm.h` as assembly, the lexer's and the preprocessor's rows of
/// merlin's diagnostics table, each with its headers in `/d/N`, and
/// [`PP_INPUTS`], in `/d/N` after them.
fn pp_jobs() -> (Vec<PpJob>, BTreeMap<String, Vec<u8>>) {
    let root = repo_root();
    let sources = repo_sources();
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for rel in &sources {
        let path = image_path(rel);
        assert!(!files.contains_key(&path), "{rel}: two files at {path}");
        files.insert(path, std::fs::read(root.join(rel)).unwrap());
    }
    let include = image_path("include");
    let kernel_include = image_path("kernel/include");
    let mut jobs = Vec::new();
    for rel in &sources {
        let kernel = rel.starts_with("kernel/") && !rel.starts_with("kernel/user/");
        if rel.ends_with(".c") {
            let mut includes = Vec::new();
            if kernel {
                includes.push(kernel_include.clone());
            }
            // A multi-file test case's own headers.
            if let Some(rest) = rel.strip_prefix("crates/merlin/tests/multi/") {
                let dir = rest.split('/').next().unwrap();
                let inc = format!("crates/merlin/tests/multi/{dir}/include");
                if root.join(&inc).is_dir() {
                    includes.push(image_path(&inc));
                }
            }
            jobs.push(PpJob::new(image_path(rel), merlin::Lang::C, &includes, &[], true));
        } else if rel.ends_with(".S") {
            let mut includes = vec![include.clone()];
            if rel.starts_with("kernel/") {
                includes.push(kernel_include.clone());
            }
            let lang = merlin::Lang::AssemblerWithCpp;
            jobs.push(PpJob::new(image_path(rel), lang, &includes, &[], true));
            if rel.ends_with(".lds.S") {
                jobs.push(PpJob::new(image_path(rel), lang, &includes, &[], false));
            }
        }
    }
    let lang = merlin::Lang::AssemblerWithCpp;
    jobs.push(PpJob::new(
        image_path("include/asm.h"),
        lang,
        &[include],
        &[],
        true,
    ));
    // The rows, then the inputs written for this test, each in a directory
    // of its own: `/d/N/t.c` (`t.S`) and its headers.
    let rows: Vec<PpInput> = lexer_rows::cases()
        .into_iter()
        .chain(preprocessor_rows::cases())
        .map(|c| (c.src, c.files, c.args, false))
        .chain(
            PP_INPUTS
                .iter()
                .map(|(src, files, args, asm)| (src.to_string(), files.to_vec(), args.to_vec(), *asm)),
        )
        .collect();
    for (k, (src, headers, args, asm)) in rows.into_iter().enumerate() {
        let dir = format!("/d/{k}");
        for (name, text) in headers {
            files.insert(format!("{dir}/{name}"), text.as_bytes().to_vec());
        }
        let input = format!("{dir}/t.{}", if asm { "S" } else { "c" });
        files.insert(input.clone(), src.into_bytes());
        let args: Vec<String> = args.iter().map(|a| a.replace("{dir}", &dir)).collect();
        let (includes, macros): (Vec<&String>, Vec<&String>) = args.iter().partition(|a| a.starts_with("-I"));
        let includes: Vec<String> = includes.iter().map(|i| i[2..].to_string()).collect();
        let macros: Vec<&str> = macros.iter().map(|m| m.as_str()).collect();
        let lang = if asm {
            merlin::Lang::AssemblerWithCpp
        } else {
            merlin::Lang::C
        };
        jobs.push(PpJob::new(input, lang, &includes, &macros, true));
    }
    (jobs, files)
}

/// The directories `files` are in, parents first (as [`image_of`] makes
/// them), but `/out`, which it makes itself.
fn dirs_of(files: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    let mut dirs = BTreeSet::new();
    for path in files.keys() {
        let mut d = path.as_str();
        while let Some((parent, _)) = d.rsplit_once('/') {
            if parent.is_empty() {
                break;
            }
            dirs.insert(parent.to_string());
            d = parent;
        }
    }
    dirs.into_iter().collect()
}

/// Machines the preprocessor's corpus is split over: more than the other
/// tests', for its 500 runs of `cc1`.
const PP_MACHINES: usize = 8;

/// The machine of the preprocessor's tests: [`machine_config`]'s, with more
/// RAM, which a compiler's process takes more of than the assembler's.
fn cc1_machine_config() -> MachineConfig {
    MachineConfig {
        ram_size: 32 << 20,
        ..machine_config()
    }
}

/// Boots the disk kernel on `image`, as [`boot`] does, on
/// [`cc1_machine_config`]'s machine.
fn boot_cc1(image: Vec<u8>, input: &str, max_steps: u64) -> (String, Vec<u8>) {
    boot_on(cc1_machine_config(), image, input, max_steps)
}

/// Runs `jobs` (each with its number in the whole list, for its output file
/// `/out/N`) with `/bin/cc1` on a machine whose image holds `files`, and
/// compares each with `merlin -E`: the file it writes, if it succeeds, and
/// what it says. Returns what differs.
fn preprocess_on_camelot(jobs: &[(usize, &PpJob)], files: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    let commands: Vec<String> = jobs
        .iter()
        .map(|(k, job)| job.command(&format!("/out/{k}")))
        .collect();
    let input = format!("{}\nexit 0\n", commands.join("\n"));
    let listed: Vec<(String, Vec<u8>)> = files.iter().map(|(p, d)| (p.clone(), d.clone())).collect();
    let dirs = dirs_of(files);
    let dirs: Vec<&str> = dirs.iter().map(String::as_str).collect();
    // An inode for every file, directory and output, and some to spare.
    let ninodes = (listed.len() + dirs.len() + jobs.len() + 64) as u32;
    let image = image_with_inodes(&["cc1"], &dirs, &listed, ninodes);
    let (out, disk) = boot_cc1(image, &input, 200_000_000_000);
    let fs = Fs::open(&disk).unwrap();
    let mut wrong = Vec::new();
    for (i, (k, job)) in jobs.iter().enumerate() {
        let (text, said) = merlin_e(job, files);
        let want_said = if text.is_some() {
            said
        } else {
            format!("{said}cc1: exit 1\n")
        };
        let got_said = shell_output(&out, &commands[i]).replace('\r', "");
        let written = fs.read_file(&format!("/out/{k}")).ok();
        let want_written = text.map(String::into_bytes);
        if got_said != want_said {
            wrong.push(format!(
                "{}: said\n{got_said}\nmerlin says\n{want_said}",
                commands[i]
            ));
        }
        if written != want_written {
            let show = |w: &Option<Vec<u8>>| match w {
                Some(t) => String::from_utf8_lossy(t).into_owned(),
                None => "(no file)".to_string(),
            };
            wrong.push(format!(
                "{}: wrote\n{}\nmerlin writes\n{}",
                commands[i],
                show(&written),
                show(&want_written)
            ));
        }
    }
    wrong
}

/// #241's first stage: `/bin/cc1 -E` on Camelot writes what the host's
/// `merlin -E` writes, and says what it says, byte for byte, for every job
/// of [`pp_jobs`]: the repository's C and `.S` files, the diagnostics
/// table's rows of the lexer and the preprocessor, and [`PP_INPUTS`]. The
/// corpus is run once, split over [`PP_MACHINES`] machines.
#[test]
fn the_preprocessor_on_camelot_writes_what_merlin_writes() {
    let (jobs, files) = pp_jobs();
    assert!(jobs.len() > 400, "{}", jobs.len());
    // The corpus has both: what preprocesses (with warnings too) and what
    // merlin refuses.
    let results: Vec<(Option<String>, String)> = jobs.iter().map(|j| merlin_e(j, &files)).collect();
    let refused = results.iter().filter(|(t, _)| t.is_none()).count();
    let warned = results
        .iter()
        .filter(|(t, s)| t.is_some() && !s.is_empty())
        .count();
    assert!(refused > 40 && warned > 20, "{refused} refused, {warned} warned");
    // Each machine gets every PP_MACHINES-th job, so the big ones spread
    // out.
    let numbered: Vec<(usize, &PpJob)> = jobs.iter().enumerate().collect();
    let wrong: Vec<String> = std::thread::scope(|scope| {
        let runs: Vec<_> = (0..PP_MACHINES)
            .map(|m| {
                let share: Vec<(usize, &PpJob)> =
                    numbered.iter().copied().skip(m).step_by(PP_MACHINES).collect();
                let files = &files;
                scope.spawn(move || preprocess_on_camelot(&share, files))
            })
            .collect();
        runs.into_iter().flat_map(|r| r.join().unwrap()).collect()
    });
    assert!(wrong.is_empty(), "{} differ:\n{}", wrong.len(), wrong.join("\n"));
}

/// cc1's command line is merlin's (main.rs), with `cc1` for `merlin` in its
/// messages: the usage errors, a file it cannot read, output to standard
/// output for several inputs; and `-g`, its compilation directory given
/// both ways or, by default, the directory cc1 runs in (one whose name
/// fills a directory entry among them).
#[test]
fn cc1_takes_merlins_command_line() {
    let a = "#define A 1\nint a = A;\nint f(void) {\n    return a;\n}\n";
    let b = "  .word __LINE__\n";
    let files = [
        ("/d/a.c".to_string(), a.as_bytes().to_vec()),
        ("/d/b.S".to_string(), b.as_bytes().to_vec()),
    ];
    let usage = "usage: cc1 [options] input.c...\n";
    // (command, what it says first, and its exit status).
    let cases: &[(&str, String, u32)] = &[
        ("cc1 -q /d/a.c", format!("cc1: unknown option -q\n{usage}"), 2),
        (
            "cc1 -E -march=rv32ima_zicsr_zifencei /d/a.c",
            format!(
                "cc1: unsupported -march 'rv32ima_zicsr_zifencei' (merlin takes rv64ima_zicsr_zifencei)\n{usage}"
            ),
            2,
        ),
        ("cc1 -E -march rv64ima_zicsr_zifencei /d/a.c", format!("cc1: unknown option -march\n{usage}"), 2),
        ("cc1 -E -mabi=ilp32 /d/a.c", format!("cc1: unknown target ABI 'ilp32'\n{usage}"), 2),
        ("cc1 -E -x fortran /d/a.c", format!("cc1: language not recognized: 'fortran'\n{usage}"), 2),
        ("cc1 -E /d/a.c -D", format!("cc1: -D needs a macro name\n{usage}"), 2),
        ("cc1 -E -D=1 /d/a.c", format!("cc1: -D needs a macro name\n{usage}"), 2),
        ("cc1 -E /d/a.c -U", format!("cc1: -U needs a macro name\n{usage}"), 2),
        ("cc1 -E /d/a.c -I", format!("cc1: -I needs a directory\n{usage}"), 2),
        ("cc1 -E /d/a.c -o", format!("cc1: -o needs a file name\n{usage}"), 2),
        ("cc1 -E /d/a.c -x", format!("cc1: -x needs a language\n{usage}"), 2),
        (
            "cc1 -E -o /out/x /d/a.c /d/b.S",
            format!("cc1: -o needs a single input file\n{usage}"),
            2,
        ),
        (
            "cc1 -g -fdebug-compilation-dir=/d /d/b.S",
            format!(
                "cc1: /d/b.S: assembler-with-cpp input is only preprocessed (-E); merlin-as assembles the \
                 output\n{usage}"
            ),
            2,
        ),
        ("cc1 -E", usage.to_string(), 2),
        (
            "cc1 -E /d/missing.c",
            "cc1: cannot read /d/missing.c: no such file or directory\n".to_string(),
            2,
        ),
        // A directory is no file to read, as merlin finds.
        ("cc1 -E /d", "cc1: cannot read /d: is a directory\n".to_string(), 2),
        (
            "cc1 -g /d/a.c -fdebug-compilation-dir",
            format!("cc1: -fdebug-compilation-dir needs a directory\n{usage}"),
            2,
        ),
        (
            "cc1 -g -fdebug-compilation-dirs /d/a.c",
            format!("cc1: unknown option -fdebug-compilation-dirs\n{usage}"),
            2,
        ),
    ];
    // What merlin writes for both files: on standard output, one after the
    // other.
    let job_a = PpJob::new("/d/a.c".into(), merlin::Lang::C, &[], &[], true);
    let job_b = PpJob::new("/d/b.S".into(), merlin::Lang::AssemblerWithCpp, &[], &[], true);
    let map: BTreeMap<String, Vec<u8>> = files.iter().cloned().collect();
    let both = format!(
        "{}{}",
        merlin_e(&job_a, &map).0.unwrap(),
        merlin_e(&job_b, &map).0.unwrap()
    );
    let together = "cc1 -E -g /d/a.c /d/b.S";
    let help = "cc1 --help";
    let mut commands: Vec<&str> = cases.iter().map(|(c, ..)| *c).collect();
    commands.extend([together, help]);
    // -g: (the directory the shell is in, the command, and the directory
    // merlin is told the names are relative to). The default is the
    // directory cc1 runs in; a name of DIRSIZ bytes has no NUL in its
    // directory entry.
    let fill = "/d/abcdefghijklmn";
    assert_eq!(fill.len() - "/d/".len(), DIRSIZ);
    let debug: &[(&str, &str, &str)] = &[
        ("/d", "cc1 -g a.c -o /out/g1", "/d"),
        ("/d", "cc1 -g -fdebug-compilation-dir=here a.c -o /out/g2", "here"),
        (
            "/d",
            "cc1 -fdebug-compilation-dir there -g a.c -o /out/g3",
            "there",
        ),
        ("/d", "cc1 -g -fdebug-compilation-dir= a.c -o /out/g4", ""),
        (fill, "cc1 -g ../a.c -o /out/g5", fill),
        ("/", "cc1 -g d/a.c -o /out/g6", "/"),
        // An absolute name, split where it leaves the compilation directory,
        // as clang on Linux and on Camelot splits it (`clang_file`).
        ("/", "cc1 -g -fdebug-compilation-dir=/d /d/a.c -o /out/g7", "/d"),
    ];
    let mut input = commands.join("\n");
    for (dir, command, _) in debug {
        input.push_str(&format!("\ncd {dir}\n{command}"));
    }
    input.push_str("\nexit 0\n");
    let (out, disk) = boot_cc1(image_of(&["cc1"], &["/d", fill], &files), &input, 40_000_000_000);
    for (command, said, status) in cases {
        let got = shell_output(&out, command).replace('\r', "");
        assert!(got.starts_with(said.as_str()), "{command}: {got}");
        assert!(
            got.ends_with(&format!("cc1: exit {status}\n")),
            "{command}: {got}"
        );
    }
    assert_eq!(shell_output(&out, together).replace('\r', ""), both);
    assert!(shell_output(&out, help).replace('\r', "").starts_with(usage));
    let fs = Fs::open(&disk).unwrap();
    let image: BTreeMap<String, Vec<u8>> = files.iter().cloned().collect();
    for (k, (dir, command, comp_dir)) in debug.iter().enumerate() {
        assert_eq!(shell_output(&out, command).replace('\r', ""), "", "{command}");
        let written = String::from_utf8(fs.read_file(&format!("/out/g{}", k + 1)).unwrap()).unwrap();
        let name = command.split(' ').rev().nth(2).unwrap();
        let job = CcJob {
            dir: Some(dir.to_string()),
            ..CcJob::with_g(name.to_string(), &[], &[], Some(comp_dir))
        };
        let (want, said) = merlin_s(&job, &image);
        assert_eq!(said, "", "{command}");
        // An absolute name is the host's to merlin, which on Windows is
        // none starting with `/`: checked below.
        if !name.starts_with('/') || cfg!(unix) {
            assert_eq!(Some(&written), want.as_ref(), "{command}");
        }
        assert!(
            written.contains("\n    .loc ") && written.contains("DW_TAG_subprogram: f\n"),
            "{command}: {written}"
        );
    }
    // The absolute name, on any host: the main file's code is in `a.c`, of
    // the compilation directory, whose name is not the compile unit's
    // (`/d/a.c`), so it is a file of its own.
    let sum = merlin::md5::hex(&merlin::md5::md5(a.as_bytes()));
    let g7 = String::from_utf8(fs.read_file("/out/g7").unwrap()).unwrap();
    assert!(
        g7.starts_with(&format!(
            "    .file \"a.c\"\n    .file 0 \"/d\" \"/d/a.c\" md5 0x{sum}\n"
        )),
        "{g7}"
    );
    assert!(
        g7.contains(&format!("\n    .file 1 \"a.c\" md5 0x{sum}\n    .loc 1 3 0\n")),
        "{g7}"
    );
}

// ---- The C compiler, stage 2: the compiler, without -g (#241) ----

/// One `cc1 FILE.c -o OUT.s`: the input file (an image path, or one relative
/// to `dir`), the options before it, as cc1 is given them, and as merlin
/// is; and the directory the shell runs it in, if not the root.
struct CcJob {
    dir: Option<String>,
    input: String,
    args: Vec<String>,
    opts: merlin::Options,
}

impl CcJob {
    /// `input`, with these `-I` directories and `-D` and `-U` options.
    fn new(input: String, includes: &[String], macros: &[&str]) -> CcJob {
        let pp = PpJob::new(input, merlin::Lang::C, includes, macros, true);
        CcJob {
            dir: None,
            input: pp.input,
            args: pp.args,
            opts: pp.opts,
        }
    }

    /// The same with `-g`, and `-fdebug-compilation-dir=DIR` if `comp_dir`
    /// says so; merlin is told the directory (else, as cc1 finds it, the
    /// one it runs in: set `dir` too, or it is the root).
    fn with_g(input: String, includes: &[String], macros: &[&str], comp_dir: Option<&str>) -> CcJob {
        let mut job = CcJob::new(input, includes, macros);
        job.args.insert(0, "-g".to_string());
        if let Some(d) = comp_dir {
            job.args.insert(1, format!("-fdebug-compilation-dir={d}"));
        }
        job.opts.debug = Some(merlin::DebugInfo {
            comp_dir: comp_dir.unwrap_or("/").to_string(),
        });
        job
    }

    /// [`Self::with_g`] run in `dir`, the compilation directory by default.
    fn with_g_in(dir: &str, input: String, includes: &[String], macros: &[&str]) -> CcJob {
        let mut job = CcJob::with_g(input, includes, macros, None);
        job.opts.debug = Some(merlin::DebugInfo {
            comp_dir: dir.to_string(),
        });
        job.dir = Some(dir.to_string());
        job
    }

    fn command(&self, out: &str) -> String {
        format!("cc1 {} {} -o {out}", self.args.join(" "), self.input).replace("  ", " ")
    }
}

/// What the host's merlin makes of `job`, reading `files` (image path,
/// contents) for its input and headers: the assembly it writes, if it
/// compiles, and what it prints on stderr (the warnings of a compilation
/// that succeeds, every diagnostic of one that fails).
fn merlin_s(job: &CcJob, files: &BTreeMap<String, Vec<u8>>) -> (Option<String>, String) {
    // A name relative to the job's directory is that directory's file.
    let full = |p: &str| match &job.dir {
        Some(d) if !p.starts_with('/') => normalize(&format!("{d}/{p}")),
        _ => normalize(p),
    };
    let text = |p: &str| {
        files
            .get(&full(p))
            .and_then(|b| String::from_utf8(b.clone()).ok())
    };
    // merlin's paths are the host's (`\` on Windows); the image's are `/`.
    let load = |p: &std::path::Path| text(&p.to_string_lossy().replace('\\', "/"));
    let src = text(&job.input).unwrap_or_else(|| panic!("{}: no such file", job.input));
    let render = |errors: &[merlin::Error]| -> String {
        errors
            .iter()
            .map(|e| format!("{}\n", e.render(&job.input)))
            .collect()
    };
    match merlin::compile_warned(&job.input, &src, &job.opts, &load) {
        Ok((asm, warnings)) => (Some(asm), render(&warnings)),
        Err(e) => (None, render(&e)),
    }
}

/// An image path without its `.` components, each `..` taking away the one
/// before it (as the kernel resolves one).
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    format!("/{}", parts.join("/"))
}

/// The compiler's corpus: every C file of the repository but cc1's own,
/// with the `-I` directories the preprocessor's test gives it, and the
/// sources of every row of merlin's diagnostics table, each in a directory
/// of its own (`/d/N/t.c` and its headers); or, if `own`, cc1's own C files
/// alone ([`cc1_compiles_itself_on_camelot`]'s); and the files they read.
fn cc_jobs(own: bool) -> (Vec<CcJob>, BTreeMap<String, Vec<u8>>) {
    let root = repo_root();
    let sources = repo_sources();
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for rel in &sources {
        files.insert(image_path(rel), std::fs::read(root.join(rel)).unwrap());
    }
    let kernel_include = image_path("kernel/include");
    let mut jobs = Vec::new();
    for rel in sources.iter().filter(|r| r.ends_with(".c")) {
        if rel.starts_with("kernel/user/cc/") != own {
            continue;
        }
        let mut includes = Vec::new();
        if rel.starts_with("kernel/") && !rel.starts_with("kernel/user/") {
            includes.push(kernel_include.clone());
        }
        if let Some(rest) = rel.strip_prefix("crates/merlin/tests/multi/") {
            let dir = rest.split('/').next().unwrap();
            let inc = format!("crates/merlin/tests/multi/{dir}/include");
            if root.join(&inc).is_dir() {
                includes.push(image_path(&inc));
            }
        }
        jobs.push(CcJob::new(image_path(rel), &includes, &[]));
    }
    if own {
        return (jobs, files);
    }
    let rows: Vec<Case> = lexer_rows::cases()
        .into_iter()
        .chain(preprocessor_rows::cases())
        .chain(parser_rows::cases())
        .chain(sema_rows::cases())
        .chain(warnings_rows::cases())
        .chain(codegen_rows::cases())
        .collect();
    for (k, c) in rows.into_iter().enumerate() {
        let dir = format!("/d/{k}");
        for (name, text) in &c.files {
            files.insert(format!("{dir}/{name}"), text.as_bytes().to_vec());
        }
        let input = format!("{dir}/t.c");
        files.insert(input.clone(), c.src.into_bytes());
        let args: Vec<String> = c.args.iter().map(|a| a.replace("{dir}", &dir)).collect();
        let (includes, macros): (Vec<&String>, Vec<&String>) = args.iter().partition(|a| a.starts_with("-I"));
        let includes: Vec<String> = includes.iter().map(|i| i[2..].to_string()).collect();
        let macros: Vec<&str> = macros.iter().map(|m| m.as_str()).collect();
        jobs.push(CcJob::new(input, &includes, &macros));
    }
    (jobs, files)
}

/// The machine of the compiler's tests: [`cc1_machine_config`]'s, with the
/// RAM compiling the biggest files takes (cc1's arena and heap reach some
/// 30 MiB for its own sema.c).
fn cc1_compile_config() -> MachineConfig {
    MachineConfig {
        ram_size: 128 << 20,
        ..cc1_machine_config()
    }
}

/// Runs `jobs` (each with its number in the whole list, for its output file
/// `/out/N`) with `/bin/cc1` on a machine whose image holds `files`, and
/// compares each with the host's merlin ([`merlin_s`]): the assembly it
/// writes, if it compiles, and what it says. Returns what differs.
fn compile_on_camelot(jobs: &[(usize, &CcJob)], files: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    let commands: Vec<String> = jobs
        .iter()
        .map(|(k, job)| job.command(&format!("/out/{k}")))
        .collect();
    // Each command, after a `cd` to its job's directory when the shell is in
    // another.
    let mut input = String::new();
    let mut here = "/";
    for ((_, job), command) in jobs.iter().zip(&commands) {
        let dir = job.dir.as_deref().unwrap_or("/");
        if dir != here {
            input.push_str(&format!("cd {dir}\n"));
            here = dir;
        }
        input.push_str(command);
        input.push('\n');
    }
    input.push_str("exit 0\n");
    let listed: Vec<(String, Vec<u8>)> = files.iter().map(|(p, d)| (p.clone(), d.clone())).collect();
    let dirs = dirs_of(files);
    let dirs: Vec<&str> = dirs.iter().map(String::as_str).collect();
    // What merlin makes of each, and room on the image for as much.
    let want: Vec<(Option<String>, String)> = jobs.iter().map(|(_, j)| merlin_s(j, files)).collect();
    let written: usize = want.iter().map(|(t, _)| t.as_ref().map_or(0, String::len)).sum();
    let ninodes = (listed.len() + dirs.len() + jobs.len() + 64) as u32;
    let image = image_with_room(&["cc1"], &dirs, &listed, ninodes, 2 * written);
    let (out, data) = boot_on(cc1_compile_config(), image, &input, 20_000_000_000_000);
    let fs = Fs::open(&data).unwrap();
    let mut wrong = Vec::new();
    for (i, ((k, _), (text, said))) in jobs.iter().zip(want).enumerate() {
        let want_said = if text.is_some() {
            said
        } else {
            format!("{said}cc1: exit 1\n")
        };
        let got_said = shell_output(&out, &commands[i]).replace('\r', "");
        let written = fs.read_file(&format!("/out/{k}")).ok();
        let want_written = text.map(String::into_bytes);
        if got_said != want_said {
            wrong.push(format!(
                "{}: said\n{got_said}\nmerlin says\n{want_said}",
                commands[i]
            ));
        }
        if written != want_written {
            let show = |w: &Option<Vec<u8>>| match w {
                Some(t) => format!("{} bytes", t.len()),
                None => "(no file)".to_string(),
            };
            wrong.push(format!(
                "{}: wrote {}, merlin writes {}",
                commands[i],
                show(&written),
                show(&want_written)
            ));
        }
    }
    wrong
}

/// Machines the compiler's corpus is split over.
const CC_MACHINES: usize = 12;

/// Runs `jobs` split over `machines` machines, each getting every
/// `machines`-th job so that the big ones spread out: what differs.
fn compile_split(jobs: &[CcJob], files: &BTreeMap<String, Vec<u8>>, machines: usize) -> Vec<String> {
    let numbered: Vec<(usize, &CcJob)> = jobs.iter().enumerate().collect();
    std::thread::scope(|scope| {
        let runs: Vec<_> = (0..machines)
            .map(|m| {
                let share: Vec<(usize, &CcJob)> =
                    numbered.iter().copied().skip(m).step_by(machines).collect();
                scope.spawn(move || compile_on_camelot(&share, files))
            })
            .collect();
        runs.into_iter().flat_map(|r| r.join().unwrap()).collect()
    })
}

/// #241's second stage: `/bin/cc1` on Camelot compiles as the host's merlin
/// does, byte for byte, without `-g`: for every C file of the repository
/// (merlin's test cases, the kernel's, the user programs', as, ld and ar's)
/// and the source of every row of merlin's diagnostics table, the assembly
/// merlin writes, or the diagnostics it gives and its exit status, its
/// warnings too. The corpus is run once, split over [`CC_MACHINES`]
/// machines.
#[test]
fn the_compiler_on_camelot_writes_what_merlin_writes() {
    let (jobs, files) = cc_jobs(false);
    assert!(jobs.len() > 1200, "{}", jobs.len());
    // The corpus has all three: what compiles, what compiles with
    // warnings, and what merlin refuses.
    let results: Vec<(Option<String>, String)> = jobs.iter().map(|j| merlin_s(j, &files)).collect();
    let compiled = results
        .iter()
        .filter(|(t, s)| t.is_some() && s.is_empty())
        .count();
    let warned = results
        .iter()
        .filter(|(t, s)| t.is_some() && !s.is_empty())
        .count();
    let refused = results.iter().filter(|(t, _)| t.is_none()).count();
    assert!(
        compiled > 200 && warned > 150 && refused > 500,
        "{compiled} compiled, {warned} warned, {refused} refused"
    );
    let wrong = compile_split(&jobs, &files, CC_MACHINES);
    assert!(wrong.is_empty(), "{} differ:\n{}", wrong.len(), wrong.join("\n"));
}

// ---- The C compiler, stage 3: -g (#241) ----

/// A header and a main file that includes it and one in `inc`, for
/// [`G_INPUTS`].
const G_H: &str = "static int twice(int x) {\n    return 2 * x;\n}\n";
const G_O: &str = "static int thrice(int x) {\n    return 3 * x;\n}\n";
const G_MAIN: &str =
    "#include \"h.h\"\n#include <o.h>\nint main(void) {\n    return twice(2) + thrice(1);\n}\n";
const G_Q: &str = "static int hq(int a) { return a + 1; }\n";
const G_UP: &str = "static int up(void) { return 1; }\n";

/// Files named every way `-g` names them (debug.c's "File names", merlin's
/// `files_are_named_as_clang_names_them`): each case's files (by name from
/// its directory, `/g/N/w`), its options (`-I` directories, and the
/// compilation directory, else the directory cc1 runs in) and its input.
/// The main file named `./m.c` or through `..`, headers next to it and in
/// `-I` directories (one named as the compilation directory), a header
/// above it, `#line` names (none, the main file's, a header's, one that
/// is a directory, with control characters and with `\`), a name not ASCII,
/// a compilation directory of `.`, `./`, none at all and one with `\`.
#[allow(clippy::type_complexity)]
const G_INPUTS: &[(&[(&str, &str)], &[&str], &str)] = &[
    (&[("m.c", G_MAIN), ("h.h", G_H), ("inc/o.h", G_O)], &["-Iinc", "-fdebug-compilation-dir=src"], "m.c"),
    (&[("m.c", G_MAIN), ("h.h", G_H), ("inc/o.h", G_O)], &["-Iinc", "-fdebug-compilation-dir=src"], "./m.c"),
    (&[("m.c", G_MAIN), ("h.h", G_H), ("inc/o.h", G_O)], &["-Iinc"], "./m.c"),
    (&[("m.c", G_MAIN), ("h.h", G_H), ("inc/o.h", G_O)], &["-Iinc", "-fdebug-compilation-dir=."], "./m.c"),
    (&[("m.c", G_MAIN), ("h.h", G_H), ("inc/o.h", G_O)], &["-I./inc", "-fdebug-compilation-dir=./"], "m.c"),
    (&[("m.c", G_MAIN), ("h.h", G_H), ("inc/o.h", G_O)], &["-Iinc/", "-fdebug-compilation-dir=a\\b"], "m.c"),
    (&[("m.c", G_MAIN), ("h.h", G_H), ("inc/o.h", G_O)], &["-Iinc", "-fdebug-compilation-dir="], "m.c"),
    (&[("sub/m.c", G_MAIN), ("sub/h.h", G_H), ("inc/o.h", G_O)], &["-Iinc", "-fdebug-compilation-dir=sub"], "sub/m.c"),
    (&[("sub/m.c", G_MAIN), ("sub/h.h", G_H), ("inc/o.h", G_O)], &["-Isub/../inc"], "sub/../sub/m.c"),
    (
        &[("m.c", "#include <h.h>\nint main(void) {\n    return twice(2);\n}\n"), ("src/h.h", G_H)],
        &["-Isrc", "-fdebug-compilation-dir=src"],
        "m.c",
    ),
    (&[("m.c", "int f(void) {\n#line 7 \"gen/y.c\"\n    return 1;\n}\n")], &["-fdebug-compilation-dir=src"], "m.c"),
    (&[("m.c", "int f(void) {\n#line 7 \"\"\n    return 1;\n}\nint g;\n")], &[], "m.c"),
    (&[("m.c", "int f(void) {\n#line 7 \"m.c\"\n    return 1;\n}\n")], &[], "m.c"),
    (&[("m.c", "int f(void) {\n#line 7 \"./m.c\"\n    return 1;\n}\n")], &["-fdebug-compilation-dir=src"], "m.c"),
    (&[("m.c", "int f(void) {\n#line 7 \"src/m.c\"\n    return 1;\n}\n")], &["-fdebug-compilation-dir=src"], "m.c"),
    (
        &[("m.c", "int f(void) {\n#line 7 \"a/\"\n    return 1;\n#line 9 \"/\"\n    return 2;\n}\n")],
        &["-fdebug-compilation-dir=a"],
        "m.c",
    ),
    (
        &[
            ("k1.c", "#include \"inc/q.h\"\nint f(int a) { return hq(a); }\n#line 1 \"./inc/q.h\"\nint g(int a) { return a; }\n"),
            ("inc/q.h", G_Q),
        ],
        &["-fdebug-compilation-dir=src"],
        "k1.c",
    ),
    (
        &[
            ("k1.c", "#line 1 \"./inc/q.h\"\nint g(int a) { return a; }\n#include \"inc/q.h\"\nint f(int a) { return hq(a); }\n"),
            ("inc/q.h", G_Q),
        ],
        &["-fdebug-compilation-dir=src"],
        "k1.c",
    ),
    (&[("caf\u{e9}.c", "int g;\nint f(void) { return g; }\n")], &[], "caf\u{e9}.c"),
    (
        &[("m.c", "int f(void) {\n#line 7 \"a\\tb\\bc\\fd\\re\\\"f\\vg\\ah\\ni~\u{e9}.c\"\n    return 1;\n}\n")],
        &["-fdebug-compilation-dir=src"],
        "m.c",
    ),
    (&[("m.c", "int f(void) {\n#line 7 \"a\\\\b\\\\c.c\"\n    return 1;\n}\n")], &[], "m.c"),
    (&[("m.c", "#include \"../up.h\"\nint main(void) { return up(); }\n"), ("../up.h", G_UP)], &[], "m.c"),
    (&[("d/m.c", "#include \"../up.h\"\nint main(void) { return up(); }\n"), ("up.h", G_UP)], &["-fdebug-compilation-dir=d"], "d/m.c"),
    (&[("m.c", "static int s;\nint *p = &s;\n")], &[], "m.c"),
];

/// Absolute names, which merlin takes as the host takes them: [`G_INPUTS`]
/// for a POSIX host only (on Windows, `/abs/m.c` is relative). An absolute
/// name split where it leaves the compilation directory, one that shares
/// only the root with it, and a network name.
#[allow(clippy::type_complexity)]
const G_ABSOLUTE: &[(&[(&str, &str)], &[&str], &str)] = &[
    (
        &[("m.c", "int f(void) {\n#line 7 \"/abs/m.c\"\n    return 1;\n}\n")],
        &["-fdebug-compilation-dir=/abs"],
        "m.c",
    ),
    (
        &[("m.c", "int f(void) {\n#line 7 \"/abs/x/m.c\"\n    return 1;\n}\n")],
        &["-fdebug-compilation-dir=/other"],
        "m.c",
    ),
    (
        &[(
            "m.c",
            "int f(void) {\n#line 7 \"//net/x/m.c\"\n    return 1;\n}\n",
        )],
        &["-fdebug-compilation-dir=//net/x"],
        "m.c",
    ),
    (
        &[("m.c", G_MAIN), ("h.h", G_H), ("inc/o.h", G_O)],
        &["-Iinc"],
        "/g/abs/w/m.c",
    ),
];

/// `-g`'s corpus: every input of the compiler's ([`cc_jobs`]) that merlin
/// compiles, with `-g`, named from the directory cc1 runs in: the
/// repository's from `/r` (the compilation directory cc1 finds itself), the
/// diagnostics table's rows from `/d` (given it); and [`G_INPUTS`], each in
/// a directory of its own (and [`G_ABSOLUTE`] on a POSIX host); the files
/// they read.
fn g_jobs() -> (Vec<CcJob>, BTreeMap<String, Vec<u8>>) {
    let (plain, mut files) = cc_jobs(false);
    let mut jobs = Vec::new();
    for job in &plain {
        let dir = if job.input.starts_with("/r/") { REPO } else { "/d" };
        let rel = |p: &str| p.strip_prefix(&format!("{dir}/")).unwrap().to_string();
        let input = rel(&job.input);
        let includes: Vec<String> = job
            .opts
            .include_dirs
            .iter()
            .map(|d| rel(&d.to_string_lossy()))
            .collect();
        let macros: Vec<&str> = job
            .args
            .iter()
            .filter(|a| a.starts_with("-D") || a.starts_with("-U"))
            .map(String::as_str)
            .collect();
        let g = if dir == REPO {
            CcJob::with_g_in(dir, input, &includes, &macros)
        } else {
            CcJob {
                dir: Some(dir.to_string()),
                ..CcJob::with_g(input, &includes, &macros, Some(dir))
            }
        };
        if merlin_s(&g, &files).0.is_some() {
            jobs.push(g);
        }
    }
    let absolute = if cfg!(unix) { G_ABSOLUTE } else { &[] };
    for (k, (named, args, input)) in G_INPUTS.iter().chain(absolute).enumerate() {
        // The absolute main file's directory is its own.
        let dir = if input.starts_with('/') {
            "/g/abs/w".to_string()
        } else {
            format!("/g/{k}/w")
        };
        for (name, text) in named.iter() {
            files.insert(normalize(&format!("{dir}/{name}")), text.as_bytes().to_vec());
        }
        let includes: Vec<String> = args
            .iter()
            .filter_map(|a| a.strip_prefix("-I"))
            .map(str::to_string)
            .collect();
        let comp_dir = args
            .iter()
            .find_map(|a| a.strip_prefix("-fdebug-compilation-dir="));
        let job = match comp_dir {
            Some(c) => CcJob {
                dir: Some(dir.clone()),
                ..CcJob::with_g(input.to_string(), &includes, &[], Some(c))
            },
            None => CcJob::with_g_in(&dir, input.to_string(), &includes, &[]),
        };
        jobs.push(job);
    }
    (jobs, files)
}

/// #241's third stage: `/bin/cc1 -g` on Camelot writes what the host's
/// `merlin -g` writes, byte for byte, line tables and DWARF compile unit
/// included, for every job of [`g_jobs`]: every C file of the repository but
/// cc1's own and every row of merlin's diagnostics table that merlin
/// compiles, and the file names of [`G_INPUTS`]. Split over
/// [`CC_MACHINES`] machines.
#[test]
fn the_compiler_on_camelot_writes_what_merlin_writes_with_g() {
    let (jobs, files) = g_jobs();
    assert!(jobs.len() > 450, "{}", jobs.len());
    // Every one compiles, and between them they describe all merlin does:
    // every kind of entry, blocks, labels and the line table's files with
    // their checksums, in directories and not.
    let asm: Vec<String> = jobs
        .iter()
        .map(|j| {
            let (text, said) = merlin_s(j, &files);
            text.unwrap_or_else(|| panic!("{}: {said}", j.input))
        })
        .collect();
    for tag in [
        "array_type",
        "enumeration_type",
        "formal_parameter",
        "label",
        "lexical_block",
        "member",
        "pointer_type",
        "compile_unit",
        "structure_type",
        "subroutine_type",
        "typedef",
        "union_type",
        "unspecified_parameters",
        "subrange_type",
        "base_type",
        "const_type",
        "enumerator",
        "subprogram",
        "variable",
        "volatile_type",
    ] {
        let n = asm
            .iter()
            .filter(|a| a.contains(&format!("DW_TAG_{tag}: ")))
            .count();
        assert!(n > 0, "no DW_TAG_{tag}");
    }
    let described = asm.iter().filter(|a| a.contains("DW_TAG_subprogram: ")).count();
    let headers = asm
        .iter()
        .filter(|a| {
            a.lines()
                .any(|l| l.starts_with("    .file 2 \"") && l.contains(" md5 0x"))
        })
        .count();
    let prologues = asm.iter().filter(|a| a.contains(" prologue_end\n")).count();
    assert!(
        described > 400 && headers > 60 && prologues > 400,
        "{described} with functions, {headers} with headers, {prologues} with prologues"
    );
    let wrong = compile_split(&jobs, &files, CC_MACHINES);
    assert!(wrong.is_empty(), "{} differ:\n{}", wrong.len(), wrong.join("\n"));
}

/// `/bin/cc1` compiles itself on Camelot: the assembly of each of its C
/// files (kernel/user/cc) is what the host's merlin writes of it, as the
/// `.s` merlin builds `/bin/cc1` from; and with `-g`, as `cargo xtask
/// kernel` builds it, run in cc1's directory, which it finds is its
/// compilation directory. A machine for each file, which compiles it both
/// ways.
#[test]
fn cc1_compiles_itself_on_camelot() {
    let (mut jobs, files) = cc_jobs(true);
    let n = jobs.len();
    assert!(n >= 18, "{n}");
    let dir = image_path("kernel/user/cc");
    for k in 0..n {
        let name = jobs[k]
            .input
            .strip_prefix(&format!("{dir}/"))
            .unwrap()
            .to_string();
        jobs.push(CcJob::with_g_in(&dir, name, &[], &[]));
    }
    for job in &jobs {
        let (text, said) = merlin_s(job, &files);
        assert!(text.is_some(), "{}: merlin compiles cc1: {said}", job.input);
    }
    // The second half's are -g's (job k + n on machine k, with job k): each
    // file with code has its functions described (tables.c has none).
    for k in 0..n {
        let plain = merlin_s(&jobs[k], &files).0.unwrap();
        let g = merlin_s(&jobs[k + n], &files).0.unwrap();
        let code = plain.contains("@function");
        assert_eq!(g.contains("DW_TAG_subprogram: "), code, "{}", jobs[k].input);
    }
    let wrong = compile_split(&jobs, &files, n);
    assert!(wrong.is_empty(), "{} differ:\n{}", wrong.len(), wrong.join("\n"));
}
