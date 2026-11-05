//! merlin-dbg on Camelot's processes: the kernel booted from the root image (`cargo
//! xtask fs`'s, built here the same way), whose shell runs `cat /README /README /README
//! | cat`, two processes running `/bin/cat` at once, at the same addresses. With the
//! kernel's symbols and cat's (with its line tables, as `cargo xtask kernel` writes it
//! next to the objects, keyed to the program: `add-symbol-file FILE program cat`):
//! `info processes` lists them, a breakpoint on a line of cat both run stops in one of
//! them only (`process PID`, then `satp VALUE`), the stops show that process's
//! arguments and memory, and `process PID` reads the other's memory through its page
//! table. The machine runs in this process under `grail::gdb::serve`, deterministic,
//! so every run of it does the same; the console of each debugged run is the
//! undebugged run's.
//!
//! At both widths: the RV32 kernel under Sv32, and the RV64 one, from an RV64 image
//! with RV64 programs, under Sv48 and under Sv39 (a hart without Sv48). Only a real
//! kernel takes the RV64 paths: the process table's 8-byte pointers read through the
//! kernel's DWARF, a high-half kernel pointer translated through the hart's Sv48 or
//! Sv39 table, satp's modes 9 and 8.

use grail::gdb::Outcome;
use grail::{DiskConfig, Machine, MachineConfig, Paging, RunResult, Xlen};
use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

fn path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)
}

/// What the shell is given: the pipeline, then `exit`, which halts the kernel.
const INPUT: &[u8] = b"cat /README /README /README | cat\nexit\n";

/// The machine a run boots: its width, its harts' paging, its disk kernel, and the
/// MODE field of satp `info processes` shows for every process.
#[derive(Clone, Copy)]
struct Setup {
    /// For the runs' caches.
    index: usize,
    xlen: Xlen,
    paging: Paging,
    kernel: &'static str,
    /// The top of satp: Sv32's bit 31 (MODE 1), or MODE 9 (Sv48) or 8 (Sv39) in
    /// bits 63:60.
    mode: u64,
}

const SV32: Setup = Setup {
    index: 0,
    xlen: Xlen::Rv32,
    paging: Paging::Sv48,
    kernel: "kernel/kernel-disk.elf",
    mode: 1 << 31,
};

const SV48: Setup = Setup {
    index: 1,
    xlen: Xlen::Rv64,
    paging: Paging::Sv48,
    kernel: "kernel/kernel-disk-rv64.elf",
    mode: 9 << 60,
};

const SV39: Setup = Setup {
    index: 2,
    xlen: Xlen::Rv64,
    paging: Paging::Sv39,
    kernel: "kernel/kernel-disk-rv64.elf",
    mode: 8 << 60,
};

/// The bootable root image of the setup's width, built once: as `cargo xtask fs`
/// builds it, for RV32; for RV64, the same of RV64 programs, the RV64 bootloader in
/// its boot block and the RV64 disk kernel after the filesystem.
fn image(s: Setup) -> &'static [u8] {
    static IMAGES: [OnceLock<Vec<u8>>; 2] = [OnceLock::new(), OnceLock::new()];
    let root = path(".");
    IMAGES[s.xlen as usize].get_or_init(|| match s.xlen {
        Xlen::Rv32 => xtask::fs::build_bootable(&root, xtask::fs::IMAGE_SIZE).unwrap(),
        Xlen::Rv64 => {
            let kernel = std::fs::read(path(s.kernel)).unwrap();
            let opts = xtask::fs::bootable_layout(xtask::fs::IMAGE_SIZE, &kernel).unwrap();
            let mut image = xtask::fs::build_for(&root, &opts, Xlen::Rv64).unwrap();
            let boot = xtask::boot::bootloader_rv64(&root).unwrap();
            camelot_fs::boot::install(&mut image, &boot, &kernel).unwrap();
            image
        }
    })
}

/// cat of the setup's width with its debugging information, and the program `/bin`
/// holds, the same code.
fn cat(s: Setup) -> &'static xtask::userprogs::Program {
    static CATS: [OnceLock<xtask::userprogs::Program>; 2] = [OnceLock::new(), OnceLock::new()];
    CATS[s.xlen as usize]
        .get_or_init(|| xtask::userprogs::build_named_for(&path("kernel/user"), "cat", s.xlen).unwrap())
}

#[derive(Clone, Default)]
struct Console(Arc<Mutex<Vec<u8>>>);

