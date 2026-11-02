//! Threads in the Camelot kernel (issue #64, spec section 10, "Kernel
//! threads"): a program whose threads add to shared counters under a mutex
//! and a spinlock gets the right totals on one hart and on four, with its
//! threads running side by side on different harts on the same page table;
//! threadtest's checks of the thread and futex calls (creating, ending and
//! joining threads, futex errors and wakes, and processes that end with
//! threads still running: exit, kill, exec, a fault) all pass on one hart
//! and on four. After each, every page and every thread slot is back (no
//! kernel stack leaks), no hart ever enters U-mode holding a lock, an ended
//! thread waiting to be joined holds no kernel stack, and a TLB shootdown of
//! a page table that threads run on several harts reaches every one of them
//! before it returns.
//!
//! The tests watch every hart's turn with `Cpu::step_traced`, as smp.rs's
//! do, and read the kernel's tables from memory (the layouts are in the
//! constants below, with the headers they come from).

use grail::cpu::{Cpu, Mode};
use grail::{Machine, MachineConfig, RunResult};
use std::collections::HashSet;
use std::ops::Range;
use std::sync::mpsc::{Receiver, Sender};
use xtask::repo_root;

const PROMPT: &str = "$ ";

/// Enough steps for a boot and a program (smp.rs's budget). Each step runs
/// every hart one instruction.
const RUN_MAX_STEPS: u64 = 150_000_000;

// Kernel layouts, from the headers named in each comment.

/// `sizeof(struct cpu)` and its fields (kernel/include/spinlock.h): the
/// running thread's process, the push_off() depth, the page table in satp
/// and the running thread.
const CPU_SIZE: u32 = 92;
const CPU_PROC: u32 = 0;
const CPU_NOFF: u32 = 4;
const CPU_PAGETABLE: u32 = 72;
const CPU_THREAD: u32 = 88;
/// Offset of the holding cpu in `struct spinlock`.
const LOCK_CPU: u32 = 4;
/// `struct thread` (kernel/include/proc.h): its size, and its state, tid,
/// process and kernel stack.
const THREAD_SIZE: u32 = 96;
const NTHREAD: u32 = 16;
const THREAD_STATE: u32 = 0;
const THREAD_TID: u32 = 4;
const THREAD_PROC: u32 = 8;
const THREAD_KSTACK: u32 = 12;
/// Thread states (kernel/include/proc.h).
const THREAD_UNUSED: u32 = 0;
const THREAD_ZOMBIE: u32 = 3;
/// `struct proc`: its size, state, name (16 bytes) and page table.
const PROC_SIZE: u32 = 164;
const NPROC: u32 = 8;
const PROC_STATE: u32 = 0;
const PROC_NAME: u32 = 8;
const PROC_PAGETABLE: u32 = 32;

/// The kernel's spinlocks that are variables of their own (kernel.rs's list).
const NAMED_LOCKS: [&str; 10] = [
    "kmem_lock",
    "uvm_lock",
    "proc_lock",
    "tickslock",
    "cons_lock",
    "uart_tx_lock",
    "ftable_lock",
    "bcache_lock",
    "itable_lock",
    "net_lock",
];

