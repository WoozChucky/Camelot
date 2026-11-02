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

/// Checks that `out` has `lines`, in this order (other lines may come between).
fn assert_lines(out: &str, lines: &[&str]) {
    let mut rest = out.lines();
    for want in lines {
        assert!(rest.any(|l| l == *want), "no line {want:?} (in order) in:\n{out}");
    }
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
            "zero 0x00000000  ra   0x80000018  sp   0x80010070  gp   0x00000000",
            "s0   0x00000000  s1   0x00000000  a0   0x80000050  a1   0x00000000",
            "pc   0x80000020 <puts>  mode M",
            "0x80000050 <message>:  0x48  0x65  0x6c  0x6c  0x6f",
            "0x80000050 <message>:  0x6c6c6548  0x7266206f",
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
    assert!(
        out.contains(&format!(
            "pc   {kmain:#010x} <kmain>  mode S
"
        )),
        "{out}"
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
    let elf = std::env::temp_dir().join(format!("merlin-dbg-sh-{}.elf", std::process::id()));
    std::fs::write(&elf, &sh.debug).unwrap();
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
    let _ = std::fs::remove_file(&elf);
    let symbols = merlin_dbg::symbols::Symbols::from_elf(&sh.debug).unwrap();
    let main_at = symbols.lookup("main").unwrap();
    assert_lines(
        &out,
        &[
            &format!("breakpoint 1 at {main_at:#010x} <main>: kernel/user/sh.c:{main}"),
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
            "s0   0x00000001  s1   0x00000002  a0   0x00000001  a1   0x00000000",
            "hart 0",
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
}

#[test]
fn starting_grail_itself() {
    // merlin-dbg -- grail-args: the machine in merlin-dbg's process, its console on
    // merlin-dbg's stdout. Commands from a file (-x), then stdin.
    let script = std::env::temp_dir().join(format!("merlin-dbg-{}.cmds", std::process::id()));
    std::fs::write(&script, "# a comment\nbreak puts\ncontinue\n").unwrap();
    let out = merlin_dbg(
        &[
            "-x",
            script.to_str().unwrap(),
            "--",
            path("examples/hello/hello.elf").to_str().unwrap(),
        ],
        "bogus\nx/1 nowhere\ndelete 7\ncontinue\nregs\n",
    );
    let _ = std::fs::remove_file(&script);
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
