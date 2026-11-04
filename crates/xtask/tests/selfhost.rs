//! Self-hosting, step 1 (#135): the assembler that runs on Camelot
//! (`kernel/user/as`, `/bin/as`) writes, for every file of a corpus, the same
//! object the host's merlin-as writes, byte for byte. The test boots the disk
//! kernel on an image with `/bin/sh`, `/bin/as` and the corpus in `/src`, has
//! the shell run `as` on each file into `/out`, then reads the objects back
//! on the host and compares them. The corpus: merlin-as's golden encodings,
//! every hand-written `.s` of the repository, the line table corpus
//! (`crates/merlin-as/tests/debug_line`, whose objects are clang's), and
//! merlin's assembly for a C file, with `-g` (`.file`, `.loc` and a DWARF
//! compile unit) and without, data of every size (`DATA`) and sections of
//! every kind (`SECTIONS`). An ignored test adds merlin's `-g` assembly for
//! two big C files, the shell and the kernel's vm.c.

use camelot_fs::mkfs::{Mkfs, Options};
use camelot_fs::read::Fs;
use grail::block::MemDisk;
use grail::{Machine, MachineConfig, RunResult};
use std::path::PathBuf;
use xtask::{kernel, repo_root, toolchain, userprogs};

const PROMPT: &str = "$ ";