impl Write for Console {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Console {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// A machine booting the root image, the shell's input waiting.
fn machine(s: Setup) -> (Machine, Console) {
    let config = MachineConfig {
        xlen: s.xlen,
        paging: s.paging,
        disks: vec![DiskConfig::memory(image(s).to_vec())],
        ..MachineConfig::default()
    };
    let mut m = Machine::new(config).unwrap();
    m.boot_from_disk().unwrap();
    let console = Console::default();
    m.set_uart_output(console.clone());
    m.set_uart_input(xtask::kernel::typed(INPUT));
    (m, console)
}

/// The console of the run without a debugger, run once.
fn undebugged(s: Setup) -> &'static str {
    static RUNS: [OnceLock<String>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    RUNS[s.index].get_or_init(|| {
        let (mut m, console) = machine(s);
        assert_eq!(m.run(Some(2_000_000_000), |_| {}), RunResult::Exited(0));
        console.text()
    })
}

/// A debugged run: merlin-dbg connected to the machine's stub, with the disk
/// kernel's symbols, given `commands`. Its output; the console must be the
/// undebugged run's.
fn debugged(s: Setup, commands: &str) -> String {
    let (mut m, console) = machine(s);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let stub = std::thread::spawn(move || grail::gdb::serve(&mut m, listener, true, &mut |_| {}).unwrap());
    let mut child = Command::new(env!("CARGO_BIN_EXE_merlin-dbg"))
        .args(["--connect", &addr.to_string(), s.kernel])
        .current_dir(path("."))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(commands.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stub.join().unwrap(), Outcome::Exited(0), "{text}");
    assert_eq!(console.text(), undebugged(s), "a different run:\n{text}");
    text
}

/// The number of the first line of `file` that starts with `start` (trimmed).
fn line_of(file: &str, start: &str) -> u32 {
    let text = std::fs::read_to_string(path(file)).unwrap();
    text.lines()
        .position(|l| l.trim_start().starts_with(start))
        .unwrap_or_else(|| panic!("no line {start:?} in {file}")) as u32
        + 1
}

struct CatFile(PathBuf);

impl Drop for CatFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// cat's debug ELF in a file of its own, for `add-symbol-file`: one per test, which
/// run at once.
fn cat_file(s: Setup) -> CatFile {
    elf_file("cat", &cat(s).debug)
}

fn elf_file(name: &str, elf: &[u8]) -> CatFile {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!("merlin-dbg-{}-{n}-{name}.elf", std::process::id()));
    std::fs::write(&p, elf).unwrap();
    CatFile(p)
}

/// The lines `out` shows for the command `cmd` (echoed as `(merlin-dbg) cmd`): those
/// after its `nth` echo, up to the next command.
fn after(out: &str, cmd: &str, nth: usize) -> Vec<String> {
    let echo = format!("(merlin-dbg) {cmd}");
    let lines: Vec<&str> = out.lines().collect();
    let Some(at) = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| **l == echo)
        .nth(nth)
        .map(|(i, _)| i)
    else {
        return Vec::new();
    };
    lines[at + 1..]
        .iter()
        .take_while(|l| !l.starts_with("(merlin-dbg) "))
        .map(|l| l.to_string())
        .collect()
}

/// A process as `info processes` lists it: its pid, state, name and satp, whether it
/// is marked (`*`) and the harts that run it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Listed {
    marked: bool,
    pid: u64,
    state: String,
    name: String,
    satp: String,
    harts: String,
}

fn processes(block: &[String]) -> Vec<Listed> {
    assert_eq!(block[0], "  PID  State     Name             satp", "{block:?}");
    block[1..]
        .iter()
        .map(|l| {
            let words: Vec<&str> = l[2..].split_whitespace().collect();
            Listed {
                marked: l.starts_with('*'),
                pid: words[0].parse().unwrap(),
                state: words[1].to_string(),
                name: words[2].to_string(),
                satp: words[3].to_string(),
                harts: words[4..].join(" "),
            }
        })
        .collect()
}

/// The marked process of an `info processes` block.
fn marked(block: &[String]) -> u64 {
    let listed = processes(block);
    let p = listed
        .iter()
        .find(|p| p.marked)
        .unwrap_or_else(|| panic!("{block:?}"));
    p.pid
}

/// The value `print` showed: `$N = value`.
fn printed(block: &[String]) -> String {
    let line = &block[0];
    let (_, value) = line.split_once(" = ").unwrap_or_else(|| panic!("{block:?}"));
    value.to_string()
}

#[test]
fn breaking_in_one_of_two_processes_running_cat() {
    cat_pipeline(SV32);
}

#[test]
fn breaking_in_one_of_two_rv64_processes_under_sv48() {
    cat_pipeline(SV48);
}

