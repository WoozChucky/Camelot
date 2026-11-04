//! The debug interface (`grail::debug`) on real programs: breakpoints, single steps
//! and watchpoints stop where they should, multi-hart machines stop between harts'
//! turns, and a debugged run executes exactly the instructions of an undebugged one.

use grail::cpu::Cpu;
use grail::debug::{Space, Stop, WatchKind};
use grail::{Machine, MachineConfig};
use std::hash::{DefaultHasher, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex};

fn root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn read(path: &str) -> Vec<u8> {
    std::fs::read(root().join(path)).unwrap()
}

fn symbol(elf: &[u8], name: &str) -> u64 {
    let elf = elf::Elf::parse(elf).unwrap();
    let symbols = elf.symbols().unwrap();
    symbols
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no symbol {name}"))
        .value
}

/// A shared buffer the UART writes to.
#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Output {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Output {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// `elf` loaded on a machine of `harts` harts, `ram` MiB, its UART input already all
/// there (so when the program reads it does not depend on timing).
fn machine(elf: &[u8], harts: usize, ram: usize, input: &[u8]) -> (Machine, Output) {
    let config = MachineConfig {
        harts,
        ..MachineConfig::with_ram(ram << 20)
    };
    let mut m = Machine::new(config).unwrap();
    m.load_elf(elf).unwrap();
    let out = Output::default();
    m.set_uart_output(out.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    for &b in input {
        tx.send(b).unwrap();
    }
    m.set_uart_input(rx);
    (m, out)
}

/// Hashes every turn: which hart, its pc, its mode, and mtime.
struct Turns(DefaultHasher, u64);

impl Turns {
    fn new() -> Self {
        Turns(DefaultHasher::new(), 0)
    }
    fn trace(&mut self) -> impl FnMut(&Cpu) + '_ {
        |cpu| {
            self.0.write_u32(cpu.id);
            self.0.write_u32(cpu.pc as u32);
            self.0.write_u8(cpu.mode as u8);
            self.0.write_u64(cpu.bus.clint.mtime);
            self.1 += 1;
        }
    }
    fn digest(&self) -> (u64, u64) {
        (self.0.finish(), self.1)
    }
}

/// Runs `m` to the end without a debugger. Returns the exit code and the turns.
fn undebugged(m: &mut Machine) -> (u32, (u64, u64)) {
    let mut turns = Turns::new();
    let r = m.run(Some(1_000_000_000), turns.trace());
    let grail::RunResult::Exited(code) = r else {
        panic!("{r:?}")
    };
    (code, turns.digest())
}

#[test]
fn breakpoint_step_and_continue_on_hello() {
    let elf = read("examples/hello/hello.elf");
    let puts = symbol(&elf, "puts");
    let (mut m, out) = machine(&elf, 1, 4, b"");
    let mut trace = |_: &Cpu| {};
    assert!(m.set_breakpoint(puts));
    assert!(!m.set_breakpoint(puts), "already set");
    assert_eq!(
        m.debug_continue(u64::MAX, &mut trace),
        Some(Stop::Breakpoint { hart: 0, pc: puts })
    );
    assert_eq!(m.pc(), puts);
    // _start called puts with a0 = message and ra = the instruction after the call.
    assert_eq!(m.reg(10), symbol(&elf, "message"));
    let mut text = [0u8; 5];
    assert_eq!(m.read_memory(m.reg(10), &mut text, Space::Virtual), 5);
    assert_eq!(&text, b"Hello");
    assert_eq!(out.text(), "", "nothing printed yet");

    // Steps go one instruction at a time, from the breakpoint on.
    assert_eq!(m.debug_step(&mut trace), Stop::Stepped { hart: 0 });
    assert_eq!(m.pc(), puts + 4);
    assert_eq!(m.reg(5), 0x1000_0000, "li t0, UART_BASE");
    assert_eq!(m.debug_step(&mut trace), Stop::Stepped { hart: 0 });
    assert_eq!(m.pc(), puts + 8);

    // Continuing runs to the end, the breakpoint is not hit again.
    assert_eq!(m.debug_continue(u64::MAX, &mut trace), Some(Stop::Exited(0)));
    assert_eq!(out.text(), "Hello from Camelot!\n");
    assert_eq!(m.debug_continue(u64::MAX, &mut trace), Some(Stop::Exited(0)));
    assert_eq!(m.debug_step(&mut trace), Stop::Exited(0));
}