/// The corpus: (name on the image, text); `big` adds merlin's assembly
/// for the shell and the kernel's vm.c.
fn corpus(big: bool) -> Vec<(String, String)> {
    let root = repo_root();
    let mut files: Vec<PathBuf> = vec![root.join("crates/merlin-as/tests/golden/encode.s")];
    for dir in [
        "kernel",
        "kernel/user",
        "kernel/boot",
        "crates/grail/rom",
        "crates/merlin-as/tests/debug_line",
    ] {
        let mut more: Vec<PathBuf> = std::fs::read_dir(root.join(dir))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "s"))
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
                if p.extension().is_some_and(|x| x == "s" || x == "S") {
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
            let name = p
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace(['/', '\\'], "_");
            (name, std::fs::read_to_string(p).unwrap())
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
    out
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
/// information: what no file of the corpus has all of.
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

/// Boots the disk kernel on `image`, types `input` (ending with `exit`).
/// Returns what it printed and the disk afterwards. The machine has the
/// default 4 MiB of RAM, which the assembler is written to fit in.
fn boot(image: Vec<u8>, input: &str, max_steps: u64) -> (String, Vec<u8>) {
    let config = MachineConfig::default();
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

/// The same with merlin's assembly for the shell and vm.c too: a few minutes.
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
        let want = merlin_as::assemble_to_elf(text).unwrap();
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

#[test]
fn the_assembler_on_camelot_reports_merlin_as_errors() {
    // Bad inputs: the messages, file and line merlin-as gives.
    let cases = [
        "    nop\n    frobnicate a0\n",
        "    addi a0, a1, 5000\n    beq a0, a1, 3\n",
        "    .word \"x\"\n    lw a0, %hi(sym)(a1)\n",
        "1:  nop\n    j 2b\n",
        "    li a0, 0x1000000000\n",
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
    ];
    let inputs: Vec<(String, String)> = cases
        .iter()
        .enumerate()
        .map(|(i, c)| (format!("case{i}"), c.to_string()))
        .collect();
    let commands: Vec<String> = (0..inputs.len())
        .map(|i| format!("as /src/{0}.s -o /out/{0}.o", short(i)))
        .collect();
    let input = format!("{}\nexit 0\n", commands.join("\n"));
    let (out, _) = boot(image(&inputs), &input, 2_000_000_000);
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
/// `with_debug` names, and compares the executables with merlin-ld's. The
/// objects go in `/o`, each program's command line in a response file in
/// `/a` (a link line is longer than a console line), the results in `/out`.
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
/// it runs (on a machine of its own: it is a bare-metal program).
#[test]
fn hello_assembled_and_linked_on_camelot_is_the_hosts_and_runs() {
    let root = repo_root();
    let ex = xtask::examples::find("hello").unwrap();
    let host = xtask::examples::build(&root, ex).unwrap();
    assert_eq!(host.elf, std::fs::read(ex.elf_path(&root)).unwrap());
    let files: Vec<(String, Vec<u8>)> = ["hello.S", "link.ld"]
        .iter()
        .map(|f| (format!("/src/{f}"), std::fs::read(ex.dir(&root).join(f)).unwrap()))
        .collect();
    let commands = [
        "as /src/hello.S -o /out/hello.o",
        "ld -T /src/link.ld /out/hello.o -o /out/hello",
    ];
    let input = format!("{}\nexit 0\n", commands.join("\n"));
    let (out, disk) = boot(image_of(&["as", "ld"], &["/src"], &files), &input, 2_000_000_000);
    for c in commands {
        assert_eq!(shell_output(&out, c), "", "{c}");
    }
    let elf = Fs::open(&disk).unwrap().read_file("/out/hello").unwrap();
    assert!(elf == host.elf, "/out/hello is not examples/hello/hello.elf");
    let mut m = Machine::new(MachineConfig::default()).unwrap();
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

/// `obj` with its section `name` made a mergeable string section
/// (`SHF_MERGE | SHF_STRINGS`, entries of one byte), as clang's
/// `.rodata.str1.1` is: merlin-as writes none.
fn mergeable(mut obj: Vec<u8>, name: &str) -> Vec<u8> {
    let at = {
        let elf = elf::Elf::parse(&obj).unwrap();
        let i = elf.sections.iter().position(|s| s.name == name).unwrap();
        elf.header.shoff as usize + 40 * i
    };
    let flags = u32::from_le_bytes(obj[at + 8..at + 12].try_into().unwrap());
    obj[at + 8..at + 12].copy_from_slice(&(flags | (elf::SHF_MERGE | elf::SHF_STRINGS) as u32).to_le_bytes());
    obj[at + 36..at + 40].copy_from_slice(&1u32.to_le_bytes());
    obj
}

/// `obj` with the 32-bit field at `field` of section `name`'s ELF32 header
/// set to `f` of its value.
fn shdr_field(mut obj: Vec<u8>, name: &str, field: usize, f: impl Fn(u32) -> u32) -> Vec<u8> {
    let at = {
        let elf = elf::Elf::parse(&obj).unwrap();
        let i = elf.sections.iter().position(|s| s.name == name).unwrap();
        elf.header.shoff as usize + 40 * i + field
    };
    let v = u32::from_le_bytes(obj[at..at + 4].try_into().unwrap());
    obj[at..at + 4].copy_from_slice(&f(v).to_le_bytes());
    obj
}

/// A program for [`the_linker_on_camelot_links_scripts_as_merlin_ld_does`]:
/// code, read-only data, data with `.dword` (R_RISCV_64) and `.word`
/// pointers, a section the scripts throw away, and debug information
/// pointing at all of them (R_RISCV_64 and R_RISCV_32 in a section that is
/// not loaded, at discarded code with a positive addend too).
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
";

/// The other object of [`SCRIPTED`]'s links.
const FAR: &str = "
        .text
        .globl far
far:    ret
";

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
/// section whose segment is dropped.
#[test]
fn the_linker_on_camelot_links_scripts_as_merlin_ld_does() {
    let objects = [object(SCRIPTED), object(FAR)];
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
        "SECTIONS { . = 0x80000400; .text : { *(.text) } .rodata : { *(.rodata) } .two : { *(.rodata.two) }          .data.rel.ro : { *(.data) } .bss : { *(.bss) } /DISCARD/ : { *(.text.gone) } }"
            .into(),
        // An empty section alone in its run, which is dropped, and a gap
        // before .bss.
        "SECTIONS { . = 0x80000000; .text : { *(.text) } .x : { sym = .; } .data : { *(.data) }          . = . + 0x100; .bss : { *(.bss) } .rodata : { *(.rodata) *(.rodata.two) }          /DISCARD/ : { *(.text.gone) } }"
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
        "MEMORY { RAM (rwx) : ORIGIN = 0x80000000, LENGTH = 1M } SECTIONS { .text : { *(.text) } > RAM          .data : { *(.data) *(.rodata) *(.rodata.two) *(.bss) } > RAM /DISCARD/ : { *(.text.gone) } }"
            .into(),
    ] {
        links.push((vec![objects[0].clone(), objects[1].clone(), orphans.clone()], script));
    }
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
    // error, merlin-ld's after a warning). Each case is its inputs, a
    // script, and more arguments: `--start-lib` makes the last input a
    // library member.
    let script = "ENTRY(_start)\nSECTIONS\n{\n    . = 0x1000;\n    .text : { *(.text) }\n    .rodata : { *(.rodata) }\n    .data : { *(.data) }\n}\n";
    let start = object(".text\n.globl _start\n_start: nop\n");
    let truncated = start[..start.len() - 50].to_vec();
    let strings = ".rodata\n.asciz \"hello\"\n.asciz \"world\"\n";
    let with_rodata = object(".text\n.globl _start\n_start: nop\n.rodata\n.word 1\n");
    let cases: Vec<(Vec<Vec<u8>>, &str, &str)> = vec![
        // Errors.
        (
            vec![object(
                ".text\n.globl _start\n_start: call missing\n    call gone\n",
            )],
            script,
            "",
        ),
        (vec![start.clone(), start.clone()], script, ""),
        (vec![start.clone()], "SECTIONS { .text 0x100 : { *(.text) } }", ""),
        (
            vec![object(
                ".text\n.globl _start\n_start: nop\n.section .extra, \"aw\"\n.word 1\n",
            )],
            script,
            "",
        ),
        (
            vec![
                object(".text\n.globl _start\n_start: beq a0, a1, far\n"),
                object(".text\n.zero 8192\n.globl far\nfar: nop\n"),
            ],
            script,
            "",
        ),
        (vec![start.clone()], script, "-e nosuch"),
        (vec![start.clone()], script, "-e 0x+10"),
        (
            vec![start.clone()],
            "SECTIONS { . = 0x1000; . = 0x10; .text : { *(.text) } x = nothere; }",
            "",
        ),
        (
            vec![object(".text\n.globl _start\n_start: nop\n.zero 64\n")],
            "MEMORY { RAM : ORIGIN = 0x1000, LENGTH = 32 }\nSECTIONS { .text : { *(.text) } > RAM }",
            "",
        ),
        (
            vec![start.clone()],
            "SECTIONS { . = 0xfffffff0; .text : { *(.text) . += 0x20; } }",
            "",
        ),
        (
            vec![start.clone()],
            "MEMORY { a : o = 0, l = 1 b : o = 1, l = 1 }",
            "",
        ),
        (
            vec![object(
                ".text\n.globl _start\n_start: la a0, x\n.data\n.globl x\nx: .word _start\n",
            )],
            "SECTIONS { /DISCARD/ : { *(.data) } .text : { *(.text) } }",
            "",
        ),
        (vec![b"not an object".to_vec()], script, ""),
        (vec![start.clone(), truncated], script, ""),
        // An alignment that is not a power of two (sh_addralign at 32), a
        // symbol table whose size is not a multiple of 16 (sh_size at 20).
        (vec![shdr_field(start.clone(), ".text", 32, |_| 12)], script, ""),
        (vec![shdr_field(start.clone(), ".symtab", 20, |size| size + 4)], script, ""),
        // Load addresses: two sections loaded over each other, a load range
        // past 4 GiB, AT after ALIGN, /DISCARD/ with an AT.
        (
            vec![with_rodata.clone()],
            "SECTIONS { . = 0x1000; .text : { *(.text) } .rodata : AT(0x1000) { *(.rodata) } }",
            "",
        ),
        (
            vec![with_rodata.clone()],
            "SECTIONS { . = 0x1000; .text : AT(0xfffffffe) { *(.text) } .rodata : AT(0 - 4) { *(.rodata) } }",
            "",
        ),
        (
            vec![start.clone()],
            "SECTIONS { .text : ALIGN(4) AT(0x100) { *(.text) } }",
            "",
        ),
        (
            vec![start.clone()],
            "SECTIONS { /DISCARD/ : AT(0x100) { *(.comment) } }",
            "",
        ),
        // The headers in the read-only first section's segment, whose load
        // address leaves no room below it for them.
        (
            vec![with_rodata.clone()],
            "SECTIONS { . = 0x80000400; .rodata : AT(0x10) { *(.rodata) } .text : { *(.text) } }",
            "",
        ),
        // ADDR: of a section not placed yet, in its own ALIGN, of none.
        (
            vec![start.clone()],
            "SECTIONS { . = 0x1000; x = ADDR(.text); .text : ALIGN(ADDR(.text)) { *(.text) } y = ADDR(.nope); }",
            "",
        ),
        // OUTPUT_FORMAT: an unknown name, and elf64-littleriscv with RV32
        // objects, every one of them reported, library members too.
        (
            vec![start.clone()],
            "OUTPUT_FORMAT(\"elf32-littleriscv\", \"elf64-bigriscv\")",
            "",
        ),
        (
            vec![start.clone(), object(".text\n.globl f\nf: nop\n")],
            "OUTPUT_FORMAT(\"elf64-littleriscv\")\nSECTIONS { . = 0x1000; .text : { *(.text) } }",
            "--start-lib",
        ),
        // Warnings: no entry symbol (the first code section's address is
        // used), and duplicate strings in mergeable sections.
        (
            vec![object(".text\nmain: nop\n")],
            "SECTIONS { . = 0x1000; .text : { *(.text) } }",
            "",
        ),
        // The same, loaded elsewhere, and with the headers loaded below it.
        (
            vec![object(".text\nmain: nop\n")],
            "SECTIONS { . = 0x1400; .text : AT(0x800) { *(.text) } }",
            "",
        ),
        (
            vec![
                mergeable(
                    object(&format!("{}{strings}", ".text\n.globl _start\n_start: nop\n")),
                    ".rodata",
                ),
                mergeable(object(strings), ".rodata"),
            ],
            script,
            "",
        ),
    ];
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut commands = Vec::new();
    // What merlin-ld says and writes for each case.
    let mut wants: Vec<(String, Option<Vec<u8>>)> = Vec::new();
    for (k, (objects, script, extra)) in cases.iter().enumerate() {
        let mut inputs = Vec::new();
        let mut library = Vec::new();
        let mut args = format!("ld -T /e/{k}.ld -o /out/{k}");
        for (j, data) in objects.iter().enumerate() {
            let path = format!("/e/{k}_{j}.o");
            let input = merlin_ld::Input {
                name: path.clone(),
                data: data.clone(),
            };
            if *extra == "--start-lib" && j == objects.len() - 1 {
                args.push_str(&format!(" --start-lib {path} --end-lib"));
                library.push(input);
            } else {
                args.push_str(&format!(" {path}"));
                inputs.push(input);
            }
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
        // As merlin-ld's command line links (/bin/ld's too).
        wants.push(match merlin_ld::link_with_library(&inputs, &library, &opts) {
            Ok(linked) => {
                assert!(!linked.layout.warnings.is_empty(), "case {k} links cleanly");
                let said = linked
                    .layout
                    .warnings
                    .iter()
                    .map(|w| format!("ld: warning: {w}\n"));
                (said.collect(), Some(linked.elf))
            }
            Err(errors) => (errors.iter().map(|e| format!("ld: error: {e}\n")).collect(), None),
        });
        commands.push(args);
    }
    let input = format!("{}\nexit 0\n", commands.join("\n"));
    let (out, disk) = boot(image_of(&["ld"], &["/e"], &files), &input, 2_000_000_000);
    let fs = Fs::open(&disk).unwrap();
    for (k, (said, elf)) in wants.iter().enumerate() {
        let got = shell_output(&out, &commands[k]).replace("\r", "");
        let written = fs.read_file(&format!("/out/{k}")).ok();
        match elf {
            // An error: the messages, exit status 1, and no file.
            None => {
                assert_eq!(got, format!("{said}ld: exit 1\n"), "case {k}: {}", commands[k]);
                assert!(written.is_none(), "case {k} wrote /out/{k}");
            }
            // A warning: the messages, success, and merlin-ld's file.
            Some(elf) => {
                assert_eq!(got, *said, "case {k}: {}", commands[k]);
                assert!(
                    written.as_ref() == Some(elf),
                    "case {k}: /out/{k} is not merlin-ld's"
                );
            }
        }
    }
}