fn committed_elf() -> Vec<u8> {
    let path = repo_root().join("kernel/kernel.elf");
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Loads the committed kernel on a machine of `harts` harts, the UART reading
/// `input`.
fn load(harts: usize, input: Receiver<u8>) -> (Machine, grail::elf::Elf) {
    let file = committed_elf();
    let elf = grail::elf::parse(&file).unwrap();
    let mut m = Machine::new(MachineConfig {
        harts,
        ..MachineConfig::default()
    })
    .unwrap();
    m.cpu.bus.uart.capture = true;
    m.load_elf(&file).unwrap();
    m.set_uart_input(input);
    (m, elf)
}

fn printed(m: &Machine) -> String {
    String::from_utf8_lossy(&m.cpu.bus.uart.captured).into_owned()
}

fn sym(elf: &grail::elf::Elf, name: &str) -> u32 {
    elf.symbol(name).unwrap_or_else(|| panic!("no symbol {name}"))
}

/// The addresses of function `name` in the committed kernel.
fn function(name: &str) -> Range<u32> {
    let file = committed_elf();
    let s = elf32::Elf::parse(&file)
        .unwrap()
        .symbols()
        .unwrap()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no symbol {name}"));
    assert!(s.size > 0, "{name} has no size");
    s.value..s.value + s.size
}

fn peek(cpu: &Cpu, addr: u32) -> u32 {
    cpu.bus.peek(addr).unwrap()
}

fn proc_name(cpu: &Cpu, p: u32) -> String {
    let bytes: Vec<u8> = (0..16)
        .map(|i| (peek(cpu, (p + PROC_NAME + i) & !3) >> (8 * ((p + PROC_NAME + i) & 3))) as u8)
        .take_while(|&b| b != 0)
        .collect();
    String::from_utf8(bytes).unwrap()
}

/// Steps `m` until the shell has printed its first prompt.
fn step_to_prompt(m: &mut Machine) {
    for _ in 0..RUN_MAX_STEPS {
        if m.cpu.bus.uart.captured.ends_with(PROMPT.as_bytes()) {
            return;
        }
        m.cpu.step();
    }
    panic!("no prompt:\n{}", printed(m));
}

/// Types `line` and steps `m`, calling `each` before every hart's turn,
/// until the shell prompts again.
fn run_command(m: &mut Machine, tx: &Sender<u8>, line: &str, mut each: impl FnMut(&Cpu)) {
    for b in format!("{line}\n").bytes() {
        tx.send(b).unwrap();
    }
    let from = m.cpu.bus.uart.captured.len();
    let mut steps = 0;
    while !m.cpu.bus.uart.captured[from..].ends_with(PROMPT.as_bytes()) {
        assert!(steps < RUN_MAX_STEPS, "`{line}` never ended:\n{}", printed(m));
        m.cpu.step_traced(&mut each);
        steps += 1;
    }
}

/// The lines between the last echo of `command` and the next prompt.
fn command_output<'a>(out: &'a str, command: &str) -> Vec<&'a str> {
    let start = format!("{PROMPT}{command}\n");
    let at = out
        .rfind(&start)
        .unwrap_or_else(|| panic!("no `{command}` in:\n{out}"))
        + start.len();
    let rest = &out[at..];
    rest[..rest
        .find(PROMPT)
        .unwrap_or_else(|| panic!("`{command}` never ended:\n{out}"))]
        .lines()
        .collect()
}

/// Ends the shell and checks the machine stops with its status, 0.
fn exit_shell(m: &mut Machine, tx: &Sender<u8>) {
    for b in b"exit\n" {
        tx.send(*b).unwrap();
    }
    assert_eq!(
        m.run(Some(RUN_MAX_STEPS), |_| {}),
        RunResult::Exited(0),
        "{}",
        printed(m)
    );
}

fn free_pages(m: &Machine, elf: &grail::elf::Elf) -> u32 {
    m.cpu.bus.peek(sym(elf, "nfree")).unwrap()
}

/// The slots of the thread table in use, as (state, tid, process, kernel
/// stack).
fn threads_in_use(m: &Machine, elf: &grail::elf::Elf) -> Vec<(u32, u32, u32, u32)> {
    let table = sym(elf, "threads");
    (0..NTHREAD)
        .map(|i| table + i * THREAD_SIZE)
        .map(|t| {
            (
                peek(&m.cpu, t + THREAD_STATE),
                peek(&m.cpu, t + THREAD_TID),
                peek(&m.cpu, t + THREAD_PROC),
                peek(&m.cpu, t + THREAD_KSTACK),
            )
        })
        .filter(|&(state, ..)| state != THREAD_UNUSED)
        .collect()
}

/// The names of the processes in the table.
fn processes(m: &Machine, elf: &grail::elf::Elf) -> Vec<String> {
    let table = sym(elf, "procs");
    (0..NPROC)
        .map(|i| table + i * PROC_SIZE)
        .filter(|&p| peek(&m.cpu, p + PROC_STATE) != 0)
        .map(|p| proc_name(&m.cpu, p))
        .collect()
}

/// After a program has ended and been collected: the shell is the only
/// process, with its one thread, which has its kernel stack.
fn check_only_the_shell(m: &Machine, elf: &grail::elf::Elf) {
    assert_eq!(processes(m, elf), ["sh"]);
    let threads = threads_in_use(m, elf);
    assert_eq!(threads.len(), 1, "{threads:x?}");
    assert_ne!(threads[0].3, 0, "the shell's thread has no kernel stack");
}

/// Watches every hart's locking state (smp.rs's LockWatch): whenever a
/// hart enters U-mode, it holds no spinlock and has no push_off() pending.
struct LockWatch {
    cpus: u32,
    locks: Vec<(&'static str, u32)>,
    was_user: [bool; grail::MAX_HARTS],
}

impl LockWatch {
    fn new(elf: &grail::elf::Elf) -> LockWatch {
        LockWatch {
            cpus: sym(elf, "cpus"),
            locks: NAMED_LOCKS.iter().map(|&n| (n, sym(elf, n))).collect(),
            was_user: [false; grail::MAX_HARTS],
        }
    }