#[test]
fn a_budget_returns_control_and_a_resume_steps_over_the_breakpoint() {
    let elf = read("examples/hello/hello.elf");
    let (mut m, out) = machine(&elf, 1, 4, b"");
    let mut trace = |_: &Cpu| {};
    // `mv t1, a0`, before the loop: hit once.
    let loop_top = symbol(&elf, "puts") + 4;
    m.set_breakpoint(loop_top);
    assert_eq!(m.debug_continue(3, &mut trace), None, "3 steps are not enough");
    let mut hits = 0;
    while let Some(Stop::Breakpoint { pc, .. }) = m.debug_continue(u64::MAX, &mut trace) {
        assert_eq!(pc, loop_top);
        hits += 1;
    }
    assert_eq!(hits, 1, "mv t1, a0 runs once");
    // A breakpoint on the loop's `lbu`: once per byte printed and once for the NUL,
    // then an exit.
    let (mut m, _) = machine(&elf, 1, 4, b"");
    let lbu = symbol(&elf, "puts") + 8;
    m.set_breakpoint(lbu);
    let mut hits = 0;
    let stop = loop {
        match m.debug_continue(u64::MAX, &mut trace) {
            Some(Stop::Breakpoint { .. }) => hits += 1,
            other => break other,
        }
    };
    assert_eq!(stop, Some(Stop::Exited(0)));
    assert_eq!(hits, "Hello from Camelot!\n".len() + 1, "every byte and the NUL");
    assert!(m.clear_breakpoint(lbu));
    assert!(!m.clear_breakpoint(lbu));
    let _ = out;
}

#[test]
fn watchpoints_stop_after_the_access() {
    let elf = read("examples/hello/hello.elf");
    let message = symbol(&elf, "message");
    let mut trace = |_: &Cpu| {};

    // A read of the message's third byte, by `lbu t2, 0(t1)`.
    let (mut m, _) = machine(&elf, 1, 4, b"");
    m.set_watchpoint(message + 2, 1, WatchKind::Read).unwrap();
    let Some(Stop::Watchpoint(hit)) = m.debug_continue(u64::MAX, &mut trace) else {
        panic!()
    };
    assert_eq!((hit.hart, hit.kind, hit.addr), (0, WatchKind::Read, message + 2));
    assert_eq!(m.reg(7), u64::from(b'l'), "the load completed: t2 = 'l'");
    assert_eq!(m.pc(), symbol(&elf, "puts") + 12, "pc after the load");
    assert!(m.clear_watchpoint(message + 2, 1, WatchKind::Read));

    // Writes to the UART's THR, a device register: every byte printed.
    let (mut m, out) = machine(&elf, 1, 4, b"");
    m.set_watchpoint(0x1000_0000, 1, WatchKind::Write).unwrap();
    // A read watchpoint on the same register is never hit: puts only writes it.
    m.set_watchpoint(0x1000_0000, 1, WatchKind::Read).unwrap();
    let mut writes = 0;
    while let Some(Stop::Watchpoint(hit)) = m.debug_continue(u64::MAX, &mut trace) {
        assert_eq!(hit.kind, WatchKind::Write);
        writes += 1;
        assert_eq!(out.text().len(), writes, "stopped right after each byte");
    }
    assert_eq!(writes, 20);
    // An access watchpoint matches reads too: the line status register, read before
    // each byte.
    let (mut m, _) = machine(&elf, 1, 4, b"");
    m.set_watchpoint(0x1000_0005, 1, WatchKind::Access).unwrap();
    let Some(Stop::Watchpoint(hit)) = m.debug_continue(u64::MAX, &mut trace) else {
        panic!()
    };
    assert_eq!((hit.kind, hit.addr), (WatchKind::Access, 0x1000_0005));
    assert!(m.set_watchpoint(0, 0, WatchKind::Write).is_err());
    assert!(m.set_watchpoint(0xffff_fffc, 8, WatchKind::Write).is_err());
}

/// Debugs `elf` with breakpoints at `stops`, stepping `steps` instructions at each
/// stop, and checks the run is the one an undebugged machine makes: the same turns,
/// output and exit code.
fn same_run_debugged(elf: &[u8], harts: usize, ram: usize, input: &[u8], stops: &[u64], steps: usize) {
    let (mut plain, plain_out) = machine(elf, harts, ram, input);
    let (code, expected) = undebugged(&mut plain);

    let (mut m, out) = machine(elf, harts, ram, input);
    let mut turns = Turns::new();
    for &pc in stops {
        m.set_breakpoint(pc);
    }
    let mut stopped = 0;
    let mut trace = turns.trace();
    let end = loop {
        match m.debug_continue(50_000, &mut trace) {
            None => continue,
            Some(Stop::Exited(code)) => break code,
            Some(_) => {
                stopped += 1;
                for _ in 0..steps {
                    let _ = m.debug_step(&mut trace);
                }
            }
        }
    };
    drop(trace);
    assert!(stopped > 0, "no breakpoint was hit");
    assert_eq!(end, code);
    assert_eq!(out.text(), plain_out.text());
    assert_eq!(turns.digest(), expected, "a different run");
    for h in 0..harts {
        assert_eq!(m.cpu.hart(h).csr.minstret, plain.cpu.hart(h).csr.minstret);
        assert_eq!(m.cpu.hart(h).x, plain.cpu.hart(h).x);
    }
    assert_eq!(m.cpu.bus.clint.mtime, plain.cpu.bus.clint.mtime);
}

