//! merlin-dbg driving Grail's GDB stub, on `examples/hello`, `examples/smp` and the
//! Camelot kernel: breakpoints by symbol, continues, registers and memory, steps,
//! backtraces, watchpoints, harts, and runs to the end whose console output is the
//! output of the same program run without a debugger.
//!
//! The stub runs in this process (`grail::gdb::serve`, what `grail --gdb` runs), on a
//! machine whose console the test captures; merlin-dbg is the real binary, connected
//! with `--connect`, run in the repository root (where the line tables' paths, like
//! `kernel/main.c`, lead to the sources). `merlin-dbg -- ...` starts its own machine,
//! tested last. Grail's own tests check `grail --gdb` itself
//! (crates/grail/tests/gdb.rs).
//!
//! Source-level debugging: breaking on a line of the kernel and of a user program,
//! stepping by line, `list`, and LLDB reading the same line tables (ignored).
//!
//! The machines are RV64, which the stub describes as `riscv:rv64`: besides the
//! examples and the kernel, linked high and run with Sv48 on, `tests/prog/prog.c`,
//! built here with merlin -g, for the source lines, the steps and the LP64 frames.

use grail::gdb::Outcome;
use grail::{Machine, MachineConfig, RunResult};
use std::io::Write;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

fn path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)
}

/// A console the test can read.
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

