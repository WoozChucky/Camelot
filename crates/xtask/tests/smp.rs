//! The Camelot kernel on several harts (issue #63, spec section 10, "Kernel
//! SMP"): it boots on 1 to 8 harts, every hart runs processes, CPU-bound
//! workers run side by side on different harts and get the right answers, and
//! the lock stress test of #61 holds up with four harts going through the locks
//! at once. These tests watch every hart's turn with `Cpu::step_traced`, whose
//! callback sees the hart about to run.

use camelot_fs::fsck;
use grail::cpu::{Cpu, Mode};
use grail::{DiskConfig, Machine, MachineConfig, RunResult};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::mpsc::Receiver;
use xtask::kernel;
use xtask::repo_root;

/// What the kernel prints before its shell starts on one hart, with
/// `xcal.dbg`'s pc masked (the same as kernel.rs's).
const BOOT_OUTPUT: &str = concat!(
    xtask::banner!(),
    "\
Camelot v0.1.0 booting...
kprintf: d=-42 u=4294967295 x=cafef00d p=0x80000000 s=excalibur c=K %=%
kprintf self-test: 26/26 passed
[dbg] pc=........ a0 = 0x00000000 (0)
ram: 4 MiB at 0x80000000
paging: on (Sv32)
vm self-test: 26/26 passed
timer: 10 Hz tick, 10000000 Hz timebase
disk: none
tick 1
tick 2
tick 3
tick 4
tick 5
tick 6
tick 7
tick 8
tick 9
tick 10
"
);

const SHELL_BANNER: &str = "Type help for the list of commands.\n";
const PROMPT: &str = "$ ";

/// mtime ticks between timer interrupts: the 10 MHz timebase / 10 Hz.
const TICK_INTERVAL: u64 = 1_000_000;

/// Enough steps for a boot and a few programs (kernel.rs's budget). Each step
/// runs every hart one instruction.
const RUN_MAX_STEPS: u64 = 150_000_000;

// Kernel layouts, from the headers named in each comment.

/// `sizeof(struct cpu)` (`CPU_SIZE` in kernel/include/spinlock.h): hart `h`'s
/// entry is at `cpus + h * CPU_SIZE`.
const CPU_SIZE: u32 = 88;
/// Offsets in `struct cpu`: the running process and the push_off() depth.
const CPU_PROC: u32 = 0;
const CPU_NOFF: u32 = 4;
/// Offset of the holding cpu in `struct spinlock`.
const LOCK_CPU: u32 = 4;
/// Offsets in `struct proc` (kernel/include/proc.h): its name (16 bytes) and
/// its parent.
const PROC_NAME: u32 = 8;
const PROC_PARENT: u32 = 24;

