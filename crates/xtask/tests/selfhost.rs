//! Self-hosting, step 1 (#135): the assembler that runs on Camelot
//! (`kernel/user/as`, `/bin/as`) writes, for every file of a corpus, the same
//! object the host's merlin-as writes, byte for byte. The test boots the disk
//! kernel on an image with `/bin/sh`, `/bin/as` and the corpus in `/src`, has
//! the shell run `as` on each file into `/out`, then reads the objects back
//! on the host and compares them. The corpus: merlin-as's golden encodings,
//! every hand-written `.s` of the repository, and merlin's assembly for a C
//! file (without `-g`: the line tables of `.file` and `.loc` are step 2). An
//! ignored test adds merlin's assembly for two big C files, the shell and the
//! kernel's vm.c, about 500 million instructions each inside Grail.

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
    for dir in ["kernel", "kernel/user", "kernel/boot", "crates/grail/rom"] {
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
    // merlin's assembly, as the kernel and the programs are built (without -g).
    let mut c_files = vec![("kernel/user", "hello.c", &[][..])];
    if big {
        c_files.extend([
            ("kernel/user", "sh.c", &[][..]),
            ("kernel", "vm.c", &["include"][..]),
        ]);
    }
    for (dir, file, includes) in c_files {
        let text = toolchain::without_debug_info(|| {
            toolchain::compile_c(&root.join(dir), file, includes, dir).unwrap()
        });
        out.push((format!("{}_{file}.s", dir.replace('/', "_")), text));
    }
    out
}

/// The name a file of the corpus has on the image: short enough for a
/// directory entry (DIRSIZ, kernel/include/fs.h).
fn short(i: usize) -> String {
    format!("f{i}")
}

/// An image with the shell and the assembler in /bin, `inputs` in /src
/// (named by `short`), and an empty /out.
fn image(inputs: &[(String, String)]) -> Vec<u8> {
    let bytes: usize = inputs.iter().map(|(_, t)| t.len()).sum();
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
    for name in ["sh", "as"] {
        let prog = userprogs::build_named(&user, name).unwrap();
        fs.add_file(&format!("/bin/{name}"), &prog.elf).unwrap();
    }
    fs.mkdir("/src").unwrap();
    fs.mkdir("/out").unwrap();
    for (i, (_, text)) in inputs.iter().enumerate() {
        fs.add_file(&format!("/src/{}.s", short(i)), text.as_bytes())
            .unwrap();
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
        "    .equ x, 1 / 0\n",
        "    addi a0, a1,\n    .bogus 1\n",
        "    .ascii \"\\q\"\n",
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
fn the_assemblers_tables_are_the_host_tools() {
    // kernel/user/as/tables.h and tables.c are generated from merlin-as's
    // and excalibur's tables (crates/xtask/src/as_tables.rs) by `cargo xtask
    // kernel`.
    use xtask::as_tables::{generate_c, generate_h, TABLES_C, TABLES_H};
    for (file, want) in [(TABLES_H, generate_h()), (TABLES_C, generate_c())] {
        let path = repo_root().join(file);
        let committed = std::fs::read_to_string(&path).unwrap().replace('\r', "");
        assert!(
            committed == want,
            "{} is out of date: run `cargo xtask kernel` and commit it",
            path.display()
        );
    }
}
