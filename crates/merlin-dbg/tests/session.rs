//! merlin-dbg driving Grail's GDB stub, on `examples/hello`, `examples/smp` and the
//! Camelot kernel: breakpoints by symbol, continues, registers and memory, steps,
//! backtraces, watchpoints, harts, and runs to the end whose console output is the
//! output of the same program run without a debugger.
//!
//! The stub runs in this process (`grail::gdb::serve`, what `grail --gdb` runs), on a
//! machine whose console the test captures; merlin-dbg is the real binary, connected
//! with `--connect`. `merlin-dbg -- ...` starts its own machine, tested last. Grail's own
//! tests check `grail --gdb` itself (crates/grail/tests/gdb.rs).

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
    assert_lines(
        &out,
        &[
            "breakpoint 1 at 0x800174dc <kmain>",
            "breakpoint 1",
            "=> 0x800174dc <kmain>:  addi sp, sp, -0x10",
            "=> 0x800174e0 <kmain+4>:  sw ra, 0xc(sp)",
            "breakpoint 2 at 0x8003f660 <kvminit>",
            "breakpoint 2",
            "=> 0x8003f660 <kvminit>:  addi sp, sp, -0x10",
        ],
    );
    assert!(out.contains("pc   0x800174dc <kmain>  mode S\n"), "{out}");
    // The backtrace: kvminit, called from kmain, called from the assembly that enters
    // S-mode (whose ra kmain saved; the chain ends there, s0 = 0).
    let bt: Vec<&str> = out
        .lines()
        .skip_while(|l| *l != "(merlin-dbg) bt")
        .skip(1)
        .take_while(|l| l.starts_with('#'))
        .collect();
    assert_eq!(bt.len(), 3, "{out}");
    assert!(bt[2].contains(" <supervisor_entry+"), "{out}");
    assert_eq!(bt[0], "#0  0x8003f660 <kvminit>");
    assert!(
        bt[1].starts_with("#1  0x8001") && bt[1].contains(" <kmain+"),
        "{out}"
    );
    let back = bt[1].trim_start_matches("#1  ");
    // finish comes back there, and says what kvminit returned.
    assert!(
        out.contains(&format!(
            "run till exit, back to {back}
=> {back}:"
        )),
        "{out}"
    );
    assert!(out.contains(&format!("=> {back}:")), "{out}");
    assert!(out.contains("returned: a0 = "), "{out}");
    assert_lines(
        &out,
        &[
            "Num  Type        Where",
            "1    breakpoint  0x800174dc <kmain>",
            "2    breakpoint  0x8003f660 <kvminit>",
        ],
    );
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