/// The kernel's spinlocks that are variables of their own (kernel.rs's list).
const NAMED_LOCKS: [&str; 9] = [
    "kmem_lock",
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

fn on_harts(harts: usize) -> MachineConfig {
    MachineConfig {
        harts,
        ..MachineConfig::default()
    }
}

/// Loads the committed kernel on a machine of `harts` harts, the UART reading
/// `input`.
fn load(harts: usize, input: Receiver<u8>) -> (Machine, grail::elf::Elf) {
    let file = committed_elf();
    let elf = grail::elf::parse(&file).unwrap();
    let mut m = Machine::new(on_harts(harts)).unwrap();
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

fn peek_byte(cpu: &Cpu, addr: u32) -> u8 {
    (cpu.bus.peek(addr & !3).unwrap() >> (8 * (addr & 3))) as u8
}

fn proc_name(cpu: &Cpu, p: u32) -> String {
    let bytes: Vec<u8> = (0..16)
        .map(|i| peek_byte(cpu, p + PROC_NAME + i))
        .take_while(|&b| b != 0)
        .collect();
    String::from_utf8(bytes).unwrap()
}

/// The process running on the hart whose turn it is (0 for none).
fn running(cpu: &Cpu, cpus: u32) -> u32 {
    cpu.bus.peek(cpus + cpu.id * CPU_SIZE + CPU_PROC).unwrap()
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
fn run_command(m: &mut Machine, tx: &std::sync::mpsc::Sender<u8>, line: &str, mut each: impl FnMut(&Cpu)) {
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

fn free_pages(m: &Machine, elf: &grail::elf::Elf) -> u32 {
    m.cpu.bus.peek(sym(elf, "nfree")).unwrap()
}

/// One line of the `harts` program: hart, pid (None while idle), ticks and
/// switches.
type HartLine = (u32, Option<u32>, u32, u32);

/// Parses the `harts` program's output (kernel/user/harts.c).
fn parse_harts(lines: &[&str]) -> Vec<HartLine> {
    assert_eq!(lines.first(), Some(&"HART   PID    TICKS SWITCHES"), "{lines:#?}");
    lines[1..]
        .iter()
        .map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            assert_eq!(f.len(), 4, "{l:?}");
            let pid = (f[1] != "-").then(|| f[1].parse().unwrap());
            (f[0].parse().unwrap(), pid, f[2].parse().unwrap(), f[3].parse().unwrap())
        })
        .collect()
}

/// Checks `harts`'s lines for a machine of `n` harts, `mtime` ticks after
/// its first tick: every hart listed once, in order, each taking timer
/// ticks; hart 0 has taken one every `TICK_INTERVAL` (give or take a few
/// that came while interrupts were off), and the others no more, as they
/// started later.
fn check_harts(lines: &[HartLine], n: u32, mtime: u64) {
    assert_eq!(lines.len() as u32, n, "{lines:?}");
    let expected = 1 + (mtime / TICK_INTERVAL) as u32;
    let hart0 = lines[0].2;
    assert!(
        hart0 <= expected && hart0 + 3 >= expected,
        "hart 0: {hart0} ticks in {mtime} after the first"
    );
    for (i, &(hart, _, ticks, _)) in lines.iter().enumerate() {
        assert_eq!(hart, i as u32, "{lines:?}");
        assert!(ticks > 0 && ticks <= hart0 + 1, "{lines:?}");
    }
    // harts itself runs on one of them (and the shell, which just started
    // it, may still be on another, on its way to wait for it).
    assert!(lines.iter().any(|l| l.1.is_some()), "{lines:?}");
}

#[test]
fn boots_on_every_hart_count() {
    // Each hart other than 0 flushes its TLB in tlb_serve() for the boot's
    // self-test, which unmaps a page of the kernel page table and so shoots
    // it down on every hart (smp.c).
    let serve = function("tlb_serve");
    for harts in [1, 2, 4, 8] {
        let (mut m, elf) = load(harts, kernel::typed(b"harts\nexit\n"));
        let sfence = sym(&elf, "sfence_vma");
        let mut flushed = HashSet::new();
        // mtime at the first tick (the boot's `tick 1`) and at each
        // prompt: the second comes right after harts's output.
        let (mut seen, mut first_tick, mut prompts) = (0, None, Vec::new());
        let r = m.run(Some(RUN_MAX_STEPS), |cpu| {
            if cpu.pc == sfence && serve.contains(&cpu.x[1]) {
                flushed.insert(cpu.id);
            }
            let out = &cpu.bus.uart.captured;
            if out.len() != seen {
                seen = out.len();
                if out.ends_with(PROMPT.as_bytes()) {
                    prompts.push(cpu.bus.clint.mtime);
                }
                if first_tick.is_none() && out.ends_with(b"tick 1\n") {
                    first_tick = Some(cpu.bus.clint.mtime);
                }
            }
        });
        let out = printed(&m);
        assert_eq!(r, RunResult::Exited(0), "{harts} harts:\n{out}");
        let at = out.find(SHELL_BANNER).unwrap_or_else(|| panic!("{out}"));
        let mut boot = BOOT_OUTPUT.to_string();
        if harts > 1 {
            boot += &format!("smp: {harts} harts running\n");
        }
        assert_eq!(kernel::mask_dbg_pc(&out[..at]), boot, "{harts} harts");
        assert_eq!(flushed, (1..harts as u32).collect::<HashSet<_>>(), "{harts} harts");
        let lines = parse_harts(&command_output(&out, "harts"));
        assert_eq!(prompts.len(), 2, "{harts} harts:\n{out}");
        check_harts(&lines, harts as u32, prompts[1] - first_tick.unwrap());
    }
}

/// The number of primes in `range`, by a sieve.
fn primes_in(range: Range<u32>) -> u32 {
    let mut composite = vec![false; range.end as usize];
    let mut count = 0;
    for n in 2..range.end {
        if composite[n as usize] {
            continue;
        }
        if range.contains(&n) {
            count += 1;
        }
        for k in (n as u64 * n as u64..range.end as u64).step_by(n as usize) {
            composite[k as usize] = true;
        }
    }
    count
}

#[test]
fn primes_workers_run_side_by_side_on_four_harts() {
    // The acceptance test of issue #63: four CPU-bound workers (each
    // `primes worker FROM TO`, a child of `primes`) on four harts. At some
    // point all four are in U-mode at once, one on each hart; every hart runs
    // a worker; and each worker's count, and the total, are right.
    const LINE: &str = "primes 4 40000";
    const LIMIT: u32 = 40000;
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load(4, rx);
    step_to_prompt(&mut m);
    let cpus = sym(&elf, "cpus");
    let mut on = [0u32; 4];
    let mut ran_on = HashSet::new();
    let mut workers = HashSet::new();
    let mut side_by_side = false;
    run_command(&mut m, &tx, LINE, |cpu| {
        let h = cpu.id as usize;
        on[h] = 0;
        if cpu.mode == Mode::U {
            let p = running(cpu, cpus);
            let parent = cpu.bus.peek(p + PROC_PARENT).unwrap();
            if proc_name(cpu, p) == "primes" && parent != 0 && proc_name(cpu, parent) == "primes" {
                on[h] = p;
                ran_on.insert(h);
                workers.insert(p);
            }
        }
        // The last hart's turn: every hart's state is from this step.
        if h == 3 && on.iter().all(|&p| p != 0) && on.iter().collect::<HashSet<_>>().len() == 4 {
            side_by_side = true;
        }
    });
    assert!(side_by_side, "the workers never ran on four harts at once");
    assert_eq!(ran_on.len(), 4, "workers ran on harts {ran_on:?}");
    assert_eq!(workers.len(), 4);
    let out = printed(&m);
    let lines = command_output(&out, LINE);
    assert_eq!(lines.len(), 5, "{lines:#?}");
    let mut slices: Vec<&str> = lines[..4].to_vec();
    slices.sort_by_key(|l| l[l.find('[').unwrap() + 1..l.find(',').unwrap()].parse::<u32>().unwrap());
    let slice = LIMIT / 4;
    for (w, line) in slices.iter().enumerate() {
        let (from, to) = (w as u32 * slice, (w as u32 + 1) * slice);
        assert_eq!(*line, format!("primes [{from}, {to}): {}", primes_in(from..to)));
    }
    assert_eq!(lines[4], format!("primes: {} below {LIMIT}, 4 workers", primes_in(0..LIMIT)));
    for b in b"exit\n" {
        tx.send(*b).unwrap();
    }
    assert_eq!(m.run(Some(RUN_MAX_STEPS), |_| {}), RunResult::Exited(0));
}

/// Watches every hart's locking state, as kernel.rs's LockWatch does hart
/// 0's: whenever a hart enters U-mode, it holds no spinlock (no named lock's
/// cpu is its own) and has no push_off() pending. Other harts may hold
/// locks meanwhile.
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

    /// Called before each hart's turn.
    fn step(&mut self, cpu: &Cpu) {
        let h = cpu.id as usize;
        let is_user = cpu.mode == Mode::U;
        if is_user && !self.was_user[h] {
            let me = self.cpus + cpu.id * CPU_SIZE;
            assert_eq!(
                cpu.bus.peek(me + CPU_NOFF),
                Some(0),
                "push_off() depth on hart {h} in U-mode at {:#x}",
                cpu.pc
            );
            for &(name, lock) in &self.locks {
                assert_ne!(
                    cpu.bus.peek(lock + LOCK_CPU),
                    Some(me),
                    "{name} held by hart {h} in U-mode at {:#x}",
                    cpu.pc
                );
            }
        }
        self.was_user[h] = is_user;
    }
}

/// Checks stress's output for `workers` workers and `rounds` rounds
/// (kernel.rs's check_stress_output).
fn check_stress_output(lines: &[&str], workers: u32, rounds: u32) {
    let mut next = vec![1; workers as usize + 1];
    for line in &lines[..lines.len() - 1] {
        let (w, r) = line
            .strip_prefix("stress ")
            .and_then(|l| l.split_once(": round "))
            .unwrap_or_else(|| panic!("{line:?} in {lines:#?}"));
        let (w, r): (usize, u32) = (w.parse().unwrap(), r.parse().unwrap());
        assert_eq!(r, next[w], "worker {w} in {lines:#?}");
        next[w] += 1;
    }
    assert!(next[1..].iter().all(|&n| n == rounds + 1), "{lines:#?}");
    assert_eq!(
        lines.last().copied(),
        Some(format!("stress: {workers} workers, {rounds} rounds, {} bytes: ok", workers * rounds * 200).as_str())
    );
}

#[test]
fn stress_holds_up_on_four_harts() {
    // Issue #61's stress program (six workers allocating and freeing
    // pages, printing and writing down one pipe) with four harts going
    // through the locks at once: every worker's output complete and in
    // order, every page back, no hart entering U-mode with a lock held, and
    // the stress processes spread over every hart.
    const LINE: &str = "stress 6 3";
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load(4, rx);
    step_to_prompt(&mut m);
    let before = free_pages(&m, &elf);
    let cpus = sym(&elf, "cpus");
    let mut locks = LockWatch::new(&elf);
    let mut ran_on = HashMap::new();
    run_command(&mut m, &tx, LINE, |cpu| {
        locks.step(cpu);
        if cpu.mode == Mode::U {
            let p = running(cpu, cpus);
            if proc_name(cpu, p) == "stress" {
                ran_on.entry(cpu.id).or_insert_with(HashSet::new).insert(p);
            }
        }
    });
    let out = printed(&m);
    check_stress_output(&command_output(&out, LINE), 6, 3);
    assert_eq!(free_pages(&m, &elf), before);
    assert_eq!(ran_on.len(), 4, "stress ran on harts {:?}", ran_on.keys());
    for b in b"exit\n" {
        tx.send(*b).unwrap();
    }
    assert_eq!(m.run(Some(RUN_MAX_STEPS), |_| {}), RunResult::Exited(0));
}

#[test]
fn systest_passes_on_four_harts() {
    let (mut m, _) = load(4, kernel::typed(b"systest\nexit\n"));
    let r = m.run(Some(2 * RUN_MAX_STEPS), |_| {});
    let out = printed(&m);
    assert_eq!(r, RunResult::Exited(0), "{out}");
    let lines = command_output(&out, "systest");
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(
        lines[0].starts_with("systest: ") && lines[0].ends_with(" checks, 0 failed"),
        "{lines:#?}"
    );
}

#[test]
fn the_disk_kernel_runs_programs_from_bin_on_four_harts() {
    // The root image, made bootable, boots the disk kernel on four harts;
    // its shell runs harts and stress from /bin, and the disk is consistent
    // afterwards.
    let image = xtask::fs::build_bootable(&repo_root(), xtask::fs::IMAGE_SIZE).unwrap();
    let mut m = Machine::new(MachineConfig {
        harts: 4,
        disks: vec![DiskConfig::memory(image)],
        ..MachineConfig::default()
    })
    .unwrap();
    m.boot_from_disk().unwrap();
    m.cpu.bus.uart.capture = true;
    m.set_uart_input(kernel::typed(b"harts\nstress 6 3\nexit\n"));
    let r = m.run(Some(3 * RUN_MAX_STEPS), |_| {});
    let out = printed(&m);
    assert_eq!(r, RunResult::Exited(0), "{out}");
    assert!(out.contains("\nsmp: 4 harts running\n"), "{out}");
    let lines = parse_harts(&command_output(&out, "harts"));
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert!(lines.iter().all(|l| l.2 > 0), "{lines:?}");
    check_stress_output(&command_output(&out, "stress 6 3"), 6, 3);
    let d = &mut m.cpu.bus.disks[0];
    let mut disk = vec![0; d.capacity() as usize * 512];
    d.backend().read(0, &mut disk).unwrap();
    let after = fsck::check(&disk).unwrap_or_else(|p| panic!("{p:#?}"));
    assert_eq!((after.replayed, after.orphans), (0, 0));
}