/// `elf` on a machine with `harts` harts, its console captured and its input all
/// there from the start (so the run does not depend on when it arrives).
fn machine(elf: &str, harts: usize, input: &[u8]) -> (Machine, Console) {
    let config = MachineConfig {
        harts,
        ..config_for(elf)
    };
    let mut m = Machine::new(config).unwrap();
    m.load_elf(&std::fs::read(path(elf)).unwrap()).unwrap();
    let console = Console::default();
    m.set_uart_output(console.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    for &b in input {
        tx.send(b).unwrap();
    }
    m.set_uart_input(rx);
    (m, console)
}

/// The machine `elf` runs on: the default one, with no firmware, for the
/// bare-metal programs, but for the kernel, which runs in S-mode on Camelot's
/// firmware (`--bios default`), with the firmware's 2 MiB and its 2.1 MiB image
/// from 0x8020_0000 in 8 MiB.
fn config_for(elf: &str) -> MachineConfig {
    if elf == "kernel/kernel.elf" {
        MachineConfig {
            bios: grail::Bios::Default,
            ram_size: 8 << 20,
            ..MachineConfig::default()
        }
    } else {
        MachineConfig::default()
    }
}

/// The program's console output and exit code without a debugger.
fn undebugged(elf: &str, harts: usize, input: &[u8]) -> (String, u32) {
    let (mut m, console) = machine(elf, harts, input);
    let RunResult::Exited(code) = m.run(Some(1_000_000_000), |_| {}) else {
        panic!("{elf} did not exit")
    };
    (console.text(), code)
}

/// The machine under a stub on a free port, waiting for merlin-dbg.
fn stub(mut m: Machine) -> (SocketAddr, JoinHandle<Outcome>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let thread = std::thread::spawn(move || grail::gdb::serve(&mut m, listener, true, &mut |_| {}).unwrap());
    (addr, thread)
}

/// Runs merlin-dbg with `args` and `commands` on stdin. Returns its output.
fn merlin_dbg(args: &[&str], commands: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_merlin-dbg"))
        .args(args)
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
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

/// A file of the test's in the temporary directory, removed when the guard is
/// dropped: when the test ends, and when it fails part way.
struct TempFile(PathBuf);

impl TempFile {
    /// `name` (made unique to this process) with `data` in it.
    fn new(name: &str, data: &[u8]) -> TempFile {
        let path = std::env::temp_dir().join(format!("merlin-dbg-{}-{name}", std::process::id()));
        std::fs::write(&path, data).unwrap();
        TempFile(path)
    }

    fn to_str(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl std::ops::Deref for TempFile {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Checks that `out` has `lines`, in this order (other lines may come between).
fn assert_lines(out: &str, lines: &[&str]) {
    let mut rest = out.lines();
    for want in lines {
        assert!(rest.any(|l| l == *want), "no line {want:?} (in order) in:\n{out}");
    }
}

/// The lines `out` shows for the command `cmd` (echoed as `(merlin-dbg) cmd`): those
/// after it, up to the next command.
fn block_after(out: &str, cmd: &str) -> String {
    let lines: Vec<&str> = out
        .lines()
        .skip_while(|l| *l != cmd)
        .skip(1)
        .take_while(|l| !l.starts_with("(merlin-dbg) "))
        .collect();
    lines.join("\n")
}

/// What `regs` shows, in GDB's layout: `given`'s lines (`name`, then the value in
/// hex at column 15, a tab and its natural form), each register they leave out a
/// signed integer that is 0. `given` must have every pointer register (`ra`, `sp`,
/// `gp`, `tp`, `fp`), `pc` and `priv`.
fn gdb_regs(given: &[&str]) -> String {
    let names = [
        "ra", "sp", "gp", "tp", "t0", "t1", "t2", "fp", "s1", "a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7",
        "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "t3", "t4", "t5", "t6", "pc", "priv",
    ];
    let lines: Vec<String> = names
        .iter()
        .map(|name| {
            let line = given.iter().find(|l| l.split(' ').next() == Some(name));
            line.map_or(format!("{name:<15}0x0\t0"), |l| l.to_string())
        })
        .collect();
    lines.join("\n")
}

/// A debugged run over `--connect` of `elf`: merlin-dbg's output, and the console's,
/// which must be the undebugged run's, with the same exit code.
fn connected(elf: &str, harts: usize, input: &[u8], commands: &str) -> String {
    let (expected, code) = undebugged(elf, harts, input);
    let (m, console) = machine(elf, harts, input);
    let (addr, thread) = stub(m);
    let out = merlin_dbg(
        &["--connect", &addr.to_string(), path(elf).to_str().unwrap()],
        commands,
    );
    assert_eq!(thread.join().unwrap(), Outcome::Exited(code), "{out}");
    assert_eq!(console.text(), expected, "a different run:\n{out}");
    assert!(
        out.ends_with(&format!("program exited with code {code}\n")),
        "{out}"
    );
    out
}

#[test]
fn kernel() {
    // The kernel (#154) runs at its link addresses, high in the address space, in
    // S-mode with Sv48 on: a breakpoint by symbol is a link address, which the
    // hart's pc is once the kernel has jumped there, and the stub reads memory and
    // the stack through the hart's page table. kvminit is one of kmain's calls. Its
    // shell (#155) runs hello and exits, which ends the run.
    let out = connected(
        "kernel/kernel.elf",
        1,
        b"hello\nexit\n",
        "break kmain\n\
         continue\n\
         regs\n\
         stepi\n\
         break kvminit\n\
         continue\n\
         bt\n\
         x/2 $sp\n\
         nexti 5\n\
         finish\n\
         next\n\
         info breakpoints\n\
         delete\n\
         continue\n",
    );
    // Addresses from the kernel's symbols and line tables, so a rebuilt kernel does
    // not break the test. break FUNC stops past the prologue, as GDB's does: at the
    // function's first line of code, cpu_init() in kmain, initlock() in kvminit.
    let (kmain, kmain_off) = past_prologue(&kernel_elf(), "kmain");
    let (kvminit, kvminit_off) = past_prologue(&kernel_elf(), "kvminit");
    assert!(kmain >= 0xffff_ffff_8000_0000, "{kmain:#x}");
    let kmain_line = line_of("kernel/main.c", "    cpu_init();");
    let kvminit_line = line_of("kernel/vm.c", "    initlock(&uvm_lock, \"uvm\");");
    assert_lines(
        &out,
        &[
            &format!("breakpoint 1 at {kmain:#x} <kmain+{kmain_off}>: kernel/main.c:{kmain_line}"),
            "breakpoint 1",
            &format!("kernel/main.c:{kmain_line}:     cpu_init();"),
            &format!("breakpoint 2 at {kvminit:#x} <kvminit+{kvminit_off}>: kernel/vm.c:{kvminit_line}"),
            "breakpoint 2",
            &format!("kernel/vm.c:{kvminit_line}:     initlock(&uvm_lock, \"uvm\");"),
        ],
    );
    // Each stop's instruction, and stepi's: the call to cpu_init, auipc and jalr.
    for (at, name, inst) in [
        (kmain, format!("kmain+{kmain_off}"), "auipc ra, "),
        (kmain + 4, format!("kmain+{}", kmain_off + 4), "jalr "),
        (kvminit, format!("kvminit+{kvminit_off}"), ""),
    ] {
        let want = format!("=> {at:#018x} <{name}>:  {inst}");
        assert!(
            out.lines().any(|l| l.starts_with(&want)),
            "no {want:?} in:\n{out}"
        );
    }
    assert_lines(
        &out,
        &[
            &format!("pc             {kmain:#x}\t{kmain:#x} <kmain+{kmain_off}>"),
            "priv           0x1\tprv:1 [Supervisor]",
        ],
    );
    // The backtrace, as GDB's: kvminit, stopped at the start of a line (so without
    // its pc), called from kmain, called from the assembly that enters S-mode
    // (whose ra kmain saved; the chain ends there, s0 = 0), which has no debug
    // information: frames read through the page table, the callers' addresses
    // high in the address space, with 16 digits.
    let bt = backtrace_after(&out, "(merlin-dbg) bt");
    assert_eq!(bt.len(), 3, "{out}");
    assert_eq!(bt[0], format!("#0  kvminit () at kernel/vm.c:{kvminit_line}"));
    assert!(
        bt[1].starts_with("#1  0xffffffff8") && bt[1].contains(" in kmain () at kernel/main.c:"),
        "{out}"
    );
    assert_eq!(frame_pc(bt[1]).len(), 2 + 16, "{out}");
    assert!(
        bt[2].starts_with("#2  0xffffffff8") && bt[2].ends_with(" in supervisor_entry ()"),
        "{out}"
    );
    let back = frame_pc(bt[1]);
    // finish comes back there, and says what kvminit returned.
    assert!(
        out.contains(&format!("run till exit, back to {back} <kmain+")),
        "{out}"
    );
    assert!(out.contains("returned: a0 = "), "{out}");
    assert_lines(
        &out,
        &[
            "Num  Type        Where",
            &format!("1    breakpoint  {kmain:#010x} <kmain+{kmain_off}>"),
            &format!("2    breakpoint  {kvminit:#010x} <kvminit+{kvminit_off}>"),
        ],
    );
}

/// The pc a backtrace's line gives a caller's frame: `#1  0x80001234 in f () ...`.
fn frame_pc(line: &str) -> &str {
    let rest = &line[line.find("  0x").expect("a frame's pc") + 2..];
    rest.split(' ').next().unwrap()
}

/// The committed kernel.
fn kernel_elf() -> Vec<u8> {
    std::fs::read(path("kernel/kernel.elf")).unwrap()
}

/// Where `break FUNC` puts its breakpoint in the program `file`: past the
/// function's prologue, at its `prologue_end` row (or its second row), as GDB does.
/// The address, and its offset into the function.
fn past_prologue(file: &[u8], func: &str) -> (u64, u64) {
    let symbols = merlin_dbg::symbols::Symbols::from_elf(file).unwrap();
    let mut lines = merlin_dbg::source::Lines::default();
    lines.add_elf(file).unwrap();
    let entry = symbols.lookup(func).unwrap_or_else(|| panic!("no {func}"));
    let (start, end) = symbols.range(entry).unwrap();
    let body = lines
        .prologue_end(start, end)
        .unwrap_or_else(|| panic!("no lines for {func}"));
    assert!(body > entry, "{func}'s prologue");
    (body, body - entry)
}

/// The number of the first line of `file` (from the repository root) that starts
/// with `start`.
fn line_of(file: &str, start: &str) -> u32 {
    let text = std::fs::read_to_string(path(file)).unwrap();
    text.lines()
        .position(|l| l.starts_with(start))
        .unwrap_or_else(|| panic!("no line starting {start:?} in {file}")) as u32
        + 1
}

/// The lines `file:N: text` in `out` for `file`, as (N, text), each checked against
/// the source: the text must be line N's.
fn source_lines(out: &str, file: &str) -> Vec<(u32, String)> {
    let source = std::fs::read_to_string(path(file)).unwrap();
    let source: Vec<&str> = source.lines().collect();
    let prefix = format!("{file}:");
    out.lines()
        .filter_map(|l| {
            let rest = l.strip_prefix(&prefix)?;
            let (n, text) = rest.split_once(": ")?;
            let n: u32 = n.parse().ok()?;
            assert_eq!(text, source[n as usize - 1].trim_end(), "{file}:{n} in:\n{out}");
            Some((n, text.to_string()))
        })
        .collect()
}

#[test]
fn source_lines_of_the_kernel() {
    // A breakpoint on kmain's first line, which stops past its prologue on its
    // body's first line; next over that line's call, into the next line's call with
    // step, past its prologue, a backtrace with lines, and the source.
    let kmain = line_of("kernel/main.c", "int kmain(void) {");
    let out = connected(
        "kernel/kernel.elf",
        1,
        b"hello\nexit\n",
        &format!(
            "break kernel/main.c:{kmain}\n\
             continue\n\
             next\n\
             step\n\
             bt\n\
             list\n\
             list main.c:{kmain}\n\
             break main.c:{}\n\
             delete\n\
             continue\n",
            kmain - 1
        ),
    );
    // kmain's first line is its prologue: the breakpoint goes past it, to the first
    // line of its body, as GDB's does.
    let (body_at, off) = past_prologue(&kernel_elf(), "kmain");
    let body = line_of("kernel/main.c", "    cpu_init();");
    assert_lines(
        &out,
        &[
            &format!("breakpoint 1 at {body_at:#x} <kmain+{off}>: kernel/main.c:{body}"),
            "breakpoint 1",
            &format!("kernel/main.c:{body}:     cpu_init();"),
            "(merlin-dbg) next",
            "(merlin-dbg) step",
        ],
    );
    // Each stop shows its line, with the source's text; next went down kmain.
    let lines: Vec<u32> = source_lines(&out, "kernel/main.c")
        .iter()
        .map(|(n, _)| *n)
        .collect();
    assert!(lines.len() >= 2, "{out}");
    assert_eq!(lines[0], body);
    assert!(lines[0] < lines[1], "{lines:?}\n{out}");
    // step went into the call on that line: the backtrace has the caller on it.
    let bt: Vec<&str> = out
        .lines()
        .skip_while(|l| *l != "(merlin-dbg) bt")
        .skip(1)
        .take_while(|l| l.starts_with('#'))
        .collect();
    assert!(!bt[0].contains(" kmain "), "{out}");
    assert!(bt[0].contains(" at kernel/"), "{out}");
    assert!(
        bt[1].contains(" in kmain () at ") && bt[1].ends_with(&format!(" at kernel/main.c:{}", lines[1])),
        "{out}"
    );
    // list: ten numbered lines; list main.c:LINE centers on it.
    let listed: Vec<&str> = out
        .lines()
        .skip_while(|l| !l.ends_with(&format!(") list main.c:{kmain}")))
        .skip(1)
        .take(10)
        .collect();
    assert_eq!(listed[0], format!("{}\t{}", kmain - 5, source_line(kmain - 5)));
    assert_eq!(listed[5], format!("{kmain}\tint kmain(void) {{"));
    // The line before kmain has no code: the breakpoint goes to the next that has.
    assert_lines(
        &out,
        &[
            &format!("line {} has no code: kernel/main.c:{kmain} instead", kmain - 1),
            &format!("breakpoint 2 at {body_at:#x} <kmain+{off}>: kernel/main.c:{body}"),
        ],
    );
}

fn source_line(n: u32) -> String {
    let text = std::fs::read_to_string(path("kernel/main.c")).unwrap();
    text.lines().nth(n as usize - 1).unwrap().to_string()
}

#[test]
fn stepping_through_a_user_program() {
    // The shell, built with its line tables as `cargo xtask kernel` writes it next to
    // the objects; the kernel runs the same program without them. It is the first
    // user program to run, so its main is where the breakpoint stops.
    let sh = xtask::userprogs::build_named(&path("kernel/user"), "sh").unwrap();
    let elf = TempFile::new("sh.elf", &sh.debug);
    let main = line_of("kernel/user/sh.c", "int main(void) {");
    let out = connected(
        "kernel/kernel.elf",
        1,
        b"hello\nexit\n",
        &format!(
            "add-symbol-file {}\n\
             break kernel/user/sh.c:{main}\n\
             continue\n\
             next\n\
             next\n\
             step\n\
             bt\n\
             delete\n\
             continue\n",
            elf.display()
        ),
    );
    // main's first line is its prologue: the breakpoint is past it, on a line of
    // its body.
    let (main_at, off) = past_prologue(&sh.debug, "main");
    let at = format!("breakpoint 1 at {main_at:#x} <main+{off}>: kernel/user/sh.c:");
    let set = out
        .lines()
        .find(|l| l.starts_with(&at))
        .unwrap_or_else(|| panic!("no {at:?} in:\n{out}"));
    let body: u32 = set[at.len()..].parse().unwrap();
    assert!(body > main, "{out}");
    let lines: Vec<u32> = source_lines(&out, "kernel/user/sh.c")
        .iter()
        .map(|(n, _)| *n)
        .collect();
    assert_eq!(lines[0], body, "{out}");
    assert!(lines.len() >= 3, "{out}");
    assert!(lines.windows(2).all(|w| w[0] < w[1]), "{lines:?}\n{out}");
    assert!(out.contains(" in _start ()\n"), "{out}");
}

/// LLDB (from the LLVM installer) reads the kernel's line tables: an address's
/// line, and a breakpoint on a line. Ignored: it needs LLDB, and a Python runtime
/// on the PATH on Windows (see crates/grail/tests/gdb.rs). `LLDB` names the binary
/// if it is not on the PATH.
#[test]
#[ignore]
fn lldb_reads_the_line_tables() {
    let lldb = std::env::var("LLDB").unwrap_or_else(|_| "lldb".into());
    let file = std::fs::read(path("kernel/kernel.elf")).unwrap();
    let kmain_at = merlin_dbg::symbols::Symbols::from_elf(&file)
        .unwrap()
        .lookup("kmain")
        .unwrap();
    let kmain = line_of("kernel/main.c", "int kmain(void) {");
    let out = Command::new(&lldb)
        .arg("-b")
        .args([
            "-o",
            &format!("target create {}", path("kernel/kernel.elf").display()),
        ])
        .args(["-o", &format!("image lookup -a {kmain_at:#x}")])
        .args(["-o", &format!("breakpoint set -f main.c -l {kmain}")])
        .output()
        .unwrap_or_else(|e| panic!("cannot run {lldb}: {e}"));
    let text = String::from_utf8_lossy(&out.stdout);
    let checks: [&[&str]; 2] = [
        &["Summary: kernel.elf`kmain at main.c:", &format!(":{kmain}")],
        &["Breakpoint 1: where = kernel.elf`kmain", "at main.c:"],
    ];
    for parts in checks {
        assert!(
            text.lines().any(|l| parts.iter().all(|p| l.contains(p))),
            "no line with {parts:?} in:\n{text}{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn starting_grail_itself() {
    // merlin-dbg -- grail-args: the machine in merlin-dbg's process, its console on
    // merlin-dbg's stdout. Commands from a file (-x), then stdin. puts has no line
    // information, so break puts goes past its prologue by GDB's prologue analysis:
    // li t0 (lui) and mv t1 (addi) are prologue to it, the lbu is not, so puts+8,
    // the top of its loop (deleted, or each character would stop there).
    let script = TempFile::new("script.cmds", b"# a comment\nbreak puts\ncontinue\ndelete\n");
    let out = merlin_dbg(
        &[
            "-x",
            script.to_str(),
            "--",
            "--bios",
            "none",
            path("examples/hello/hello.elf").to_str().unwrap(),
        ],
        "bogus\nx/1 nowhere\ndelete 7\ncontinue\nregs\n",
    );
    assert_lines(
        &out,
        &[
            "(merlin-dbg) break puts",
            "breakpoint 1 at 0x80000028 <puts+8>",
            "breakpoint 1",
            "=> 0x0000000080000028 <puts+8>:  lbu t2, 0x0(t1)",
            "(merlin-dbg) delete",
            "(merlin-dbg) bogus",
            "error: unknown command 'bogus' (try help)",
            "error: no symbol 'nowhere'",
            "error: no breakpoint 7",
            "Hello from Camelot!",
            "program exited with code 0",
            "error: the program is not running",
        ],
    );

    // Quitting kills the machine: an endless kernel shell waiting for input it never
    // gets, stopped at a breakpoint (on Camelot's firmware, grail's default).
    let out = merlin_dbg(
        &["--", path("kernel/kernel.elf").to_str().unwrap()],
        "break kmain\ncontinue\nquit\n",
    );
    assert!(out.ends_with("(merlin-dbg) quit\n"), "{out}");

    // Mistakes in the arguments.
    let out = Command::new(env!("CARGO_BIN_EXE_merlin-dbg"))
        .args(["--", "--harts", "99", "x.elf"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stdout).contains("merlin-dbg: grail: --harts:"));
}

#[test]
fn interrupting_a_running_machine() {
    // The kernel's shell waits for input forever. Instead of Ctrl-C (a console event
    // a test cannot send portably), the library's interrupt flag, which Ctrl-C sets.
    use std::sync::atomic::{AtomicBool, Ordering};
    let (m, _console) = machine("kernel/kernel.elf", 1, b"");
    let (addr, thread) = stub(m);
    static INTERRUPT: AtomicBool = AtomicBool::new(false);
    let setter = std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(500));
        INTERRUPT.store(true, Ordering::SeqCst);
    });
    let args = [
        "--connect".to_string(),
        addr.to_string(),
        path("kernel/kernel.elf").display().to_string(),
    ];
    let mut input = std::io::Cursor::new(b"continue\nbt\nquit\n".to_vec());
    let mut out = Vec::new();
    assert_eq!(merlin_dbg::run(&args, &mut input, &mut out, &INTERRUPT), 0);
    setter.join().unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("(merlin-dbg) continue\ninterrupted\n=> 0x"), "{out}");
    // The innermost frame, its pc shown unless it starts a line, as in GDB.
    assert!(out.contains("(merlin-dbg) bt\n#0  "), "{out}");
    // quit with --connect detaches: the machine runs on, and the stub waits for the
    // next client. Kill it through one.
    let mut remote = merlin_dbg::remote::Remote::connect(addr).unwrap();
    remote.kill();
    assert_eq!(thread.join().unwrap(), Outcome::Killed);
}

#[test]
fn hello() {
    // hello's session: the registers and addresses with 16 digits, and x/g's 8-byte
    // units.
    let out = connected(
        "examples/hello/hello.elf",
        1,
        b"",
        "break *puts\n\
         continue\n\
         regs\n\
         x/5b $a0\n\
         x/2 message\n\
         x/2g message\n\
         x/3xh message\n\
         stepi\n\
         si 2\n\
         bt\n\
         disas $pc-8 3\n\
         finish\n\
         continue\n",
    );
    assert_lines(
        &out,
        &[
            "=> 0x0000000000001000:  auipc t0, 0x0",
            "(merlin-dbg) break *puts",
            "breakpoint 1 at 0x80000020 <puts>",
            "(merlin-dbg) continue",
            "breakpoint 1",
            "=> 0x0000000080000020 <puts>:  lui t0, 0x10000",
            "0x80000050 <message>:  0x48  0x65  0x6c  0x6c  0x6f",
            "0x80000050 <message>:  0x6c6c6548  0x7266206f",
            "0x80000050 <message>:  0x7266206f6c6c6548  0x6c656d6143206d6f",
            "0x80000050 <message>:  0x6548  0x6c6c  0x206f",
            "=> 0x0000000080000024 <puts+4>:  mv t1, a0",
            "=> 0x0000000080000028 <puts+8>:  lbu t2, 0x0(t1)",
            "=> 0x000000008000002c <puts+12>:  beqz t2, 0x80000048 <puts+40>",
            "#0  0x000000008000002c in puts ()",
            "#1  0x0000000080000018 in _start ()",
            "   0x0000000080000024 <puts+4>:  mv t1, a0",
            "   0x0000000080000028 <puts+8>:  lbu t2, 0x0(t1)",
            "=> 0x000000008000002c <puts+12>:  beqz t2, 0x80000048 <puts+40>",
            "run till exit, back to 0x0000000080000018 <_start+24>",
            "=> 0x0000000080000018 <_start+24>:  xcal.dbg a0",
            "returned: a0 = 0x0000000000000014 (20)",
            "program exited with code 0",
        ],
    );
    // GDB's info registers: the integers signed at 64 bits.
    assert_eq!(
        block_after(&out, "(merlin-dbg) regs"),
        gdb_regs(&[
            "ra             0x80000018\t0x80000018 <_start+24>",
            "sp             0x80010070\t0x80010070",
            "gp             0x0\t0x0",
            "tp             0x0\t0x0",
            "t0             0x80000000\t2147483648",
            "fp             0x0\t0x0",
            "a0             0x80000050\t2147483728",
            "pc             0x80000020\t0x80000020 <puts>",
            "priv           0x3\tprv:3 [Machine]",
        ])
    );
}

#[test]
fn harts_and_watchpoints() {
    let out = connected(
        "examples/smp/smp.elf",
        2,
        b"",
        "break *work\n\
         continue\n\
         continue\n\
         hart\n\
         regs\n\
         hart 0\n\
         x/1g $sp\n\
         hart 1\n\
         si\n\
         delete 1\n\
         watch counter\n\
         watch -r ready\n\
         continue\n\
         continue\n\
         info breakpoints\n\
         delete\n\
         hart 3\n\
         continue\n",
    );
    assert_lines(
        &out,
        &[
            "connected: 2 harts",
            "breakpoint 1 at 0x80000070 <work>",
            "[hart 0] breakpoint 1",
            "=> 0x0000000080000070 <work>:  auipc t0, 0x0",
            "[hart 1] breakpoint 1",
            "=> 0x0000000080000070 <work>:  auipc t0, 0x0",
            "* hart 1, M-mode, at 0x0000000080000070 <work>",
            "hart 0",
            // Hart 0's stack, past every section: no symbol names it.
            "(merlin-dbg) x/1g $sp",
            "0x80010470:  0x0000000000000000",
            "hart 1",
            "=> 0x0000000080000074 <work+4>:  addi t0, t0, 0x3c0",
            "watchpoint 2 (write) at 0x80000430 <counter>, 8 bytes",
            "watchpoint 3 (read) at 0x8000043c <ready>, 8 bytes",
            "[hart 0] watchpoint 2 (write) hit at 0x80000430 <counter>",
            "=> 0x0000000080000084 <work+20>:  addi t1, t1, -0x1",
            "[hart 1] watchpoint 2 (write) hit at 0x80000430 <counter>",
            "2    watchpoint  0x80000430 <counter>, 8 bytes, write",
            "3    watchpoint  0x8000043c <ready>, 8 bytes, read",
            "error: no hart 3: the machine has 2",
        ],
    );
    // Hart 1's registers.
    assert_eq!(
        block_after(&out, "(merlin-dbg) regs"),
        gdb_regs(&[
            "ra             0x0\t0x0",
            "sp             0x8000f470\t0x8000f470",
            "gp             0x0\t0x0",
            "tp             0x0\t0x0",
            "t0             0x30002000\t805314560",
            "t1             0x2000004\t33554436",
            "t2             0x1\t1",
            "fp             0x1\t0x1",
            "s1             0x2\t2",
            "a0             0x1\t1",
            "pc             0x80000070\t0x80000070 <work>",
            "priv           0x3\tprv:3 [Machine]",
        ])
    );
}

/// `tests/prog/prog.c`'s directory, from the repository root: the compilation
/// directory of its line tables, so they name `crates/merlin-dbg/tests/prog/prog.c`
/// (as the kernel's name `kernel/main.c`).
const PROG_DIR: &str = "crates/merlin-dbg/tests/prog";

/// `tests/prog/prog.c` and `crt0.s` built with our tools, `prog.c` with its line
/// tables (merlin -g), into an executable at 0x80000000 (merlin-ld's default
/// layout), written to a file of its own for `test`, removed with the guard.
fn prog_program(test: &str) -> TempFile {
    program(PROG_DIR, "prog.c", &format!("prog-{test}"))
}

/// `dir/file` (a directory from the repository root) and tests/prog/crt0.s built
/// with our tools, `file` with its debug information (merlin -g, `dir` its
/// compilation directory), into an executable at 0x80000000, written to a file
/// named for `test`, removed with the guard.
fn program(dir: &str, file: &str, test: &str) -> TempFile {
    let opts = merlin::Options {
        debug: Some(merlin::DebugInfo { comp_dir: dir.into() }),
        ..merlin::Options::default()
    };
    let src = std::fs::read_to_string(path(dir).join(file)).unwrap();
    let asm = merlin::compile_with(file, &src, &opts, &merlin::no_files)
        .unwrap_or_else(|e| panic!("merlin: {e:?}"));
    let crt0 = std::fs::read_to_string(path(PROG_DIR).join("crt0.s")).unwrap();
    let inputs: Vec<merlin_ld::Input> = [("crt0.o", crt0), ("prog.o", asm)]
        .into_iter()
        .map(|(name, asm)| merlin_ld::Input {
            name: name.into(),
            data: merlin_as::assemble_to_elf(&asm).unwrap_or_else(|e| panic!("merlin-as {name}: {e:?}")),
        })
        .collect();
    let linked = merlin_ld::link(&inputs, &merlin_ld::Options::default())
        .unwrap_or_else(|e| panic!("merlin-ld: {e:?}"));
    TempFile::new(&format!("{test}.elf"), &linked.elf)
}

/// The addresses of `names` in the program at `elf`, from its symbols (so that
/// merlin's code changing does not break the tests).
fn addresses<const N: usize>(elf: &Path, names: [&str; N]) -> [u64; N] {
    let symbols = merlin_dbg::symbols::Symbols::from_elf(&std::fs::read(elf).unwrap()).unwrap();
    names.map(|n| symbols.lookup(n).unwrap_or_else(|| panic!("no symbol {n}")))
}

/// The backtrace after the command `cmd` in `out`: its `#N` lines.
fn backtrace_after<'a>(out: &'a str, cmd: &str) -> Vec<&'a str> {
    out.lines()
        .skip_while(|l| *l != cmd)
        .skip(1)
        .take_while(|l| l.starts_with('#'))
        .collect()
}

/// The `n`th backtrace in `out` (from 0): the `#N` lines after the `n`th `bt`.
fn nth_backtrace(out: &str, n: usize) -> Vec<&str> {
    let mut lines = out.lines();
    for _ in 0..=n {
        lines.find(|l| *l == "(merlin-dbg) bt").expect("a bt");
    }
    lines.take_while(|l| l.starts_with('#')).collect()
}

#[test]
fn source_lines_of_a_program() {
    // A breakpoint on main's first line, which stops past main's prologue on the
    // call of puts (`long f;` has no code), next by line over it to the call of
    // fact, step into fact past its prologue, a backtrace with lines, list, and a
    // line with no code.
    let elf = prog_program("lines");
    let [fact_at] = addresses(&elf, ["fact"]);
    let (body_at, off) = past_prologue(&std::fs::read(&*elf).unwrap(), "main");
    let file = format!("{PROG_DIR}/prog.c");
    let main = line_of(&file, "int main(void) {");
    let fact = line_of(&file, "long fact(long n) {");
    let out = connected(
        elf.to_str(),
        1,
        b"",
        &format!(
            "break {file}:{main}\n\
             continue\n\
             next\n\
             step\n\
             bt\n\
             list\n\
             list prog.c:{fact}\n\
             break prog.c:{}\n\
             delete\n\
             continue\n",
            main - 1
        ),
    );
    assert_lines(
        &out,
        &[
            &format!("breakpoint 1 at {body_at:#x} <main+{off}>: {file}:{}", main + 2),
            "breakpoint 1",
            &format!("=> {body_at:#018x} <main+{off}>:  auipc a0, 0x0"),
            &format!("{file}:{}:     puts(\"fact\\n\");", main + 2),
        ],
    );
    // Each stop shows its line, with the source's text: puts's call, fact's call,
    // then the first line of fact's body.
    let lines: Vec<u32> = source_lines(&out, &file).iter().map(|(n, _)| *n).collect();
    let want = [
        line_of(&file, "    puts(\"fact\\n\");"),
        line_of(&file, "    f = fact(5);"),
        line_of(&file, "    if (n <= 1)"),
    ];
    assert_eq!(lines[..3], want, "{out}");
    // step went into the call, past fact's prologue (which saves ra and s0 with sd):
    // fact, called from main on that line, called from _start.
    let bt = backtrace_after(&out, "(merlin-dbg) bt");
    assert_eq!(bt.len(), 3, "{out}");
    let stop = out
        .lines()
        .find(|l| l.starts_with("=> 0x") && l.contains(" <fact+"))
        .unwrap_or_else(|| panic!("no stop in fact in:\n{out}"));
    let pc = u64::from_str_radix(&stop["=> 0x".len()..][..16], 16).unwrap();
    assert!(
        stop.contains(&format!(" <fact+{}>", pc - fact_at)) && pc - fact_at >= 16,
        "{out}"
    );
    // fact's frame, with its argument; main's, with its pc (the call's return
    // address); _start's, without debug information.
    assert_eq!(bt[0], format!("#0  fact (n=5) at {file}:{}", want[2]), "{out}");
    assert!(
        bt[1].starts_with("#1  0x00000000800") && bt[1].contains(" in main () at "),
        "{out}"
    );
    assert!(bt[1].ends_with(&format!(" at {file}:{}", want[1])), "{out}");
    assert!(bt[2].ends_with(" in _start ()"), "{out}");
    // list: ten numbered lines; list prog.c:LINE centers on it.
    let listed: Vec<&str> = out
        .lines()
        .skip_while(|l| !l.ends_with(&format!(") list prog.c:{fact}")))
        .skip(1)
        .take(10)
        .collect();
    let source = std::fs::read_to_string(path(&file)).unwrap();
    let text = |n: u32| source.lines().nth(n as usize - 1).unwrap().to_string();
    assert_eq!(listed[0], format!("{}\t{}", fact - 5, text(fact - 5)));
    assert_eq!(listed[5], format!("{fact}\tlong fact(long n) {{"));
    // The line before main has no code: the breakpoint goes to main's, and so past
    // its prologue.
    assert_lines(
        &out,
        &[
            &format!("line {} has no code: {file}:{main} instead", main - 1),
            &format!("breakpoint 2 at {body_at:#x} <main+{off}>: {file}:{}", main + 2),
        ],
    );
}

#[test]
fn stepping_through_a_program() {
    // Breakpoints in sum, a variadic function (its ra and s0 below the saved
    // argument registers, at CFA-64 and CFA-72), called from fact five deep in its
    // recursion: on its first statement (break sum) and in its loop, at the va_arg
    // line, both past its prologue, so that the backtraces read sum's saved ra and
    // s0 from its frame in memory; finish, next back up the recursion, a
    // watchpoint on a 64-bit global and its doublewords, and step into puts.
    let elf = prog_program("steps");
    let [sum_at, total_at] = addresses(&elf, ["sum", "total"]);
    let file = format!("{PROG_DIR}/prog.c");
    let line = |start: &str| line_of(&file, start);
    let va_arg = line("        s += va_arg(ap, long);");
    let out = connected(
        elf.to_str(),
        1,
        b"",
        &format!(
            "break sum\n\
         continue\n\
         bt\n\
         break prog.c:{va_arg}\n\
         continue\n\
         bt\n\
         finish\n\
         next\n\
         next\n\
         next\n\
         bt\n\
         delete\n\
         watch total\n\
         continue\n\
         x/1g total\n\
         continue\n\
         x/1g total\n\
         delete\n\
         step\n\
         step\n\
         continue\n"
        ),
    );
    // break sum stops past its prologue, on its body's first line: sum's frame is
    // built, and the backtrace reads its ra and s0 from their slots.
    let (body_at, off) = past_prologue(&std::fs::read(&*elf).unwrap(), "sum");
    assert_eq!(body_at - off, sum_at);
    let sum = line("    long s = 0;");
    let base_case = line("        return sum(1, 1L);");
    let recursion = line("    return n * fact(n - 1);");
    let call = line("    f = fact(5);");
    assert_lines(
        &out,
        &[
            &format!("breakpoint 1 at {body_at:#x} <sum+{off}>: {file}:{sum}"),
            &format!("=> {body_at:#018x} <sum+{off}>:  li a0, 0x0"),
        ],
    );
    // In the loop: the va_arg line, further into sum than its prologue, stays where
    // it is.
    let at = |l: &str| l.split(" at ").nth(1).map(str::to_string);
    let set = out
        .lines()
        .find(|l| l.starts_with("breakpoint 2 at ") && l.ends_with(&format!(": {file}:{va_arg}")))
        .unwrap_or_else(|| panic!("no breakpoint 2 at line {va_arg} in:\n{out}"));
    assert!(set.contains(" <sum+"), "{out}");
    assert_lines(
        &out,
        &[
            "breakpoint 2",
            &format!("{file}:{va_arg}:         s += va_arg(ap, long);"),
        ],
    );
    // Both times: sum, fact(1) to fact(5), main, _start. sum's own frame (built,
    // its ra and s0 read from CFA-64 and CFA-72) leads to fact(1).
    let first = backtrace_after(&out, "(merlin-dbg) bt");
    let bt = nth_backtrace(&out, 1);
    assert_eq!(bt.len(), 8, "{out}");
    assert_eq!(first[1..], bt[1..], "{out}");
    assert_eq!(at(first[0]), Some(format!("{file}:{sum}")));
    assert_eq!(at(bt[0]), Some(format!("{file}:{va_arg}")));
    // Each frame with its arguments: sum's count, then fact's n from 1 up to 5.
    assert!(bt[0].contains(" sum (n="), "{out}");
    assert!(bt[1].contains(" in fact (n=1) at "), "{out}");
    assert_eq!(at(bt[1]), Some(format!("{file}:{base_case}")));
    for (i, f) in bt[2..6].iter().enumerate() {
        assert!(f.contains(&format!(" in fact (n={}) at ", i + 2)), "{out}");
        assert_eq!(at(f), Some(format!("{file}:{recursion}")));
    }
    assert!(bt[6].contains(" in main () at "), "{out}");
    assert_eq!(at(bt[6]), Some(format!("{file}:{call}")));
    assert!(bt[7].ends_with(" in _start ()"), "{out}");
    // finish, from the loop: back in fact(1), with sum's result.
    let back = frame_pc(bt[1]);
    assert!(
        out.contains(&format!("run till exit, back to {back} <fact+")),
        "{out}"
    );
    assert_lines(&out, &["returned: a0 = 0x0000000000000001 (1)"]);
    // next: fact(1)'s closing brace, then fact(2)'s line of the call, and its brace;
    // the backtrace has lost fact(1).
    let lines: Vec<u32> = source_lines(&out, &file).iter().map(|(n, _)| *n).collect();
    assert_eq!(
        lines[..6],
        [sum, va_arg, base_case, recursion + 1, recursion, recursion + 1],
        "{out}"
    );
    assert_eq!(nth_backtrace(&out, 2).len(), 6, "{out}");
    // The watchpoint: 8 bytes of total, written twice; the second time with the
    // high half set.
    assert_lines(
        &out,
        &[
            &format!("watchpoint 3 (write) at {total_at:#x} <total>, 8 bytes"),
            &format!("watchpoint 3 (write) hit at {total_at:#x} <total>"),
            &format!(
                "{file}:{}:     total = total + sum(3, 0x100000000L, 2L, 3L);",
                call + 2
            ),
            &format!("{total_at:#x} <total>:  0x0000000000000078"),
            &format!("watchpoint 3 (write) hit at {total_at:#x} <total>"),
            &format!("{file}:{}:     puts(\"done\\n\");", call + 3),
            &format!("{total_at:#x} <total>:  0x000000010000007d"),
            &format!("{file}:{}:     while (*s) {{", line("    while (*s) {")),
            &format!("{file}:{}:         putc(*s);", line("        putc(*s);")),
        ],
    );
}

#[test]
fn starting_grail_and_an_rv32_program_left_out() {
    // merlin-dbg -- a program: the machine is RV64, and the stub says so. break puts
    // past its prologue by GDB's analysis.
    let out = merlin_dbg(
        &[
            "--",
            "--bios",
            "none",
            path("examples/hello/hello.elf").to_str().unwrap(),
        ],
        "break puts\ncontinue\nx/1g $a0\ndelete\ncontinue\n",
    );
    assert_lines(
        &out,
        &[
            "=> 0x0000000000001000:  auipc t0, 0x0",
            "breakpoint 1 at 0x80000028 <puts+8>",
            "=> 0x0000000080000028 <puts+8>:  lbu t2, 0x0(t1)",
            "0x80000050 <message>:  0x7266206f6c6c6548",
            "Hello from Camelot!",
            "program exited with code 0",
        ],
    );
    // An RV32 program's symbols (clang's, crates/elf/tests/llvm) for the machine:
    // left out with a warning, and the session goes on without them, as GDB's does.
    // The machine stays stopped at its first instruction, where the stub waited for
    // merlin-dbg (dropping the connection would have let it run off), until the
    // session runs it to a breakpoint and to its end.
    let (m, console) = machine("examples/hello/hello.elf", 1, b"");
    let (addr, thread) = stub(m);
    let rv32 = path("crates/elf/tests/llvm/rv32/prog.elf");
    let out = merlin_dbg(
        &["--connect", &addr.to_string(), rv32.to_str().unwrap()],
        "break puts\nbreak 0x80000020\ncontinue\ncontinue\n",
    );
    assert_lines(
        &out,
        &[
            &format!(
                "warning: {}: an RV32 program (ELF32): merlin-dbg debugs RV64 (ELF64) programs only: going on \
                 without its symbols",
                rv32.display()
            ),
            "=> 0x0000000000001000:  auipc t0, 0x0",
            "error: no symbol 'puts'",
            "breakpoint 1 at 0x80000020",
            "breakpoint 1",
            "=> 0x0000000080000020:  lui t0, 0x10000",
            "program exited with code 0",
        ],
    );
    assert_eq!(thread.join().unwrap(), Outcome::Exited(0));
    assert_eq!(console.text(), "Hello from Camelot!\n");
}

/// `tests/vars/vars.c`'s directory, from the repository root.
const VARS_DIR: &str = "crates/merlin-dbg/tests/vars";

/// The output of each command in `out`, in order: what follows its echoed line.
fn replies(out: &str, commands: &[&str]) -> Vec<String> {
    let mut rest = out;
    commands
        .iter()
        .map(|c| {
            let echo = format!("(merlin-dbg) {c}\n");
            let at = rest.find(&echo).unwrap_or_else(|| panic!("no {c:?} in:\n{out}"));
            rest = &rest[at + echo.len()..];
            let end = rest.find("(merlin-dbg) ").unwrap_or(rest.len());
            rest[..end].trim_end().to_string()
        })
        .collect()
}

#[test]
fn variables_and_types() {
    // tests/vars/vars.c: stopped in walk three deep in its
    // recursion, the arguments and locals of each frame (up and down), globals of
    // every kind of type, members, elements, pointers followed, the types, and a
    // function's struct arguments.
    let file = format!("{VARS_DIR}/vars.c");
    let add = line_of(&file, "    sum += list->value;");
    let rec = line_of(&file, "        sum = walk(list->next, depth + 1);");
    let call = line_of(&file, "        counter = walk(&nodes[0], 0) + inner;");
    let area_call = line_of(
        &file,
        "    total = (count_t)area(box.corners[0], box.corners[1]);",
    );
    let commands = [
        "bt",
        "info args",
        "info locals",
        "print list",
        "print *list",
        "print list->value",
        "print nodes",
        "print nodes[1].next->value + 1",
        "print box",
        "print box.corners[1].y",
        "print box.name",
        "print *box.name",
        "print grid",
        "print grid[1][2]",
        "print &counter",
        "print/x counter",
        "print big",
        "print walk",
        "print sizeof(Node)",
        "print &nodes[2] - &nodes[0]",
        "ptype Node",
        "ptype box",
        "ptype enum color",
        "ptype walk",
        "ptype list",
        "whatis nodes",
        "whatis Node",
        "whatis total",
        "whatis *box.name",
        "up",
        "info args",
        "up 2",
        "info locals",
        "print w.bytes",
        "print letter",
        "print depth",
        "down",
        "print depth",
        "frame 0",
        "print nope",
        "print *nodes@2",
        "print depth == 2 && list->next == 0",
        "print counter << 2 | 1",
        "print -1 < 0u",
        "print list->value > 25 || nope",
        // Casts: a struct is no cast's result or operand.
        "print (struct big)big",
        "print (struct point)big",
        "print (Node)counter",
        "print (unsigned)-1",
        "print (char)65",
        "print (long unsigned int)1 - 2",
        // An enum's values signed as its type (int) is.
        "print sv",
        "print sv + 0",
        "print sv < 0",
        "print/x sv",
        // C's integer types, named as GDB names them for LP64.
        "print sizeof(1L)",
        "whatis sizeof(int)",
        "whatis &nodes[2] - &nodes[0]",
        "whatis big + 1",
        "whatis 1L",
        "whatis 4294967296",
        "print 2147483647 + 1",
        // `@` between the shifts and `+`: grid[0][0]@(1+2).
        "print grid[0][0]@1+2",
        "print hi",
        "print/x hi",
        "print /x counter",
        "print $pc",
        "whatis $sp",
        "whatis $ra",
        "ptype nest",
        "print nest",
        "ptype old",
        "whatis old",
        // Frames past the outermost: up N goes as far as there are.
        "up 9",
        "up",
        "frame 9",
        "down 9",
        "down",
    ];
    let elf = program(VARS_DIR, "vars.c", "vars");
    let [nodes, counter, walk] = addresses(&elf, ["nodes", "counter", "walk"]);
    let ptr = 8;
    let node = |i: u64| match i {
        0 => format!("{nodes:#x} <nodes>"),
        i => format!("{:#x} <nodes+{}>", nodes + i * 2 * ptr, i * 2 * ptr),
    };
    let script = format!(
        "break vars.c:{add}\ncontinue\n{}\ndelete\nbreak area\ncontinue\nbt\ninfo args\nprint b.x - a.x\n\
         delete\ncontinue\n",
        commands.join("\n")
    );
    let out = connected(elf.to_str(), 1, b"", &script);
    let r = replies(&out, &commands);
    let at = |line: u32| format!("at {file}:{line}");
    // The backtrace: the innermost frame without its pc (it is at the start of
    // its line), the callers with theirs, each with its arguments and line.
    let bt: Vec<&str> = r[0].lines().collect();
    assert_eq!(bt.len(), 5, "{out}");
    assert_eq!(bt[0], format!("#0  walk (list={}, depth=2) {}", node(2), at(add)));
    for (i, depth) in [(1, 1), (2, 0)] {
        assert!(bt[i].starts_with(&format!("#{i}  0x")), "{out}");
        let want = format!(" in walk (list={}, depth={depth}) {}", node(depth), at(rec));
        assert!(bt[i].ends_with(&want), "{} in:\n{out}", want);
    }
    assert!(bt[3].ends_with(&format!(" in main () {}", at(call))), "{out}");
    assert!(bt[4].ends_with(" in _start ()"), "{out}");
    let want = [
        format!("list = {}\ndepth = 2", node(2)),
        "sum = 0\ncalls = 3".to_string(),
        format!("$1 = (Node *) {}", node(2)),
        "$2 = {value = 30, next = 0x0}".to_string(),
        "$3 = 30".to_string(),
        format!(
            "$4 = {{{{value = 10, next = {}}}, {{value = 20, next = {}}}, {{value = 30, next = 0x0}}}}",
            node(1),
            node(2)
        ),
        "$5 = 31".to_string(),
    ];
    assert_eq!(r[1..8], want, "{out}");
    assert!(r[8].starts_with("$6 = {color = BLUE, corners = {{x = 1, y = 2}, {x = 30, y = 40}}, name = 0x"));
    assert!(r[8].ends_with(" \"box\"}"), "{out}");
    assert_eq!(r[9], "$7 = 40");
    assert!(
        r[10].starts_with("$8 = 0x") && r[10].ends_with(" \"box\""),
        "{out}"
    );
    let want = [
        "$9 = 98 'b'".to_string(),
        "$10 = {{1, 2, 3}, {4, 5, 6}}".to_string(),
        "$11 = 6".to_string(),
        format!("$12 = (int *) {counter:#x} <counter>"),
        "$13 = 0x3".to_string(),
        "$14 = -5".to_string(),
        format!("$15 = {{int (Node *, int)}} {walk:#x} <walk>"),
        format!("$16 = {}", 2 * ptr),
        "$17 = 2".to_string(),
        "type = struct node {\n    int value;\n    struct node *next;\n}".to_string(),
        "type = struct shape {\n    enum color color;\n    struct point corners[2];\n    const char *name;\n}"
            .to_string(),
        "type = enum color {RED, GREEN = 5, BLUE}".to_string(),
        "type = int (Node *, int)".to_string(),
        "type = struct node {\n    int value;\n    struct node *next;\n} *".to_string(),
        "type = Node [3]".to_string(),
        "type = struct node".to_string(),
        "type = count_t".to_string(),
        "type = const char".to_string(),
    ];
    assert_eq!(r[11..29], want, "{out}");
    // up: walk one level out, its line the call's.
    assert!(r[29].starts_with("#1  0x"), "{out}");
    assert!(r[29].contains(&format!(" in walk (list={}, depth=1) {}\n", node(1), at(rec))));
    assert!(r[29].ends_with(&format!(
        "{file}:{rec}:         sum = walk(list->next, depth + 1);"
    )));
    assert_eq!(r[30], format!("list = {}\ndepth = 1", node(1)));
    // Two more: main, whose block's local comes first.
    assert!(r[31].contains(&format!(" in main () {}\n", at(call))), "{out}");
    assert_eq!(
        r[32..37],
        [
            "inner = 6\nw = {u = 287454020, bytes = \"D3\\\"\\021\"}\nletter = 65 'A'\ni = 3\nseen = 1",
            "$18 = \"D3\\\"\\021\"",
            "$19 = 65 'A'",
            "error: no symbol \"depth\" in current context",
            &format!(
                "#2  {} in walk (list={}, depth=0) {}\n{file}:{rec}:         sum = walk(list->next, depth + 1);",
                frame_pc(bt[2]),
                node(0),
                at(rec)
            ),
        ],
        "{out}"
    );
    assert_eq!(r[37], "$20 = 0");
    assert!(r[38].starts_with("#0  walk (list="), "{out}");
    assert_eq!(r[39], "error: no symbol \"nope\" in current context");
    // GDB's `@`, and C's comparisons, logical operators (the right one not
    // evaluated when the left decides), shifts and bitwise operators, with
    // C's conversions: -1 is not below 0u.
    assert_eq!(
        r[40..45],
        [
            format!(
                "$21 = {{{{value = 10, next = {}}}, {{value = 20, next = {}}}}}",
                node(1),
                node(2)
            ),
            "$22 = 1".to_string(),
            "$23 = 13".to_string(),
            "$24 = 0".to_string(),
            "$25 = 1".to_string(),
        ],
        "{out}"
    );
    // Casts, enums, C's integer types and conversions (a sum of ints wraps
    // as an int), GDB's `@` at its precedence, /x of a char array, the
    // registers' types, and the bodies of anonymous members' types.
    let (word_max, long_size, wide) = ("18446744073709551615", "8", "long");
    let invalid = "error: Invalid cast.";
    assert_eq!(
        r[45..66],
        [
            invalid.to_string(),
            invalid.to_string(),
            invalid.to_string(),
            "$26 = 4294967295".to_string(),
            "$27 = 65 'A'".to_string(),
            format!("$28 = {word_max}"),
            "$29 = NEG".to_string(),
            "$30 = -2".to_string(),
            "$31 = 1".to_string(),
            "$32 = 0xfffffffe".to_string(),
            format!("$33 = {long_size}"),
            "type = unsigned long".to_string(),
            "type = long".to_string(),
            "type = long long".to_string(),
            "type = long".to_string(),
            format!("type = {wide}"),
            "$34 = -2147483648".to_string(),
            "$35 = {1, 2, 3}".to_string(),
            "$36 = \"hi\\000\"".to_string(),
            "$37 = {0x68, 0x69, 0x0, 0x0}".to_string(),
            "$38 = 0x3".to_string(),
        ],
        "{out}"
    );
    assert!(
        r[66].starts_with("$39 = (void (*)()) 0x") && r[66].contains(" <walk+"),
        "{out}"
    );
    assert_eq!(
        r[67..73],
        [
            "type = void *",
            "type = void (*)()",
            "type = struct outer {\n    int k;\n    struct {\n        int a;\n        char c;\n    } in;\n    \
             union {\n        int i;\n        short s;\n    } un;\n}",
            "$40 = {k = 1, in = {a = 2, c = 99 'c'}, un = {i = 3, s = 3}}",
            "type = int ()",
            "type = int ()",
        ],
        "{out}"
    );
    // up 9 goes to the outermost frame, and up from there cannot; down 9 to
    // the innermost, and down from there cannot; frame 9 is no frame.
    assert!(
        r[73].starts_with("#4  0x") && r[73].ends_with(" in _start ()"),
        "{out}"
    );
    assert_eq!(r[74], "error: Initial frame selected; you cannot go up.");
    assert_eq!(r[75], "error: No frame at level 9.");
    assert!(r[76].starts_with("#0  walk (list="), "{out}");
    assert_eq!(
        r[77],
        "error: Bottom (innermost) frame selected; you cannot go down."
    );
    // area's struct arguments: `...` in a backtrace, as GDB's frame-arguments
    // scalars, and in full in info args.
    assert_lines(
        &out,
        &[
            &format!(
                "#0  area (a=..., b=...) at {file}:{}",
                line_of(&file, "    int w = b.x - a.x;")
            ),
            "(merlin-dbg) info args",
            "a = {x = 1, y = 2}",
            "b = {x = 30, y = 40}",
            "$41 = 29",
        ],
    );
    assert!(out.contains(&format!(" in main () {}\n", at(area_call))), "{out}");
}

#[test]
fn variables_in_the_kernel() {
    // The shell's first system call, a write: sys_write, called from syscall,
    // called from user_trap, which the trap vector (assembly) jumped to from U-mode:
    // the chain of frames ends there, at the program's s0. Each frame's arguments,
    // the trap frame's members and its registers (an array), and the process (a
    // struct reached by a pointer, its name a char array).
    let out = connected(
        "kernel/kernel.elf",
        1,
        b"hello\nexit\n",
        "break sys_write\n\
         continue\n\
         bt\n\
         info args\n\
         print f->scause\n\
         print f->regs[17]\n\
         up\n\
         info locals\n\
         print p->name\n\
         print p->pid\n\
         print num\n\
         whatis p\n\
         ptype f\n\
         up\n\
         info args\n\
         delete\n\
         continue\n",
    );
    let line = |file: &str, start: &str| format!("{file}:{}", line_of(file, start));
    let bt = backtrace_after(&out, "(merlin-dbg) bt");
    assert_eq!(bt.len(), 3, "{out}");
    let f = bt[0]
        .strip_prefix("#0  sys_write (f=")
        .and_then(|r| r.split(')').next())
        .unwrap_or_else(|| panic!("{out}"));
    assert!(f.starts_with("0x"), "{out}");
    // sys_write's first line of code, the one after its name's.
    let body = format!(
        "kernel/sysfile.c:{}",
        line_of("kernel/sysfile.c", "long sys_write(struct trap_frame *f) {") + 1
    );
    assert_eq!(bt[0], format!("#0  sys_write (f={f}) at {body}"));
    assert!(bt[1].ends_with(&format!(
        " in syscall (f={f}) at {}",
        line("kernel/syscall.c", "        ret = sys_write(f);")
    )));
    assert!(bt[2].ends_with(&format!(
        " in user_trap (f={f}) at {}",
        line("kernel/trap.c", "        syscall(f);")
    )));
    let r = replies(
        &out,
        &[
            "info args",
            "print f->scause",
            "print f->regs[17]",
            "up",
            "info locals",
            "print p->name",
            "print p->pid",
            "print num",
            "whatis p",
            "ptype f",
            "up",
            "info args",
        ],
    );
    assert_eq!(r[0], format!("f = {f}"));
    assert_eq!(r[1], "$1 = 8", "{out}"); // CAUSE_ECALL_U
    assert_eq!(r[2], "$2 = 2", "{out}"); // a7: SYS_write
    assert!(
        r[3].starts_with("#1  0x") && r[3].contains(" in syscall (f="),
        "{out}"
    );
    // syscall's locals: the process, the call's number, the stack pointer.
    let locals: Vec<&str> = r[4].lines().collect();
    assert!(locals.iter().any(|l| l.starts_with("p = 0x")), "{out}");
    assert!(locals.contains(&"num = 2"), "{out}");
    assert!(locals.iter().any(|l| l.starts_with("sp = ")), "{out}");
    assert_eq!(r[5], "$3 = \"sh\", '\\000' <repeats 13 times>", "{out}");
    assert_eq!(r[6], "$4 = 1", "{out}");
    assert_eq!(r[7], "$5 = 2", "{out}");
    assert_eq!(r[8], "type = struct proc *");
    assert!(
        r[9].starts_with("type = struct trap_frame {\n    uintptr regs[32];\n    uintptr sepc;\n"),
        "{out}"
    );
    assert!(r[9].ends_with("} *"), "{out}");
    assert!(
        r[10].starts_with("#2  0x") && r[10].contains(" in user_trap (f="),
        "{out}"
    );
    assert_eq!(r[11], format!("f = {f}"));
}

/// `tests/vars/vars.c` built by clang (`CLANG`, -O0 -g) and linked by ld.lld (`LLD`)
/// with tests/prog/crt0.s (which merlin-as assembles: it has Grail's `xcal.halt`),
/// into files named for `test`.
fn clang_program(test: &str) -> TempFile {
    let clang = std::env::var("CLANG").unwrap_or_else(|_| "clang".into());
    let lld = std::env::var("LLD").unwrap_or_else(|_| format!("ld.lld{}", std::env::consts::EXE_SUFFIX));
    let (bits, abi) = (64, "lp64");
    let obj = TempFile::new(&format!("{test}-clang-vars-{bits}.o"), b"");
    let out = Command::new(&clang)
        .current_dir(path(VARS_DIR))
        .args([
            &format!("--target=riscv{bits}"),
            &format!("-march=rv{bits}ima_zicsr_zifencei"),
            &format!("-mabi={abi}"),
            "-mcmodel=medany",
            "-mno-relax",
            "-O0",
            "-g",
            &format!("-fdebug-compilation-dir={VARS_DIR}"),
            "-ffreestanding",
            "-fno-builtin",
            "-fno-pic",
            "-c",
            "vars.c",
            "-o",
        ])
        .arg(&*obj)
        .output()
        .unwrap_or_else(|e| panic!("cannot run {clang}: {e}"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let crt0 = std::fs::read_to_string(path(PROG_DIR).join("crt0.s")).unwrap();
    let crt0 = merlin_as::assemble_to_elf(&crt0).unwrap();
    let crt0 = TempFile::new(&format!("{test}-clang-crt0-{bits}.o"), &crt0);
    let elf = TempFile::new(&format!("{test}-clang-vars-{bits}.elf"), b"");
    // Everything in RAM, from 0x80000000, as merlin-ld lays a program out.
    let script = TempFile::new(
        &format!("{test}-clang-vars-{bits}.ld"),
        b"SECTIONS {\n  . = 0x80000000;\n  .text : { *(.text*) }\n  .rodata : { *(.rodata*) *(.srodata*) }\n  \
          .data : { *(.data*) *(.sdata*) }\n  .bss : { *(.bss*) *(.sbss*) *(COMMON) }\n}\n",
    );
    let out = Command::new(&lld)
        .args(["-e", "_start", "-T"])
        .arg(&*script)
        .arg(&*crt0)
        .arg(&*obj)
        .arg("-o")
        .arg(&*elf)
        .output()
        .unwrap_or_else(|e| panic!("cannot run {lld}: {e}"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    elf
}

/// What LLDB shows of the variables of `elf` (vars.c built by merlin or clang),
/// stopped in walk three calls deep: `frame variable` in walk's innermost frame,
/// one out, and in main, and `target variable` of the globals. Every number in
/// hex, an address, is left out: the two builds lay memory out differently.
fn lldb_variables(elf: &Path) -> Vec<String> {
    let lldb = std::env::var("LLDB").unwrap_or_else(|_| "lldb".into());
    let file = format!("{VARS_DIR}/vars.c");
    let add = line_of(&file, "    sum += list->value;");
    let (m, _console) = machine(elf.to_str().unwrap(), 1, b"");
    let (addr, thread) = stub(m);
    let script = [
        format!("target create {}", elf.display()),
        format!("gdb-remote {addr}"),
        format!("breakpoint set -f vars.c -l {add}"),
        "continue".into(),
        "frame variable".into(),
        "frame variable *list".into(),
        "up".into(),
        "frame variable".into(),
        "up 2".into(),
        "frame variable".into(),
        "target variable counter nodes box grid big total".into(),
        "breakpoint delete --force".into(),
        "continue".into(),
    ];
    let mut cmd = Command::new(&lldb);
    cmd.arg("-b");
    for line in &script {
        cmd.args(["-o", line]);
    }
    let out = cmd.output().unwrap_or_else(|e| panic!("cannot run {lldb}: {e}"));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        text.contains("exited with status = 0"),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(thread.join().unwrap(), Outcome::Exited(0));
    // The variables: from each `frame variable` and `target variable` to the next
    // command, without the addresses.
    let mut lines = Vec::new();
    let mut keep = false;
    for l in text.lines() {
        if l.starts_with("(lldb) ") {
            keep = l.contains(" variable");
            if keep {
                lines.push(l.to_string());
            }
            continue;
        }
        if keep && !l.trim().is_empty() {
            lines.push(without_addresses(l));
        }
    }
    lines
}

/// `line` with every hex number, an address, as `ADDR`.
fn without_addresses(line: &str) -> String {
    let mut s = String::new();
    let mut rest = line;
    while let Some(i) = rest.find("0x") {
        s += &rest[..i];
        let hex = rest[i + 2..]
            .find(|c: char| !c.is_ascii_hexdigit())
            .unwrap_or(rest.len() - i - 2);
        s += "ADDR";
        rest = &rest[i + 2 + hex..];
    }
    s + rest
}

/// merlin-dbg reads clang's debug information as it reads merlin's: on
/// tests/vars/vars.c built by each, stopped at the same line, the backtrace, the
/// arguments and locals of each frame, values of every kind of type and `ptype`
/// are the same, addresses aside. clang's locations are `DW_OP_breg2` (from `sp`,
/// a caller's from the CFA of the frame it called) and `DW_OP_addrx`, merlin's
/// `DW_OP_fbreg` and `DW_OP_addr`. Ignored: it needs clang and
/// ld.lld (`CLANG` and `LLD` name them if they are not on the PATH).
#[test]
#[ignore]
fn merlin_dbg_reads_clangs_variables_as_merlins() {
    let file = format!("{VARS_DIR}/vars.c");
    let add = line_of(&file, "    sum += list->value;");
    let script = format!(
        "break vars.c:{add}\ncontinue\nbt\ninfo args\ninfo locals\nprint *list\nprint nodes\nprint box\n\
         print grid\nprint big\nprint total\nptype Node\nptype box\nwhatis nodes\nup\ninfo args\ninfo locals\n\
         up 2\ninfo locals\nprint w\ndelete\nbreak area\ncontinue\nbt\ninfo args\ndelete\ncontinue\n"
    );
    // What the session shows of the program, without the lines that depend on
    // its code (a stop's instruction, a breakpoint's address) and the addresses.
    let shown = |elf: &Path| -> Vec<String> {
        let out = connected(elf.to_str().unwrap(), 1, b"", &script);
        out.lines()
            .filter(|l| !l.starts_with("=> ") && !l.starts_with("breakpoint ") && !l.ends_with(" ()"))
            .map(without_addresses)
            .collect()
    };
    let merlin = program(VARS_DIR, "vars.c", "dbg-vars");
    let clang = clang_program("dbg");
    let ours = shown(&merlin);
    assert!(
        ours.iter().any(|l| l == "$1 = {value = 30, next = ADDR}"),
        "{ours:#?}"
    );
    assert_eq!(ours, shown(&clang), "merlin's (left) and clang's (right)");
}

/// LLDB reads merlin's debug information as it reads clang's: tests/vars/vars.c
/// built by each, stopped at the same line, shows the same variables, of the same
/// types, with the same values, in walk's frames and in main, and the same globals.
/// Ignored: it needs LLDB (with a Python runtime on the PATH on
/// Windows, see crates/grail/tests/gdb.rs), clang and ld.lld (`LLDB`, `CLANG` and
/// `LLD` name them if they are not on the PATH).
#[test]
#[ignore]
fn lldb_reads_merlins_variables_as_clangs() {
    let merlin = program(VARS_DIR, "vars.c", "lldb-vars");
    let clang = clang_program("lldb");
    let ours = lldb_variables(&merlin);
    let theirs = lldb_variables(&clang);
    assert!(
        ours.iter().any(|l| l.contains("(Node *) list = ADDR")),
        "{ours:#?}"
    );
    assert!(ours.iter().any(|l| l.contains("(int) depth = 2")), "{ours:#?}");
    assert!(
        ours.iter().any(|l| l.contains("(short[2][3]) grid = {")),
        "{ours:#?}"
    );
    assert!(
        ours.iter().any(|l| l.contains("(count_t) total = 0")),
        "{ours:#?}"
    );
    assert_eq!(ours, theirs, "merlin's (left) and clang's (right)");
}
