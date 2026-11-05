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
//! before it returns. `threadtest fs`, booted with the root image, checks
//! threads using the filesystem while another of their process execs, exits,
//! is killed or closes their descriptor: each ends, and the disk is left
//! consistent and as it was.
//!
//! The tests watch every hart's turn with `Cpu::step_traced`, as smp.rs's
//! do, and read the kernel's tables from memory (the layouts are in the
//! constants below and in common/mod.rs, with the headers they come from).

mod common;

use camelot_fs::mkfs::Options;
use camelot_fs::{fsck, SECTOR_SIZE};
use common::*;
use grail::block::MemDisk;
use grail::cpu::{Cpu, Mode};
use grail::Machine;
use std::collections::HashSet;
use std::ops::Range;
use xtask::repo_root;

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
const PROC_SIZE: u32 = 172;
const NPROC: u32 = 8;
const PROC_STATE: u32 = 0;
const PROC_PAGETABLE: u32 = 32;

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

/// One hart's call of tlb_shootdown() under way (see [`Shootdowns`]).
#[derive(Clone, Copy)]
struct Shootdown {
    /// Where the call returns to, and its sp.
    ra: u32,
    sp: u32,
    /// The page table shot down.
    pt: u32,
    /// The other harts that have had `pt` in satp (cpus[i].pagetable)
    /// without a break from the call's start to its IPI (or its return, if
    /// it sends none): the call must target each, as it looks at every
    /// hart's in between.
    running: u32,
    /// Whether the IPI has gone out.
    sent: bool,
    /// The harts the IPI went to, and those that have flushed since the
    /// call began.
    targets: u32,
    flushed: u32,
}

/// Watches the TLB shootdowns of user page tables (smp.c): for each call of
/// tlb_shootdown() on a page table other than the kernel's, the harts its
/// IPI asks to flush, and which of them did (in tlb_serve()) before the call
/// returned. Every one of them must have; and every other hart that ran the
/// page table from the call's start until its IPI must have been one of
/// them.
/// `shared` counts the calls that reached another hart, `most` the most
/// other harts that ran the table through one call's start and IPI (so
/// that the check above had something to check).
struct Shootdowns {
    entry: u32,
    body: Range<u32>,
    ipi: u32,
    sfence: u32,
    serve: Range<u32>,
    kernel_pt: u32,
    cpus: u32,
    harts: usize,
    /// Per requesting hart.
    active: [Option<Shootdown>; grail::MAX_HARTS],
    shared: u32,
    most: u32,
}

impl Shootdowns {
    fn new(elf: &grail::elf::Elf, m: &Machine, harts: usize) -> Shootdowns {
        Shootdowns {
            entry: sym(elf, "tlb_shootdown"),
            body: function("tlb_shootdown"),
            ipi: sym(elf, "machine_ipi"),
            sfence: sym(elf, "sfence_vma"),
            serve: function("tlb_serve"),
            kernel_pt: m.cpu.bus.peek(sym(elf, "kernel_pagetable").into()).unwrap(),
            cpus: sym(elf, "cpus"),
            harts,
            active: [None; grail::MAX_HARTS],
            shared: 0,
            most: 0,
        }
    }

    /// The harts whose cpus[i].pagetable is `pt`.
    fn running(&self, cpu: &Cpu, pt: u32) -> u32 {
        (0..self.harts as u32)
            .filter(|&i| peek(cpu, self.cpus + i * CPU_SIZE + CPU_PAGETABLE) == pt)
            .fold(0, |set, i| set | 1 << i)
    }