#[test]
fn a_debugged_run_is_the_undebugged_run() {
    let hello = read("examples/hello/hello.elf");
    same_run_debugged(&hello, 1, 4, b"", &[symbol(&hello, "puts") + 8], 3);
}

#[test]
fn a_debugged_kernel_runs_as_it_would() {
    let kernel = read("kernel/kernel.elf");
    let stops = [symbol(&kernel, "kmain"), symbol(&kernel, "timer_init")];
    same_run_debugged(&kernel, 1, 4, b"hello\nexit\n", &stops, 5);
}

#[test]
fn harts_stop_between_turns() {
    let smp = read("examples/smp/smp.elf");
    let work = symbol(&smp, "work");
    // Every hart works: hart 0 once it has woken the others, hart 1 once woken.
    let (mut m, _) = machine(&smp, 2, 4, b"");
    let mut trace = |_: &Cpu| {};
    m.set_breakpoint(work);
    assert_eq!(
        m.debug_continue(u64::MAX, &mut trace),
        Some(Stop::Breakpoint { hart: 0, pc: work })
    );
    assert_eq!(m.cpu.next_hart(), 0);
    assert_eq!(
        m.debug_continue(u64::MAX, &mut trace),
        Some(Stop::Breakpoint { hart: 1, pc: work })
    );
    // Stopped before hart 1's turn: hart 0 has had its turn of this step.
    assert_eq!(m.cpu.next_hart(), 1);
    m.select_hart(1).unwrap();
    assert_eq!(m.pc(), work);
    assert_eq!(m.reg(8), 1, "s0 = hart id");
    assert_eq!(m.csr(0xf14), Some(1), "mhartid");
    m.select_hart(0).unwrap();
    assert_eq!(m.csr(0xf14), Some(0));
    let hart0 = m.pc();

    // Stepping hart 1 runs its turn, which ends the step.
    m.select_hart(1).unwrap();
    assert_eq!(m.debug_step(&mut trace), Stop::Stepped { hart: 1 });
    assert_eq!(m.pc(), work + 4);
    assert_eq!(m.cpu.next_hart(), 0);
    assert_eq!(m.cpu.hart(0).pc, hart0, "hart 0 has not run");
    let _ = hart0;
    // Stepping hart 1 again: hart 0 takes its turn first, then hart 1 its.
    m.debug_step(&mut trace);
    assert_eq!(m.pc(), work + 8);
    // Stepping hart 0 runs only its turn.
    m.select_hart(0).unwrap();
    m.debug_step(&mut trace);
    assert_eq!(m.cpu.next_hart(), 1);
    assert_eq!(m.cpu.hart(1).pc, work + 8);
    assert!(m.select_hart(2).is_err());

    // Watching the shared counter: each stop is one hart's amoadd.w, adding 1.
    m.clear_breakpoint(work);
    let counter = symbol(&smp, "counter");
    m.set_watchpoint(counter, 4, WatchKind::Write).unwrap();
    let read_counter = |m: &Machine| {
        let mut word = [0; 4];
        m.read_memory(counter, &mut word, Space::Physical);
        u32::from_le_bytes(word)
    };
    let mut last = None;
    let mut harts = [false; 2];
    for _ in 0..10 {
        let Some(Stop::Watchpoint(hit)) = m.debug_continue(u64::MAX, &mut trace) else {
            panic!()
        };
        assert_eq!((hit.addr, hit.kind), (counter, WatchKind::Write));
        harts[hit.hart] = true;
        let now = read_counter(&m);
        if let Some(last) = last {
            assert_eq!(now, last + 1);
        }
        last = Some(now);
    }
    assert_eq!(harts, [true, true], "both harts add");
    m.clear_debug_points();
    assert_eq!(m.debug_continue(u64::MAX, &mut trace), Some(Stop::Exited(0)));
}

#[test]
fn a_debugged_smp_run_is_the_undebugged_run() {
    let smp = read("examples/smp/smp.elf");
    let stops = [symbol(&smp, "work"), symbol(&smp, "ipi_handler")];
    same_run_debugged(&smp, 2, 4, b"", &stops, 7);
    same_run_debugged(&smp, 3, 4, b"", &stops, 2);
}
