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
//! RV64: the same on RV64 machines, which the stub describes as `riscv:rv64`:
//! `examples/hello/hello-rv64.elf`, `examples/smp/smp-rv64.elf` with 2 harts, and
//! `tests/rv64/prog.c`, built here with merlin -g for RV64, for the source lines,
//! the steps and the LP64 frames. The kernel's RV64 case waits for an RV64 kernel
//! (#154).

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
        ..MachineConfig::default()
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
fn hello() {
    let out = connected(
        "examples/hello/hello.elf",
        1,
        b"",
        "break puts\n\
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
         continue\n",
    );
    assert_lines(
        &out,
        &[
            "=> 0x00001000:  auipc t0, 0x0",
            "(merlin-dbg) break puts",
            "breakpoint 1 at 0x80000020 <puts>",
            "(merlin-dbg) continue",
            "breakpoint 1",
            "=> 0x80000020 <puts>:  lui t0, 0x10000",
            "0x80000050 <message>:  0x48  0x65  0x6c  0x6c  0x6f",
            "0x80000050 <message>:  0x6c6c6548  0x7266206f",
            // GDB's giants and halfwords on RV32 too.
            "0x80000050 <message>:  0x7266206f6c6c6548  0x6c656d6143206d6f",
            "0x80000050 <message>:  0x6548  0x6c6c  0x206f",
            "=> 0x80000024 <puts+4>:  mv t1, a0",
            "=> 0x80000028 <puts+8>:  lbu t2, 0x0(t1)",
            "=> 0x8000002c <puts+12>:  beqz t2, 0x80000048 <puts+40>",
            "#0  0x8000002c <puts+12>",
            "#1  0x80000018 <_start+24>",
            "   0x80000024 <puts+4>:  mv t1, a0",
            "   0x80000028 <puts+8>:  lbu t2, 0x0(t1)",
            "=> 0x8000002c <puts+12>:  beqz t2, 0x80000048 <puts+40>",
            "program exited with code 0",
        ],
    );
    // GDB's info registers: the integers signed at 32 bits.
    assert_eq!(
        block_after(&out, "(merlin-dbg) regs"),
        gdb_regs(&[
            "ra             0x80000018\t0x80000018 <_start+24>",
            "sp             0x80010070\t0x80010070",
            "gp             0x0\t0x0",
            "tp             0x0\t0x0",
            "t0             0x80000000\t-2147483648",
            "fp             0x0\t0x0",
            "a0             0x80000050\t-2147483568",
            "pc             0x80000020\t0x80000020 <puts>",
            "priv           0x3\tprv:3 [Machine]",
        ])
    );
}