    fn step(&mut self, cpu: &Cpu) {
        let h = cpu.id as usize;
        if cpu.pc as u32 == self.entry && cpu.x[10] as u32 != self.kernel_pt {
            let pt = cpu.x[10] as u32;
            self.active[h] = Some(Shootdown {
                ra: cpu.x[1] as u32,
                sp: cpu.x[2] as u32,
                pt,
                running: self.running(cpu, pt) & !(1 << h),
                sent: false,
                targets: 0,
                flushed: 0,
            });
        }
        // A hart that leaves the page table, even for a moment, need not be
        // asked: it flushes as it comes back (the scheduler's sfence.vma).
        for k in 0..self.harts {
            if let Some(mut s) = self.active[k] {
                if !s.sent {
                    s.running &= self.running(cpu, s.pt);
                    self.active[k] = Some(s);
                }
            }
        }
        if let Some(mut s) = self.active[h] {
            if cpu.pc as u32 == self.ipi && self.body.contains(&(cpu.x[1] as u32)) {
                s.targets = cpu.x[10] as u32;
                s.sent = true;
                self.active[h] = Some(s);
            } else if cpu.pc as u32 == s.ra && cpu.x[2] as u32 == s.sp {
                assert_eq!(
                    s.running & s.targets,
                    s.running,
                    "hart {h}'s shootdown of {:#x} left out harts {:#x}, which ran it",
                    s.pt,
                    s.running & !s.targets
                );
                assert_eq!(
                    s.targets & s.flushed,
                    s.targets,
                    "hart {h}'s shootdown returned before harts {:#x} flushed",
                    s.targets & !s.flushed
                );
                if s.targets != 0 {
                    self.shared += 1;
                }
                self.most = self.most.max(s.running.count_ones());
                self.active[h] = None;
            }
        }
        // A flush for another hart's request: any from when the request
        // began, as a target may serve its ticket before the IPI goes out
        // (another request's IPI woke it).
        if cpu.pc as u32 == self.sfence && self.serve.contains(&(cpu.x[1] as u32)) {
            for (k, a) in self.active.iter_mut().enumerate() {
                if let Some(s) = a {
                    if k != h {
                        s.flushed |= 1 << h;
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
        if cpu.pc as u32 != self.free || !self.join.contains(&(cpu.x[1] as u32)) {
            return;
        }
        let t = cpu.x[10] as u32;
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
    // of sbrk() from three threads at once reach every other hart running
    // the process before they return, two of them at once at least once.
    for harts in [1, 4] {
        let (tx, rx) = std::sync::mpsc::channel();
        let (mut m, elf) = load(harts, rx);
        step_to_prompt(&mut m);
        let before = free_pages(&m, &elf);
        let mut locks = LockWatch::new(&elf);
        let mut shootdowns = Shootdowns::new(&elf, &m, harts);
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
        assert_eq!(
            lines[3],
            format!("threadtest: {THREADTEST_CHECKS} checks, 0 failed"),
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
            assert!(
                shootdowns.most >= 2,
                "no shootdown found two other harts running the process"
            );
        }
        exit_shell(&mut m, &tx);
    }
}

/// The checks `threadtest` and `threadtest fs` make: each must run every one.
const THREADTEST_CHECKS: u32 = 96;
const THREADTEST_FS_CHECKS: u32 = 16;

#[test]
fn threads_using_the_filesystem_end_with_their_process_on_one_and_four_harts() {
    // threadtest fs, booted with the root image (so programs come from the
    // disk): a process execs while another of its threads reads the same
    // program, or writes more than a transaction's worth at a time; exits
    // and is killed while its threads read and write; closes a descriptor
    // two of its threads are reading and writing; changes directory while
    // another opens. Every one ends (exec waits for the others only once it
    // has let go of the program's inode and its transaction), every page
    // and thread slot is back, and the disk is consistent and holds what it
    // did, the scratch files gone.
    let image = xtask::fs::build(
        &repo_root(),
        &Options {
            size: xtask::fs::IMAGE_SIZE,
            ..Options::default()
        },
    )
    .unwrap();
    let before = fsck::check(&image).unwrap_or_else(|p| panic!("{p:#?}"));
    for harts in [1, 4] {
        let (tx, rx) = std::sync::mpsc::channel();
        let disk = MemDisk::from_image(image.clone()).unwrap();
        let (mut m, elf) = load_with(harts, rx, vec![Box::new(disk)]);
        step_to_prompt(&mut m);
        let pages = free_pages(&m, &elf);
        let mut locks = LockWatch::new(&elf);
        run_command(&mut m, &tx, "threadtest fs", |cpu| locks.step(cpu));
        let out = printed(&m);
        assert_eq!(
            command_output(&out, "threadtest fs"),
            [format!("threadtest fs: {THREADTEST_FS_CHECKS} checks, 0 failed")],
            "{harts} harts"
        );
        assert_eq!(free_pages(&m, &elf), pages, "{harts} harts: pages leaked");
        check_only_the_shell(&m, &elf);
        exit_shell(&mut m, &tx);
        let d = &mut m.cpu.bus.disks[0];
        let mut disk = vec![0; d.capacity() as usize * SECTOR_SIZE];
        d.backend().read(0, &mut disk).unwrap();
        let after = fsck::check(&disk).unwrap_or_else(|p| panic!("{harts} harts: {p:#?}"));
        assert_eq!((after.replayed, after.orphans), (0, 0), "{harts} harts");
        assert_eq!(
            (after.free_blocks, after.free_inodes),
            (before.free_blocks, before.free_inodes),
            "{harts} harts: the scratch files left something behind"
        );
    }
}