    fn step(&mut self, cpu: &Cpu) {
        let h = cpu.id as usize;
        let is_user = cpu.mode == Mode::U;
        if is_user && !self.was_user[h] {
            let me = self.cpus + cpu.id * CPU_SIZE;
            assert_eq!(
                cpu.bus.peek(me + CPU_NOFF),
                Some(0),
                "push_off() depth on hart {h} in U-mode"
            );
            for &(name, lock) in &self.locks {
                assert_ne!(
                    cpu.bus.peek(lock + LOCK_CPU),
                    Some(me),
                    "{name} held by hart {h} in U-mode"
                );
            }
        }
        self.was_user[h] = is_user;
    }
}

/// Watches every hart for threads of the process called `name` in U-mode:
/// whether two of its threads ever ran at the same time, on different
/// harts with the process's page table in satp.
struct SideBySide {
    cpus: u32,
    name: &'static str,
    harts: usize,
    /// Per hart, this step: the thread running in U-mode, and its process.
    on: [(u32, u32); grail::MAX_HARTS],
    /// The most of the process's threads seen in U-mode at once.
    most: usize,
}

impl SideBySide {
    fn new(elf: &grail::elf::Elf, name: &'static str, harts: usize) -> SideBySide {
        SideBySide {
            cpus: sym(elf, "cpus"),
            name,
            harts,
            on: [(0, 0); grail::MAX_HARTS],
            most: 0,
        }
    }

    fn step(&mut self, cpu: &Cpu) {
        let h = cpu.id as usize;
        let me = self.cpus + cpu.id * CPU_SIZE;
        self.on[h] = (0, 0);
        if cpu.mode == Mode::U {
            let p = peek(cpu, me + CPU_PROC);
            if p != 0 && proc_name(cpu, p) == self.name {
                // The hart runs on the process's own page table.
                assert_eq!(peek(cpu, me + CPU_PAGETABLE), peek(cpu, p + PROC_PAGETABLE));
                self.on[h] = (peek(cpu, me + CPU_THREAD), p);
            }
        }
        // The last hart's turn: every hart's state is from this step.
        if h == self.harts - 1 {
            let running: Vec<(u32, u32)> = self.on[..self.harts]
                .iter()
                .copied()
                .filter(|t| t.0 != 0)
                .collect();
            let threads: HashSet<u32> = running.iter().map(|t| t.0).collect();
            let procs: HashSet<u32> = running.iter().map(|t| t.1).collect();
            if procs.len() == 1 && threads.len() == running.len() {
                self.most = self.most.max(threads.len());
            }
        }
    }
}

/// Watches the TLB shootdowns of user page tables (smp.c): for each call of
/// tlb_shootdown() on a page table other than the kernel's, the harts its
/// IPI asks to flush, and which of them did (in tlb_serve()) before the call
/// returned. Every one of them must have; `shared` counts the calls that
/// reached another hart.
struct Shootdowns {
    entry: u32,
    body: Range<u32>,
    ipi: u32,
    sfence: u32,
    serve: Range<u32>,
    kernel_pt: u32,
    /// Per requesting hart: where the call returns to, its sp, the harts
    /// the IPI went to and those that have flushed since.
    active: [Option<(u32, u32, u32, u32)>; grail::MAX_HARTS],
    shared: u32,
}

impl Shootdowns {
    fn new(elf: &grail::elf::Elf, m: &Machine) -> Shootdowns {
        Shootdowns {
            entry: sym(elf, "tlb_shootdown"),
            body: function("tlb_shootdown"),
            ipi: sym(elf, "machine_ipi"),
            sfence: sym(elf, "sfence_vma"),
            serve: function("tlb_serve"),
            kernel_pt: m.cpu.bus.peek(sym(elf, "kernel_pagetable")).unwrap(),
            active: [None; grail::MAX_HARTS],
            shared: 0,
        }
    }

    fn step(&mut self, cpu: &Cpu) {
        let h = cpu.id as usize;
        if cpu.pc == self.entry && cpu.x[10] != self.kernel_pt {
            self.active[h] = Some((cpu.x[1], cpu.x[2], 0, 0));
        }
        if let Some((ra, sp, targets, flushed)) = self.active[h] {
            if cpu.pc == self.ipi && self.body.contains(&cpu.x[1]) {
                self.active[h] = Some((ra, sp, cpu.x[10], flushed));
            } else if cpu.pc == ra && cpu.x[2] == sp {
                assert_eq!(
                    targets & flushed,
                    targets,
                    "hart {h}'s shootdown returned before harts {:#x} flushed",
                    targets & !flushed
                );
                if targets != 0 {
                    self.shared += 1;
                }
                self.active[h] = None;
            }
        }
        // A flush for another hart's request: any from when the request
        // began, as a target may serve its ticket before the IPI goes out
        // (another request's IPI woke it).
        if cpu.pc == self.sfence && self.serve.contains(&cpu.x[1]) {
            for (k, a) in self.active.iter_mut().enumerate() {
                if let Some((_, _, _, flushed)) = a {
                    if k != h {
                        *flushed |= 1 << h;
                    }
                }
            }
        }
    }
}

/// Watches thread_join() (proc.c) free the slot of the thread it joined,
/// under proc_lock: the thread has ended, and holds no kernel stack any
/// more (the scheduler freed it as the thread switched away for the last
/// time). `joined` counts them.
struct Joins {
    free: u32,
    join: Range<u32>,
    joined: u32,
}

impl Joins {
    fn new(elf: &grail::elf::Elf) -> Joins {
        Joins {
            free: sym(elf, "thread_free"),
            join: function("thread_join"),
            joined: 0,
        }
    }

