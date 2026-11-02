//! The Camelot kernel on several harts (issue #63, spec section 10, "Kernel
//! SMP"): it boots on 1 to 8 harts, every hart runs processes, CPU-bound
//! workers run side by side on different harts and get the right answers, and
//! the lock stress test of #61 holds up with four harts going through the locks
//! at once; TLB shootdowns wait for every hart they must reach, and a panic on
//! one hart stops the others' output. These tests watch every hart's turn with
//! `Cpu::step_traced`, whose callback sees the hart about to run, and some
//! change a hart between steps (its pc, to make it fault, skip a call or make
//! one).

mod common;

use camelot_fs::fsck;
use common::*;
use grail::cpu::{Cpu, Mode};
use grail::{DiskConfig, Machine, MachineConfig, RunResult};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
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

/// mtime ticks between timer interrupts: the 10 MHz timebase / 10 Hz.
const TICK_INTERVAL: u64 = 1_000_000;

/// The process running on the hart whose turn it is (0 for none).
fn running(cpu: &Cpu, cpus: u32) -> u32 {
    cpu.bus.peek(cpus + cpu.id * CPU_SIZE + CPU_PROC).unwrap()
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
            (
                f[0].parse().unwrap(),
                pid,
                f[2].parse().unwrap(),
                f[3].parse().unwrap(),
            )
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

/// Whether the hart whose turn it is is about to flush its TLB for another
/// hart's shootdown: at `sfence_vma`, called from tlb_serve().
fn serving(cpu: &Cpu, sfence: u32, serve: &Range<u32>) -> bool {
    cpu.pc == sfence && serve.contains(&cpu.x[1])
}

#[test]
fn boots_on_every_hart_count() {
    // Each hart other than 0 flushes its TLB in tlb_serve() for the boot's
    // self-test, which unmaps a page of the kernel page table and so shoots
    // it down on every hart (smp.c), and has done so by the time the
    // self-test frees the page: the shootdown waits for every hart.
    let serve = function("tlb_serve");
    let selftest = function("tlb_selftest");
    for harts in [1, 2, 4, 8] {
        let (mut m, elf) = load(harts, kernel::typed(b"harts\nexit\n"));
        let sfence = sym(&elf, "sfence_vma");
        let kfree = sym(&elf, "kfree");
        let ticks = sym(&elf, "ticks");
        let mut flushed = HashSet::new();
        // The harts that had flushed when hart 0 freed the test page.
        let mut flushed_at_free = None;
        // mtime at the first tick (the boot's `tick 1`) and at each
        // prompt (the second comes right after harts's output), and the
        // clock, `ticks`, at each prompt.
        let (mut seen, mut first_tick, mut prompts, mut clock) = (0, None, Vec::new(), Vec::new());
        let r = m.run(Some(RUN_MAX_STEPS), |cpu| {
            if serving(cpu, sfence, &serve) {
                flushed.insert(cpu.id);
            }
            if cpu.id == 0 && cpu.pc == kfree && selftest.contains(&cpu.x[1]) {
                assert!(flushed_at_free.is_none(), "the self-test freed twice");
                flushed_at_free = Some(flushed.clone());
            }
            let out = &cpu.bus.uart.captured;
            if out.len() != seen {
                seen = out.len();
                if out.ends_with(PROMPT.as_bytes()) {
                    prompts.push(cpu.bus.clint.mtime);
                    clock.push(cpu.bus.peek(ticks).unwrap());
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
        let others: HashSet<u32> = (1..harts as u32).collect();
        assert_eq!(flushed, others, "{harts} harts");
        if harts > 1 {
            assert_eq!(flushed_at_free.as_ref(), Some(&others), "{harts} harts");
        }
        let lines = parse_harts(&command_output(&out, "harts"));
        assert_eq!(prompts.len(), 2, "{harts} harts:\n{out}");
        let since_first = prompts[1] - first_tick.unwrap();
        check_harts(&lines, harts as u32, since_first);
        // The clock that sleep and uptime read counts hart 0's ticks alone:
        // ten a second however many harts tick (a few fewer if some came
        // while interrupts were off), not one per tick of every hart.
        let expected = 1 + (since_first / TICK_INTERVAL) as u32;
        assert!(
            clock[1] <= expected && clock[1] + 3 >= expected,
            "{harts} harts: the clock counted {} ticks in {since_first} after the first",
            clock[1]
        );
    }
}

/// The value of the kernel's variable `name`.
fn peek_sym(m: &Machine, elf: &grail::elf::Elf, name: &str) -> u32 {
    m.cpu.bus.peek(sym(elf, name)).unwrap()
}

/// Steps `m` until `stop` says so, at most `max` steps; between steps, so
/// `stop` may change any hart. Calls `each` before every hart's turn.
fn step_until(
    m: &mut Machine,
    max: u64,
    mut each: impl FnMut(&Cpu),
    mut stop: impl FnMut(&mut Machine) -> bool,
) -> bool {
    for _ in 0..max {
        if stop(m) {
            return true;
        }
        if m.cpu.bus.exit.is_some() {
            return false;
        }
        m.cpu.step_traced(&mut each);
    }
    false
}

#[test]
fn a_shootdown_waits_for_a_hart_slow_to_flush() {
    // The boot's self-test shoots a kernel mapping down on every hart. Hart
    // 2 skips the IPI that asks it to flush (the test returns from
    // tlb_serve() at its entry), so it flushes only at its next timer tick:
    // hart 0 must still be waiting then, and free the page only after it,
    // with no hart reported as missing the shootdown.
    let (mut m, elf) = load(4, kernel::typed(b"exit\n"));
    let serve = function("tlb_serve");
    let selftest = function("tlb_selftest");
    let sfence = sym(&elf, "sfence_vma");
    let kfree = sym(&elf, "kfree");
    // Hart 2's request counts.
    let (asked, done) = (sym(&elf, "tlb_asked") + 8, sym(&elf, "tlb_done") + 8);
    let mut skipped = false;
    // The steps at which hart 2 flushed for the shootdown and hart 0 freed
    // the page.
    let (mut step, mut flushed) = (0u64, None);
    let freed = Cell::new(None);
    let ended = step_until(
        &mut m,
        RUN_MAX_STEPS,
        |cpu| {
            if cpu.id == 0 {
                step += 1;
            }
            if cpu.id == 2 && flushed.is_none() && serving(cpu, sfence, &serve) {
                flushed = Some(step);
            }
            if cpu.id == 0 && cpu.pc == kfree && selftest.contains(&cpu.x[1]) {
                freed.set(Some(step));
            }
        },
        |m| {
            let hart = m.cpu.hart(2);
            if !skipped && hart.pc == serve.start && m.cpu.bus.peek(asked) != m.cpu.bus.peek(done) {
                skipped = true;
                let ra = hart.x[1];
                m.cpu.hart_mut(2).pc = ra;
            }
            freed.get().is_some()
        },
    );
    assert!(ended, "hart 0 never freed the page:\n{}", printed(&m));
    assert!(skipped);
    let (flushed, freed) = (flushed.unwrap(), freed.get().unwrap());
    assert!(
        freed > flushed,
        "freed at step {freed}, hart 2 flushed at {flushed}"
    );
    assert_eq!(m.run(Some(RUN_MAX_STEPS), |_| {}), RunResult::Exited(0));
    let out = printed(&m);
    assert!(out.contains("\nsmp: 4 harts running\n"), "{out}");
    assert!(!out.contains("missed"), "{out}");
}

#[test]
fn a_kernel_shootdown_reaches_harts_running_processes() {
    // The kernel page table's second-level tables are in every process's
    // page table, so a shootdown of the kernel table must reach every hart,
    // whatever its satp. While `primes 4 40000` has workers in U-mode on
    // harts 1-3, each with its process's page table in satp, the test has
    // hart 0 call tlb_shootdown(kernel_pagetable) as it enters user_trap
    // (where only a0 and ra are live, and interrupts are off): each of harts
    // 1-3 must flush in tlb_serve() before the call returns. The workers'
    // counts are still right afterwards.
    const LINE: &str = "primes 4 40000";
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load(4, rx);
    step_to_prompt(&mut m);
    for b in format!("{LINE}\nexit\n").bytes() {
        tx.send(b).unwrap();
    }
    let kernel_pt = peek_sym(&m, &elf, "kernel_pagetable");
    let cpus = sym(&elf, "cpus");
    let user_trap = sym(&elf, "user_trap");
    let shootdown = sym(&elf, "tlb_shootdown");
    let serve = function("tlb_serve");
    let sfence = sym(&elf, "sfence_vma");
    // Hart 0's registers to put back once the call returns, while it runs.
    let mut saved: Option<[u32; 32]> = None;
    let calling = Cell::new(false);
    let mut flushed = HashSet::new();
    let returned = step_until(
        &mut m,
        RUN_MAX_STEPS,
        |cpu| {
            if calling.get() && cpu.id != 0 && serving(cpu, sfence, &serve) {
                flushed.insert(cpu.id);
            }
        },
        |m| {
            let hart0 = m.cpu.hart(0);
            if hart0.pc != user_trap {
                return false;
            }
            match saved {
                None => {
                    let busy = (1..4).all(|h| {
                        let satp = m.cpu.bus.peek(cpus + h * CPU_SIZE + CPU_PAGETABLE).unwrap();
                        m.cpu.hart(h as usize).mode == Mode::U && satp != kernel_pt && satp != 0
                    });
                    if busy {
                        saved = Some(hart0.x);
                        let h = m.cpu.hart_mut(0);
                        h.x[10] = kernel_pt;
                        h.x[1] = user_trap;
                        h.pc = shootdown;
                        calling.set(true);
                    }
                    false
                }
                // Back from the call: user_trap goes on as if nothing had
                // happened.
                Some(regs) if hart0.x[2] == regs[2] => {
                    m.cpu.hart_mut(0).x = regs;
                    calling.set(false);
                    true
                }
                Some(_) => false,
            }
        },
    );
    assert!(returned, "hart 0's call never returned:\n{}", printed(&m));
    assert_eq!(flushed, HashSet::from([1, 2, 3]));
    assert_eq!(m.run(Some(RUN_MAX_STEPS), |_| {}), RunResult::Exited(0));
    let out = printed(&m);
    let lines = command_output(&out, LINE);
    assert_eq!(
        lines.last().copied(),
        Some(format!("primes: {} below 40000, 4 workers", primes_in(0..40000)).as_str()),
        "{lines:#?}"
    );
}

/// sstatus bits (spec section 3): SIE, SPIE, SPP, and the mask of the
/// mstatus bits sstatus shows (SIE, SPIE, SPP, SUM, MXR).
const SSTATUS_SIE: u32 = 1 << 1;
const SSTATUS_SPIE: u32 = 1 << 5;
const SSTATUS_SPP: u32 = 1 << 8;
const SSTATUS_MASK: u32 = SSTATUS_SIE | SSTATUS_SPIE | SSTATUS_SPP | (1 << 18) | (1 << 19);

/// Exit code of a kernel panic (`PANIC_EXIT_CODE` in kernel/include/trap.h).
const PANIC_EXIT: u32 = 0xfe;

/// Where a hart stopped for good by panic_freeze() may be: in it, or in the
/// wfi() it calls.
struct Freeze {
    body: Range<u32>,
    wfi: Range<u32>,
}

impl Freeze {
    fn new(elf: &grail::elf::Elf) -> Freeze {
        // wfi() (riscv.s, no size) is `wfi` and `ret`.
        let wfi = sym(elf, "wfi");
        Freeze {
            body: function("panic_freeze"),
            wfi: wfi..wfi + 8,
        }
    }

    /// Whether hart `h` of `m` is there.
    fn holds(&self, m: &Machine, h: usize) -> bool {
        let (pc, ra) = (m.cpu.hart(h).pc, m.cpu.hart(h).x[1]);
        self.body.contains(&pc) || (self.wfi.contains(&pc) && self.body.contains(&ra))
    }
}

#[test]
fn a_panic_stops_the_harts_printing_meanwhile() {
    // While `stress 6 20` has workers printing on 4 harts, one hart faults
    // in the kernel (the test sets its pc to 0, unmapped, as it runs in
    // S-mode with the kernel's trap vector) while another is writing a
    // process's output to the UART (in uart_putc, holding uart_tx_lock).
    // The panic report must come out whole, word for word, after which
    // nothing else is printed; the printing hart stops in panic_freeze();
    // and the machine stops with the panic's exit code.
    const LINE: &str = "stress 6 20";
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load(4, rx);
    step_to_prompt(&mut m);
    for b in format!("{LINE}\n").bytes() {
        tx.send(b).unwrap();
    }
    let putc = function("uart_putc");
    let kernel_vec = sym(&elf, "kernel_vec");
    let cpus = sym(&elf, "cpus");
    let tx_lock = sym(&elf, "uart_tx_lock");
    // The printing hart, the faulting one, and the latter's registers and
    // sstatus as it faults.
    let mut fault = None;
    let found = step_until(
        &mut m,
        RUN_MAX_STEPS,
        |_| {},
        |m| {
            let holder = m.cpu.bus.peek(tx_lock + LOCK_CPU).unwrap();
            let Some(printer) = (0..4).find(|&h| {
                let hart = m.cpu.hart(h);
                let p = m.cpu.bus.peek(cpus + h as u32 * CPU_SIZE + CPU_PROC).unwrap();
                putc.contains(&hart.pc)
                    && holder == cpus + h as u32 * CPU_SIZE
                    && p != 0
                    && proc_name(&m.cpu, p) == "stress"
            }) else {
                return false;
            };
            let Some(victim) = (0..4).find(|&h| {
                let hart = m.cpu.hart(h);
                h != printer && hart.mode == Mode::S && hart.csr.stvec == kernel_vec
            }) else {
                return false;
            };
            let hart = m.cpu.hart(victim);
            fault = Some((printer, victim, hart.x, hart.csr.mstatus & SSTATUS_MASK));
            m.cpu.hart_mut(victim).pc = 0;
            true
        },
    );
    assert!(
        found,
        "no hart printed while another was in the kernel:\n{}",
        printed(&m)
    );
    let (printer, victim, regs, before) = fault.unwrap();
    assert_eq!(m.run(Some(RUN_MAX_STEPS), |_| {}), RunResult::Exited(PANIC_EXIT));
    let out = printed(&m);
    let at = out.find("panic: ").unwrap_or_else(|| panic!("no panic:\n{out}"));
    let report = &out[at..];
    // The trap from S-mode: SPP set, SPIE what SIE was, SIE off.
    let mut sstatus = (before & !(SSTATUS_SIE | SSTATUS_SPIE)) | SSTATUS_SPP;
    if before & SSTATUS_SIE != 0 {
        sstatus |= SSTATUS_SPIE;
    }
    let mut want = format!(
        "panic: instruction page fault\n  scause 0x0000000c  sepc 0x00000000  stval 0x00000000  sstatus \
         0x{sstatus:08x}\n"
    );
    // How the page table in satp maps 0: unmapped in the kernel's, a
    // process's code (not executable in S-mode) in a process's.
    let va = report.lines().nth(2).unwrap_or_default();
    assert!(va.starts_with("  va 0x00000000 "), "report:\n{report}");
    want += va;
    want += "\n";
    for (i, r) in regs.iter().enumerate().skip(1) {
        want += &format!("  {:>3} {r:08x}", excalibur::reg::abi_name(i as u8));
        if i % 4 == 0 || i == 31 {
            want += "\n";
        }
    }
    assert_eq!(
        report, want,
        "hart {victim} faulted while hart {printer} printed; output:\n{out}"
    );
    assert!(
        Freeze::new(&elf).holds(&m, printer),
        "hart {printer} at {:#x}",
        m.cpu.hart(printer).pc
    );
}

#[test]
fn a_hart_stops_printing_while_a_panic_begins() {
    // panic_begin() sets `panicking`, then stores its hart's id in
    // panic_hart: a hart that prints in between must take the panic for
    // another hart's and stop, hart 0 included, although its id is 0. The
    // test sets `panicking` alone as hart 0 enters smp_boot(), on 1 hart
    // and on 4: hart 0 stops in panic_freeze() at its next print (the shell's
    // banner on 1 hart, `smp: 4 harts running` on 4), and nothing more is
    // printed.
    for harts in [1, 4] {
        let (mut m, elf) = load(harts, kernel::typed(b""));
        let smp_boot = sym(&elf, "smp_boot");
        let panicking = sym(&elf, "panicking");
        assert!(step_until(
            &mut m,
            RUN_MAX_STEPS,
            |_| {},
            |m| m.cpu.hart(0).pc == smp_boot
        ));
        m.cpu.bus.load(panicking, &[1]).unwrap();
        let before = printed(&m);
        let freeze = Freeze::new(&elf);
        let stopped = step_until(&mut m, RUN_MAX_STEPS, |_| {}, |m| freeze.holds(m, 0));
        assert!(stopped, "{harts} harts:\n{}", printed(&m));
        assert_eq!(m.run(Some(1_000_000), |_| {}), RunResult::StepLimit);
        assert_eq!(printed(&m), before, "{harts} harts");
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
    slices.sort_by_key(|l| {
        l[l.find('[').unwrap() + 1..l.find(',').unwrap()]
            .parse::<u32>()
            .unwrap()
    });
    let slice = LIMIT / 4;
    for (w, line) in slices.iter().enumerate() {
        let (from, to) = (w as u32 * slice, (w as u32 + 1) * slice);
        assert_eq!(*line, format!("primes [{from}, {to}): {}", primes_in(from..to)));
    }
    assert_eq!(
        lines[4],
        format!("primes: {} below {LIMIT}, 4 workers", primes_in(0..LIMIT))
    );
    for b in b"exit\n" {
        tx.send(*b).unwrap();
    }
    assert_eq!(m.run(Some(RUN_MAX_STEPS), |_| {}), RunResult::Exited(0));
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
        Some(
            format!(
                "stress: {workers} workers, {rounds} rounds, {} bytes: ok",
                workers * rounds * 200
            )
            .as_str()
        )
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