#[test]
fn breaking_in_one_of_two_rv64_processes_under_sv39() {
    cat_pipeline(SV39);
}

fn cat_pipeline(s: Setup) {
    let cat_elf = cat_file(s);
    let cat_file = cat_elf.0.display().to_string();
    // The loaded program is the one on the image: -g changes no loaded byte.
    assert!(
        image(s).windows(cat(s).elf.len()).any(|w| w == cat(s).elf),
        "/bin/cat is not this cat"
    );
    let main = line_of("kernel/user/cat.c", "if (argc < 2) {");
    let write = line_of("kernel/user/cat.c", "int w = write(STDOUT");

    // First, breakpoints for every process: both cats stop in main, and on the line
    // that writes, each in its turn. The writer (`cat /README /README /README`) runs
    // first, to its main; then the reader (`cat`, its input the pipe) reaches its
    // main, before or after some of the writer's writes (the scheduler decides, and
    // decides otherwise at each width and paging mode). Each stop after the writer's
    // main is one of them: which, `breakpoint N` says.
    let mut commands = format!(
        "add-symbol-file {cat_file} program cat\n\
         break main\n\
         continue\n\
         print argc\n\
         print argv[1]\n\
         info processes\n\
         break kernel/user/cat.c:{write}\n"
    );
    for _ in 0..12 {
        commands.push_str("continue\ninfo processes\nprint argc\nprint fd\n");
    }
    let out = debugged(s, &commands);
    // The cat symbols are cat's alone: the breakpoint stops only where cat runs.
    assert!(
        out.contains(&format!("breakpoint 1 at {:#x} <main+", main_addr(s)))
            && out.contains(&format!(": kernel/user/cat.c:{main}, program cat\n")),
        "{out}"
    );
    assert_eq!(printed(&after(&out, "print argc", 0)), "4", "{out}");
    let argv1 = printed(&after(&out, "print argv[1]", 0));
    let (argv1, text) = argv1.split_once(' ').unwrap();
    assert_eq!(text, "\"/README\"", "{out}");
    let writer = marked(&after(&out, "info processes", 0));
    // The stops after it: the reader's main (breakpoint 1), and the write line
    // (breakpoint 2), with the process it stopped in and its fd: the writer's file (3)
    // or the reader's input (0).
    let mut main_stop = None;
    let mut stops = Vec::new();
    for n in 1..=12 {
        let hit = after(&out, "continue", n);
        match hit.first().map(String::as_str) {
            Some("breakpoint 1") => main_stop = Some(n),
            Some("breakpoint 2") => {
                let pid = marked(&after(&out, "info processes", n));
                stops.push((pid, printed(&after(&out, "print fd", n - 1))));
            }
            _ => break,
        }
    }
    let main_stop = main_stop.unwrap_or_else(|| panic!("the reader never reached main: {out}"));
    assert_eq!(printed(&after(&out, "print argc", main_stop)), "1", "{out}");
    let listed = processes(&after(&out, "info processes", main_stop));
    let reader = marked(&after(&out, "info processes", main_stop));
    assert_ne!(writer, reader);
    // The shell and the two cats, the one stopped running on hart 0.
    let names: Vec<(&str, &str)> = listed
        .iter()
        .map(|p| (p.name.as_str(), p.state.as_str()))
        .collect();
    assert_eq!(names, [("sh", "live"), ("cat", "live"), ("cat", "live")], "{out}");
    let reader_listed = listed.iter().find(|p| p.pid == reader).unwrap();
    assert_eq!(reader_listed.harts, "hart 0");
    let reader_satp = reader_listed.satp.clone();
    let satp = u64::from_str_radix(reader_satp.trim_start_matches("0x"), 16).unwrap();
    let mode_bits = match s.xlen {
        Xlen::Rv32 => 1 << 31,
        Xlen::Rv64 => 0xf << 60,
    };
    assert_eq!(satp & mode_bits, s.mode, "satp's mode: {out}");
    assert_ne!(listed.iter().find(|p| p.pid == writer).unwrap().satp, reader_satp);
    let writer_stops = stops.iter().filter(|s| s.0 == writer).count();
    let reader_stops: Vec<&(u64, String)> = stops.iter().filter(|s| s.0 == reader).collect();
    assert_eq!(writer_stops, 3, "one write per file: {stops:?}\n{out}");
    assert!(
        stops
            .iter()
            .all(|(p, fd)| fd == if *p == writer { "3" } else { "0" }),
        "{stops:?}"
    );
    // The writer writes first: the reader has nothing to write until it has.
    assert_eq!(stops[0].0, writer, "{stops:?}");
    let reads = reader_stops.len();
    assert!(reads >= 2, "{stops:?}");

    // Now the breakpoint for the reader alone, by its pid, then by its satp: it stops
    // there only, as often as the reader writes, and the writer's hits pass. At the
    // first stop: the reader's arguments and memory, and the writer's memory through
    // the writer's page table.
    let mut commands = format!(
        "add-symbol-file {cat_file} program cat\n\
         break main\n\
         continue\n\
         continue\n\
         delete\n\
         break kernel/user/cat.c:{write} process 99\n\
         break kernel/user/cat.c:{write} process {reader}\n\
         info breakpoints\n\
         continue\n\
         info processes\n\
         info args\n\
         print n\n\
         up\n\
         info args\n\
         print argv[0]\n\
         print *argv@argc\n\
         print (char *) {argv1}\n\
         process\n\
         process 99\n\
         process {writer}\n\
         print (char *) {argv1}\n\
         x/8b {argv1}\n\
         print buf\n\
         info processes\n\
         process\n"
    );
    for _ in 1..reads - 1 {
        commands.push_str("continue\ninfo processes\nprint fd\n");
    }
    commands.push_str(&format!(
        "delete\nbreak kernel/user/cat.c:{write} satp {reader_satp}\ncontinue\ninfo processes\nprint fd\ndelete\ncontinue\n"
    ));
    let out = debugged(s, &commands);
    assert!(
        out.contains(&format!(
            "(merlin-dbg) break kernel/user/cat.c:{write} process 99\nerror: no process 99\n"
        )),
        "{out}"
    );
    assert!(
        out.contains(&format!(
            "<copy+{}>: kernel/user/cat.c:{write}, process {reader}, program cat\n",
            copy_offset(s)
        )),
        "{out}"
    );
    let listed = after(&out, "info breakpoints", 0);
    assert!(
        listed[1].ends_with(&format!(", process {reader}, program cat")),
        "{listed:?}"
    );
    // Every stop is the reader's, on its input: as many as it has writes.
    let hits = out
        .lines()
        .filter(|l| *l == "breakpoint 2" || *l == "breakpoint 3")
        .count();
    assert_eq!(hits, reads, "{out}");
    let mut n = 0;
    while let Some(block) = Some(after(&out, "info processes", n)).filter(|b| !b.is_empty()) {
        if n != 1 {
            assert_eq!(marked(&block), reader, "stop {n}: {out}");
        }
        n += 1;
    }
    assert_eq!(n, reads + 1, "{out}");
    for i in 0..reads - 1 {
        assert_eq!(printed(&after(&out, "print fd", i)), "0", "{out}");
    }
    // The reader's arguments: copy's, then main's.
    let args = after(&out, "info args", 0);
    assert_eq!(args[0], "fd = 0", "{out}");
    assert!(
        args[1].starts_with("name = 0x") && args[1].ends_with(" \"input\""),
        "{out}"
    );
    let n: u32 = printed(&after(&out, "print n", 0)).parse().unwrap();
    assert!(n > 0 && n <= 512, "{out}");
    let args = after(&out, "info args", 1);
    assert_eq!(args[0], "argc = 1", "{out}");
    assert!(args[1].starts_with("argv = 0x"), "{out}");
    assert!(
        printed(&after(&out, "print argv[0]", 0)).ends_with(" \"cat\""),
        "{out}"
    );
    let all = printed(&after(&out, "print *argv@argc", 0));
    assert!(all.starts_with("{0x") && all.ends_with(" \"cat\"}"), "{out}");
    // The writer's second argument's address: in the reader, not "/README"; read
    // through the writer's page table, it is.
    let mine = printed(&after(&out, &format!("print (char *) {argv1}"), 0));
    assert!(!mine.contains("README"), "{mine}");
    let selected = after(&out, "process", 0);
    assert_eq!(
        selected,
        [format!(
            "print and x read process {reader} (cat)'s memory, as hart 0 sees it"
        )],
        "{out}"
    );
    assert_eq!(after(&out, "process 99", 0), ["error: no process 99"], "{out}");
    let chosen = after(&out, &format!("process {writer}"), 0);
    assert!(
        chosen[0].starts_with(&format!("process {writer} (cat), satp 0x")),
        "{out}"
    );
    let theirs = printed(&after(&out, &format!("print (char *) {argv1}"), 1));
    assert_eq!(theirs, format!("{argv1} \"/README\""), "{out}");
    let bytes = after(&out, &format!("x/8b {argv1}"), 0);
    assert_eq!(
        bytes,
        [format!(
            "{argv1}:  0x2f  0x52  0x45  0x41  0x44  0x4d  0x45  0x00"
        )],
        "{out}"
    );
    // cat's `buf`, a static: its address is the symbol's, the same in both cats (the
    // frames, the reader's, have no part in it). Read through the writer's page
    // table, it is the writer's, what it read from /README last. The reader's, read
    // from the pipe, starts the same way, so this shows the read works; the bytes at
    // argv[1] above are what tell the two address spaces apart.
    assert!(
        printed(&after(&out, "print buf", 0)).starts_with("\"Camelot root filesystem\\n"),
        "{out}"
    );
    let selected = processes(&after(&out, "info processes", 1));
    let w = selected.iter().find(|p| p.pid == writer).unwrap();
    let r = selected.iter().find(|p| p.pid == reader).unwrap();
    assert!(w.marked && w.harts.is_empty(), "{selected:?}");
    assert!(!r.marked && r.harts == "hart 0", "{selected:?}");
    assert_eq!(
        after(&out, "process", 1),
        [format!(
            "print and x read process {writer}'s memory, through its page table"
        )],
        "{out}"
    );
    // By satp: the last stop is the reader's too.
    assert!(
        out.contains(&format!(", satp {reader_satp}, program cat\n")),
        "{out}"
    );
    assert!(out.ends_with("program exited with code 0\n"), "{out}");
}