    fn step(&mut self, cpu: &Cpu) {
        if cpu.pc != self.free || !self.join.contains(&cpu.x[1]) {
            return;
        }
        let t = cpu.x[10];
        assert_eq!(peek(cpu, t + THREAD_STATE), THREAD_ZOMBIE);
        assert_eq!(
            peek(cpu, t + THREAD_KSTACK),
            0,
            "an ended thread kept its kernel stack"
        );
        self.joined += 1;
    }
}

#[test]
fn threads_add_up_under_a_mutex_on_one_and_four_harts() {
    // Issue #64's first criterion: a program starts several threads that
    // update shared data under a mutex and joins them; the result is right
    // with 1 hart and with several. On 4 the threads run side by side, on the
    // process's one page table.
    const LINE: &str = "threads 4 300";
    for harts in [1, 4] {
        let (tx, rx) = std::sync::mpsc::channel();
        let (mut m, elf) = load(harts, rx);
        step_to_prompt(&mut m);
        let before = free_pages(&m, &elf);
        check_only_the_shell(&m, &elf);
        let mut locks = LockWatch::new(&elf);
        let mut side = SideBySide::new(&elf, "threads", harts);
        run_command(&mut m, &tx, LINE, |cpu| {
            locks.step(cpu);
            side.step(cpu);
        });
        let out = printed(&m);
        assert_eq!(
            command_output(&out, LINE),
            ["threads: 4 threads, 300 rounds: counter 1200, spun 1200: ok"],
            "{harts} harts"
        );
        assert_eq!(free_pages(&m, &elf), before, "{harts} harts: pages leaked");
        check_only_the_shell(&m, &elf);
        if harts == 1 {
            assert_eq!(side.most, 1);
        } else {
            assert!(side.most >= 2, "the threads never ran side by side");
        }
        exit_shell(&mut m, &tx);
    }
}

#[test]
fn threadtest_passes_on_one_and_four_harts() {
    // Issue #64's second criterion, and the futex calls: threadtest's
    // checks (thread exit, join on an ended thread, exit, kill, exec and a
    // fault with threads still running, futex edge cases) pass on 1 hart and
    // on 4. Its child that faults is reported as any killed process is.
    // Afterwards every page and thread slot is back; joining an ended
    // thread finds it without a kernel stack; and on 4 harts the shootdowns
    // of sbrk() from two threads at once reach the other hart running the
    // process before they return.
    for harts in [1, 4] {
        let (tx, rx) = std::sync::mpsc::channel();
        let (mut m, elf) = load(harts, rx);
        step_to_prompt(&mut m);
        let before = free_pages(&m, &elf);
        let mut locks = LockWatch::new(&elf);
        let mut shootdowns = Shootdowns::new(&elf, &m);
        let mut joins = Joins::new(&elf);
        run_command(&mut m, &tx, "threadtest", |cpu| {
            locks.step(cpu);
            shootdowns.step(cpu);
            joins.step(cpu);
        });
        let out = printed(&m);
        let lines = command_output(&out, "threadtest");
        assert_eq!(lines.len(), 4, "{harts} harts: {lines:#?}");
        assert!(
            lines[0].starts_with("pid ") && lines[0].ends_with(" (threadtest) killed: store page fault"),
            "{lines:#?}"
        );
        assert!(lines[1].starts_with("  scause 0x0000000f  sepc 0x"), "{lines:#?}");
        assert!(lines[1].ends_with("  stval 0x00000000"), "{lines:#?}");
        assert_eq!(lines[2], "  va 0x00000000 is not mapped");
        assert!(
            lines[3].starts_with("threadtest: ") && lines[3].ends_with(" checks, 0 failed"),
            "{harts} harts: {lines:#?}"
        );
        assert_eq!(free_pages(&m, &elf), before, "{harts} harts: pages leaked");
        check_only_the_shell(&m, &elf);
        assert!(joins.joined > 30, "{} joins", joins.joined);
        if harts > 1 {
            assert!(
                shootdowns.shared > 0,
                "no shootdown reached another hart running the process"
            );
        }
        exit_shell(&mut m, &tx);
    }
}