#[test]
fn kernel() {
    // kmain runs in S-mode with paging on: the stub reads memory through the hart's
    // page table. kvminit is one of its calls.
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
    // Addresses from the kernel's symbols, so a rebuilt kernel does not break the test.
    let file = std::fs::read(path("kernel/kernel.elf")).unwrap();
    let symbols = merlin_dbg::symbols::Symbols::from_elf(&file).unwrap();
    let kmain = symbols.lookup("kmain").unwrap();
    let kvminit = symbols.lookup("kvminit").unwrap();
    assert_lines(
        &out,
        &[
            &format!(
                "breakpoint 1 at {kmain:#010x} <kmain>: kernel/main.c:{}",
                line_of("kernel/main.c", "int kmain(void) {")
            ),
            "breakpoint 1",
            &format!("=> {kmain:#010x} <kmain>:  addi sp, sp, -0x10"),
            &format!("=> {:#010x} <kmain+4>:  sw ra, 0xc(sp)", kmain + 4),
            &format!(
                "breakpoint 2 at {kvminit:#010x} <kvminit>: kernel/vm.c:{}",
                line_of("kernel/vm.c", "void kvminit(void) {")
            ),
            "breakpoint 2",
            &format!("=> {kvminit:#010x} <kvminit>:  addi sp, sp, -0x10"),
        ],
    );
    assert_lines(
        &out,
        &[
            &format!("pc             {kmain:#x}\t{kmain:#x} <kmain>"),
            "priv           0x1\tprv:1 [Supervisor]",
        ],
    );
    // The backtrace: kvminit, called from kmain, called from the assembly that enters
    // S-mode (whose ra kmain saved; the chain ends there, s0 = 0).
    let bt: Vec<&str> = out
        .lines()
        .skip_while(|l| *l != "(merlin-dbg) bt")
        .skip(1)
        .take_while(|l| l.starts_with('#'))
        .collect();
    assert_eq!(bt.len(), 3, "{out}");
    assert_eq!(
        bt[0],
        format!(
            "#0  {kvminit:#010x} <kvminit> at kernel/vm.c:{}",
            line_of("kernel/vm.c", "void kvminit(void) {")
        )
    );
    assert!(bt[1].starts_with("#1  0x") && bt[1].contains(" <kmain+"), "{out}");
    assert!(bt[1].contains(" at kernel/main.c:"), "{out}");
    assert!(bt[2].contains(" <supervisor_entry+"), "{out}");
    let back = bt[1].trim_start_matches("#1  ").split(" at ").next().unwrap();
    // finish comes back there, and says what kvminit returned.
    assert!(
        out.contains(&format!(
            "run till exit, back to {back}
=> {back}:"
        )),
        "{out}"
    );
    assert!(out.contains("returned: a0 = "), "{out}");
    assert_lines(
        &out,
        &[
            "Num  Type        Where",
            &format!("1    breakpoint  {kmain:#010x} <kmain>"),
            &format!("2    breakpoint  {kvminit:#010x} <kvminit>"),
        ],
    );
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
    // kmain's first line, then line by line with next (over the calls), into a
    // call with step, past its prologue, a backtrace with lines, and the source.
    let kmain = line_of("kernel/main.c", "int kmain(void) {");
    let out = connected(
        "kernel/kernel.elf",
        1,
        b"hello\nexit\n",
        &format!(
            "break kernel/main.c:{kmain}\n\
             continue\n\
             next\n\
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
    let file = std::fs::read(path("kernel/kernel.elf")).unwrap();
    let symbols = merlin_dbg::symbols::Symbols::from_elf(&file).unwrap();
    let kmain_at = symbols.lookup("kmain").unwrap();
    assert_lines(
        &out,
        &[
            &format!("breakpoint 1 at {kmain_at:#010x} <kmain>: kernel/main.c:{kmain}"),
            "breakpoint 1",
            &format!("=> {kmain_at:#010x} <kmain>:  addi sp, sp, -0x10"),
            &format!("kernel/main.c:{kmain}: int kmain(void) {{"),
            "(merlin-dbg) next",
            "(merlin-dbg) next",
            "(merlin-dbg) step",
        ],
    );
    // Each stop shows its line, with the source's text; next went down kmain.
    let lines: Vec<u32> = source_lines(&out, "kernel/main.c")
        .iter()
        .map(|(n, _)| *n)
        .collect();
    assert!(lines.len() >= 3, "{out}");
    assert_eq!(lines[0], kmain);
    assert!(lines[0] < lines[1] && lines[1] < lines[2], "{lines:?}\n{out}");
    // step went into the call on that line: the backtrace has the caller on it.
    let bt: Vec<&str> = out
        .lines()
        .skip_while(|l| *l != "(merlin-dbg) bt")
        .skip(1)
        .take_while(|l| l.starts_with('#'))
        .collect();
    assert!(!bt[0].contains("<kmain"), "{out}");
    assert!(bt[0].contains(" at kernel/"), "{out}");
    assert!(
        bt[1].contains(" <kmain+") && bt[1].ends_with(&format!(" at kernel/main.c:{}", lines[2])),
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
            &format!("breakpoint 2 at {kmain_at:#010x} <kmain>: kernel/main.c:{kmain}"),
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
    let symbols = merlin_dbg::symbols::Symbols::from_elf(&sh.debug).unwrap();
    let main_at = symbols.lookup("main").unwrap();
    assert_lines(
        &out,
        &[
            &format!("breakpoint 1 at {main_at:#x} <main>: kernel/user/sh.c:{main}"),
            "breakpoint 1",
            &format!("kernel/user/sh.c:{main}: int main(void) {{"),
        ],
    );
    let lines: Vec<u32> = source_lines(&out, "kernel/user/sh.c")
        .iter()
        .map(|(n, _)| *n)
        .collect();
    assert!(lines.len() >= 4, "{out}");
    assert!(lines.windows(2).all(|w| w[0] < w[1]), "{lines:?}\n{out}");
    assert!(out.contains(" <_start+"), "{out}");
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
fn harts_and_watchpoints() {
    let out = connected(
        "examples/smp/smp.elf",
        2,
        b"",
        "break work\n\
         continue\n\
         continue\n\
         hart\n\
         regs\n\
         hart 0\n\
         x/1 $sp\n\
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
            "=> 0x80000070 <work>:  auipc t0, 0x0",
            "[hart 1] breakpoint 1",
            "=> 0x80000070 <work>:  auipc t0, 0x0",
            "* hart 1, M-mode, at 0x80000070 <work>",
            "hart 0",
            // Hart 0's stack, past every section: no symbol names it.
            "(merlin-dbg) x/1 $sp",
            "0x80010470:  0x00000000",
            "hart 1",
            "=> 0x80000074 <work+4>:  addi t0, t0, 0x3c0",
            "watchpoint 2 (write) at 0x80000430 <counter>, 4 bytes",
            "watchpoint 3 (read) at 0x8000043c <ready>, 4 bytes",
            "[hart 0] watchpoint 2 (write) hit at 0x80000430 <counter>",
            "=> 0x80000084 <work+20>:  addi t1, t1, -0x1",
            "[hart 1] watchpoint 2 (write) hit at 0x80000430 <counter>",
            "2    watchpoint  0x80000430 <counter>, 4 bytes, write",
            "3    watchpoint  0x8000043c <ready>, 4 bytes, read",
            "error: no hart 3: the machine has 2",
        ],
    );
    // Hart 1's registers, the same at both widths.
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

#[test]
fn starting_grail_itself() {
    // merlin-dbg -- grail-args: the machine in merlin-dbg's process, its console on
    // merlin-dbg's stdout. Commands from a file (-x), then stdin.
    let script = TempFile::new("script.cmds", b"# a comment\nbreak puts\ncontinue\n");
    let out = merlin_dbg(
        &[
            "-x",
            script.to_str(),
            "--",
            path("examples/hello/hello.elf").to_str().unwrap(),
        ],
        "bogus\nx/1 nowhere\ndelete 7\ncontinue\nregs\n",
    );
    assert_lines(
        &out,
        &[
            "(merlin-dbg) break puts",
            "breakpoint 1 at 0x80000020 <puts>",
            "breakpoint 1",
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
    // gets, stopped at a breakpoint.
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
    assert!(out.contains("(merlin-dbg) bt\n#0  0x"), "{out}");
    // quit with --connect detaches: the machine runs on, and the stub waits for the
    // next client. Kill it through one.
    let mut remote = merlin_dbg::remote::Remote::connect(addr).unwrap();
    remote.kill();
    assert_eq!(thread.join().unwrap(), Outcome::Killed);
}

// ---- RV64 ----

#[test]
fn hello_rv64() {
    // hello's session on an RV64 machine: the same addresses and code, the registers
    // and addresses with 16 digits, and x/g's 8-byte units.
    let out = connected(
        "examples/hello/hello-rv64.elf",
        1,
        b"",
        "break puts\n\
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
            "(merlin-dbg) break puts",
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
            "#0  0x000000008000002c <puts+12>",
            "#1  0x0000000080000018 <_start+24>",
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
fn harts_and_watchpoints_rv64() {
    let out = connected(
        "examples/smp/smp-rv64.elf",
        2,
        b"",
        "break work\n\
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
    // Hart 1's registers, the same at both widths.
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

/// `tests/rv64/prog.c`'s directory, from the repository root: the compilation
/// directory of its line tables, so they name `crates/merlin-dbg/tests/rv64/prog.c`
/// (as the kernel's name `kernel/main.c`).
const RV64_DIR: &str = "crates/merlin-dbg/tests/rv64";

/// `tests/rv64/prog.c` and `crt0.s` built for RV64 with our tools, `prog.c` with
/// its line tables (merlin -g), into an executable at 0x80000000 (merlin-ld's
/// default layout), written to a file of its own for `test`, removed with the
/// guard.
fn rv64_program(test: &str) -> TempFile {
    let xlen = excalibur::Xlen::Rv64;
    let dir = path(RV64_DIR);
    let opts = merlin::Options {
        xlen,
        debug: Some(merlin::DebugInfo {
            comp_dir: RV64_DIR.into(),
        }),
        ..merlin::Options::default()
    };
    let src = std::fs::read_to_string(dir.join("prog.c")).unwrap();
    let asm = merlin::compile_with("prog.c", &src, &opts, &merlin::no_files)
        .unwrap_or_else(|e| panic!("merlin: {e:?}"));
    let crt0 = std::fs::read_to_string(dir.join("crt0.s")).unwrap();
    let as_opts = merlin_as::Options { xlen };
    let inputs: Vec<merlin_ld::Input> = [("crt0.o", crt0), ("prog.o", asm)]
        .into_iter()
        .map(|(name, asm)| merlin_ld::Input {
            name: name.into(),
            data: merlin_as::assemble_to_elf_with(&asm, &as_opts)
                .unwrap_or_else(|e| panic!("merlin-as {name}: {e:?}")),
        })
        .collect();
    let linked = merlin_ld::link(&inputs, &merlin_ld::Options::default())
        .unwrap_or_else(|e| panic!("merlin-ld: {e:?}"));
    TempFile::new(&format!("rv64-{test}.elf"), &linked.elf)
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

#[test]
fn source_lines_of_an_rv64_program() {
    // main's first line, next by line over puts to the call of fact, step into
    // fact past its prologue, a backtrace with lines, list, and a line with no code.
    let elf = rv64_program("lines");
    let [main_at, fact_at] = addresses(&elf, ["main", "fact"]);
    let file = format!("{RV64_DIR}/prog.c");
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
            &format!("breakpoint 1 at {main_at:#x} <main>: {file}:{main}"),
            "breakpoint 1",
            &format!("=> {main_at:#018x} <main>:  addi sp, sp, -0x10"),
            &format!("{file}:{main}: int main(void) {{"),
        ],
    );
    // Each stop shows its line, with the source's text: main's, puts's call, fact's
    // call, then the first line of fact's body.
    let lines: Vec<u32> = source_lines(&out, &file).iter().map(|(n, _)| *n).collect();
    let want = [
        main,
        line_of(&file, "    puts(\"fact\\n\");"),
        line_of(&file, "    f = fact(5);"),
        line_of(&file, "    if (n <= 1)"),
    ];
    assert_eq!(lines[..4], want, "{out}");
    // step went into the call, past fact's prologue (which saves ra and s0 with sd):
    // fact, called from main on that line, called from _start.
    let bt = backtrace_after(&out, "(merlin-dbg) bt");
    assert_eq!(bt.len(), 3, "{out}");
    let pc = u64::from_str_radix(&bt[0]["#0  0x".len()..][..16], 16).unwrap();
    assert!(
        bt[0].contains(&format!(" <fact+{}>", pc - fact_at)) && pc - fact_at >= 16,
        "{out}"
    );
    assert!(bt[0].ends_with(&format!(" at {file}:{}", want[3])), "{out}");
    assert!(
        bt[1].starts_with("#1  0x00000000800") && bt[1].contains(" <main+"),
        "{out}"
    );
    assert!(bt[1].ends_with(&format!(" at {file}:{}", want[2])), "{out}");
    assert!(bt[2].contains(" <_start+"), "{out}");
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
    // The line before main has no code: the breakpoint goes to main's.
    assert_lines(
        &out,
        &[
            &format!("line {} has no code: {file}:{main} instead", main - 1),
            &format!("breakpoint 2 at {main_at:#x} <main>: {file}:{main}"),
        ],
    );
}

#[test]
fn stepping_through_an_rv64_program() {
    // A breakpoint in sum, a variadic function (its ra and s0 below the saved
    // argument registers), called from fact five deep in its recursion: the
    // backtrace, finish, next back up the recursion, a watchpoint on a 64-bit
    // global and its doublewords, and step into puts.
    let elf = rv64_program("steps");
    let [sum_at, total_at] = addresses(&elf, ["sum", "total"]);
    let file = format!("{RV64_DIR}/prog.c");
    let line = |start: &str| line_of(&file, start);
    let out = connected(
        elf.to_str(),
        1,
        b"",
        "break sum\n\
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
         continue\n",
    );
    let sum = line("long sum(int n, ...) {");
    let base_case = line("        return sum(1, 1L);");
    let recursion = line("    return n * fact(n - 1);");
    let call = line("    f = fact(5);");
    assert_lines(
        &out,
        &[
            &format!("breakpoint 1 at {sum_at:#x} <sum>: {file}:{sum}"),
            &format!("=> {sum_at:#018x} <sum>:  addi sp, sp, -0x50"),
        ],
    );
    // sum, fact(1) to fact(5), main, _start.
    let at = |l: &str| l.split(" at ").nth(1).map(str::to_string);
    let bt = backtrace_after(&out, "(merlin-dbg) bt");
    assert_eq!(bt.len(), 8, "{out}");
    assert_eq!(at(bt[0]), Some(format!("{file}:{sum}")));
    assert!(bt[1].contains(" <fact+"), "{out}");
    assert_eq!(at(bt[1]), Some(format!("{file}:{base_case}")));
    for f in &bt[2..6] {
        assert!(f.contains(" <fact+"), "{out}");
        assert_eq!(at(f), Some(format!("{file}:{recursion}")));
    }
    assert!(bt[6].contains(" <main+"), "{out}");
    assert_eq!(at(bt[6]), Some(format!("{file}:{call}")));
    assert!(bt[7].contains(" <_start+"), "{out}");
    // finish: back in fact(1), with sum's result.
    let back = bt[1].trim_start_matches("#1  ").split(" at ").next().unwrap();
    assert!(
        out.contains(&format!("run till exit, back to {back}\n=> {back}:")),
        "{out}"
    );
    assert_lines(&out, &["returned: a0 = 0x0000000000000001 (1)"]);
    // next: fact(1)'s closing brace, then fact(2)'s line of the call, and its brace;
    // the backtrace has lost fact(1).
    let lines: Vec<u32> = source_lines(&out, &file).iter().map(|(n, _)| *n).collect();
    assert_eq!(
        lines[..5],
        [sum, base_case, recursion + 1, recursion, recursion + 1],
        "{out}"
    );
    let second = out.replacen("(merlin-dbg) bt", "", 1);
    let bt = backtrace_after(&second, "(merlin-dbg) bt");
    assert_eq!(bt.len(), 6, "{out}");
    // The watchpoint: 8 bytes of total, written twice; the second time with the
    // high half set.
    assert_lines(
        &out,
        &[
            &format!("watchpoint 2 (write) at {total_at:#x} <total>, 8 bytes"),
            &format!("watchpoint 2 (write) hit at {total_at:#x} <total>"),
            &format!(
                "{file}:{}:     total = total + sum(3, 0x100000000L, 2L, 3L);",
                call + 2
            ),
            &format!("{total_at:#x} <total>:  0x0000000000000078"),
            &format!("watchpoint 2 (write) hit at {total_at:#x} <total>"),
            &format!("{file}:{}:     puts(\"done\\n\");", call + 3),
            &format!("{total_at:#x} <total>:  0x000000010000007d"),
            &format!("{file}:{}:     while (*s) {{", line("    while (*s) {")),
            &format!("{file}:{}:         putc(*s);", line("        putc(*s);")),
        ],
    );
}

#[test]
fn starting_an_rv64_grail_and_the_widths_agreeing() {
    // merlin-dbg -- an RV64 program: the machine is RV64 (the ELF64 program made it
    // so), and the stub says so.
    let out = merlin_dbg(
        &["--", path("examples/hello/hello-rv64.elf").to_str().unwrap()],
        "break puts\ncontinue\nx/1g $a0\ncontinue\n",
    );
    assert_lines(
        &out,
        &[
            "=> 0x0000000000001000:  auipc t0, 0x0",
            "breakpoint 1 at 0x80000020 <puts>",
            "0x80000050 <message>:  0x7266206f6c6c6548",
            "Hello from Camelot!",
            "program exited with code 0",
        ],
    );
    // An RV32 program's symbols for an RV64 machine: refused, and the machine,
    // left alone, runs on to its end.
    let (m, console) = machine("examples/hello/hello-rv64.elf", 1, b"");
    let (addr, thread) = stub(m);
    let out = Command::new(env!("CARGO_BIN_EXE_merlin-dbg"))
        .args([
            "--connect",
            &addr.to_string(),
            path("examples/hello/hello.elf").to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("hello.elf: an RV32 program (ELF32), but the machine is RV64"),
        "{text}"
    );
    assert_eq!(thread.join().unwrap(), Outcome::Exited(0));
    assert_eq!(console.text(), "Hello from Camelot!\n");
}