/// A location in several programs' symbols gets a breakpoint in each, as GDB gives a
/// breakpoint a location in every objfile that has it: with cat's and the shell's
/// keyed to their programs, `break main` is cat's main where cat runs and the shell's
/// where the shell does.
#[test]
fn a_location_in_two_programs_is_a_breakpoint_in_each() {
    let s = SV32;
    let cat_elf = cat_file(s);
    let sh = xtask::userprogs::build_named_for(&path("kernel/user"), "sh", s.xlen).unwrap();
    let sh_elf = elf_file("sh", &sh.debug);
    let commands = format!(
        "add-symbol-file {} program cat\n\
         add-symbol-file {} program sh\n\
         break main\n\
         info breakpoints\n\
         continue\n\
         info processes\n\
         continue\n\
         info processes\n\
         delete\n\
         continue\n",
        cat_elf.0.display(),
        sh_elf.0.display()
    );
    let out = debugged(s, &commands);
    let set = after(&out, "break main", 0);
    assert_eq!(set.len(), 2, "{out}");
    assert!(
        set[0].starts_with(&format!("breakpoint 1 at {:#x} <main+", main_addr(s)))
            && set[0].ends_with(", program cat"),
        "{out}"
    );
    assert!(
        set[1].starts_with("breakpoint 2 at 0x") && set[1].ends_with(", program sh"),
        "{out}"
    );
    // The machine starts at its first instruction: the shell reaches its main, then
    // the first cat its own; each stops at its program's breakpoint only.
    for (n, (hit, name)) in [("breakpoint 2", "sh"), ("breakpoint 1", "cat")]
        .into_iter()
        .enumerate()
    {
        assert_eq!(after(&out, "continue", n)[0], hit, "{out}");
        let listed = processes(&after(&out, "info processes", n));
        assert!(listed.iter().any(|p| p.marked && p.name == name), "{out}");
    }
    assert!(out.ends_with("program exited with code 0\n"), "{out}");
}

/// cat's `main` past its prologue, where `break main` stops.
fn main_addr(s: Setup) -> u64 {
    let symbols = merlin_dbg::symbols::Symbols::from_elf(&cat(s).debug).unwrap();
    let mut lines = merlin_dbg::source::Lines::default();
    lines.add_elf(&cat(s).debug).unwrap();
    let entry = symbols.lookup("main").unwrap();
    let (start, end) = symbols.range(entry).unwrap();
    lines.prologue_end(start, end).unwrap()
}

/// How far into `copy` its write line's code starts.
fn copy_offset(s: Setup) -> u64 {
    let symbols = merlin_dbg::symbols::Symbols::from_elf(&cat(s).debug).unwrap();
    let mut lines = merlin_dbg::source::Lines::default();
    lines.add_elf(&cat(s).debug).unwrap();
    let write = line_of("kernel/user/cat.c", "int w = write(STDOUT");
    let copy = symbols.lookup("copy").unwrap();
    let (addrs, _) = lines
        .breakpoints("kernel/user/cat.c", write, |a| {
            symbols.containing(a).map(|(_, s)| s)
        })
        .unwrap();
    addrs[0] - copy
}
