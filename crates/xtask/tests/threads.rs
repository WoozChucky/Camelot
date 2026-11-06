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
//! consistent and as it was. `maptest threads` (issue #179) maps and unmaps
//! memory from four threads at once: munmap frees pages only after a
//! shootdown that reached every hart running the process; and `maptest`'s
//! two processes share an object while running on two harts at once.
//!
//! The tests watch every hart's turn with `Cpu::step_traced`, as smp.rs's
//! do, and read the kernel's tables from memory (the layouts are in the
//! constants below and in common/mod.rs, with the headers they come from).
//! They count the pages as kstacks.rs does (a kernel stack stays mapped in
//! its hart's cache once its thread ends), after a first run of the command
//! that has made the tables the kernel stacks' slots need, filled the
//! caches and grown the shell's stack.

mod common;

#[path = "../../grail/tests/common/kstacks.rs"]
mod kstacks;

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
/// process and kernel stack (the pointers 8 bytes).
const THREAD_SIZE: u64 = 192;
const NTHREAD: u64 = 16;
const THREAD_STATE: u64 = 0;
const THREAD_TID: u64 = 4;
const THREAD_PROC: u64 = 8;
const THREAD_KSTACK: u64 = 16;
/// Thread states (kernel/include/proc.h).
const THREAD_UNUSED: u32 = 0;
const THREAD_ZOMBIE: u32 = 3;
/// `struct proc`: its size and state (its name and page table are in
/// common/mod.rs).
const PROC_SIZE: u64 = 464;
const NPROC: u64 = 8;
const PROC_STATE: u64 = 0;

/// The slots of the thread table in use, as (state, tid, process, kernel
/// stack).
fn threads_in_use(m: &Machine, elf: &grail::elf::Elf) -> Vec<(u32, u32, u64, u64)> {
    let table = sym(elf, "threads");
    (0..NTHREAD)
        .map(|i| table + i * THREAD_SIZE)
        .map(|t| {
            (
                peek(&m.cpu, t + THREAD_STATE),
                peek(&m.cpu, t + THREAD_TID),
                peek64(&m.cpu, t + THREAD_PROC),
                peek64(&m.cpu, t + THREAD_KSTACK),
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
/// harts with the process's page table in satp; and whether two processes
/// of that name did.
struct SideBySide {
    cpus: u64,
    name: &'static str,
    harts: usize,
    /// Per hart, this step: the thread running in U-mode, and its process.
    on: [(u64, u64); grail::MAX_HARTS],
    /// The most of the process's threads seen in U-mode at once.
    most: usize,
    /// The most processes called `name` seen in U-mode at once.
    apart: usize,
}

impl SideBySide {
    fn new(elf: &grail::elf::Elf, name: &'static str, harts: usize) -> SideBySide {
        SideBySide {
            cpus: sym(elf, "cpus"),
            name,
            harts,
            on: [(0, 0); grail::MAX_HARTS],
            most: 0,
            apart: 0,
        }
    }

    fn step(&mut self, cpu: &Cpu) {
        let h = cpu.id as usize;
        let me = self.cpus + u64::from(cpu.id) * CPU_SIZE;
        self.on[h] = (0, 0);
        if cpu.mode == Mode::U {
            let p = peek64(cpu, me + CPU_PROC);
            if p != 0 && proc_name(cpu, p) == self.name {
                // The hart runs on the process's own page table.
                assert_eq!(peek64(cpu, me + CPU_PAGETABLE), peek64(cpu, p + PROC_PAGETABLE));
                self.on[h] = (peek64(cpu, me + CPU_THREAD), p);
            }
        }
        // The last hart's turn: every hart's state is from this step.
        if h == self.harts - 1 {
            let running: Vec<(u64, u64)> = self.on[..self.harts]
                .iter()
                .copied()
                .filter(|t| t.0 != 0)
                .collect();
            let threads: HashSet<u64> = running.iter().map(|t| t.0).collect();
            let procs: HashSet<u64> = running.iter().map(|t| t.1).collect();
            if procs.len() == 1 && threads.len() == running.len() {
                self.most = self.most.max(threads.len());
            }
            self.apart = self.apart.max(procs.len());
        }
    }
}

/// One hart's call of tlb_shootdown() under way (see [`Shootdowns`]).
#[derive(Clone, Copy)]
struct Shootdown {
    /// Where the call returns to, and its sp.
    ra: u64,
    sp: u64,
    /// The page table shot down.
    pt: u64,
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
    entry: u64,
    body: Range<u64>,
    ipi: u64,
    sfence: u64,
    serve: Range<u64>,
    kernel_pt: u64,
    cpus: u64,
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
            ipi: sym(elf, "sbi_send_ipi"),
            sfence: sym(elf, "sfence_vma"),
            serve: function("tlb_serve"),
            kernel_pt: peek64(&m.cpu, sym(elf, "kernel_pagetable")),
            cpus: sym(elf, "cpus"),
            harts,
            active: [None; grail::MAX_HARTS],
            shared: 0,
            most: 0,
        }
    }

    /// The harts whose cpus[i].pagetable is `pt`.
    fn running(&self, cpu: &Cpu, pt: u64) -> u32 {
        (0..self.harts as u64)
            .filter(|&i| peek64(cpu, self.cpus + i * CPU_SIZE + CPU_PAGETABLE) == pt)
            .fold(0, |set, i| set | 1 << i)
    }

    fn step(&mut self, cpu: &Cpu) {
        let h = cpu.id as usize;
        if cpu.pc == self.entry && cpu.x[10] != self.kernel_pt {
            let pt = cpu.x[10];
            self.active[h] = Some(Shootdown {
                ra: cpu.x[1],
                sp: cpu.x[2],
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
            if cpu.pc == self.ipi && self.body.contains(&cpu.x[1]) {
                s.targets = cpu.x[10] as u32;
                s.sent = true;
                self.active[h] = Some(s);
            } else if cpu.pc == s.ra && cpu.x[2] == s.sp {
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
        if cpu.pc == self.sfence && self.serve.contains(&cpu.x[1]) {
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

/// Watches munmap's unmap_range() (mmap.c) free the pages it unmapped
/// (release_areas()) only after its tlb_shootdown(): Grail has no TLB, so a
/// translation a hart kept would never show, and the order is what the test
/// can check. [`Shootdowns`] checks that the shootdown reached every hart.
/// `unmaps` counts the frees.
struct UnmapOrder {
    unmap: Range<u64>,
    release: u64,
    shootdown: u64,
    /// Per hart: whether it has called tlb_shootdown() since it last
    /// entered unmap_range().
    shot: [bool; grail::MAX_HARTS],
    unmaps: u32,
}

impl UnmapOrder {
    fn new(elf: &grail::elf::Elf) -> UnmapOrder {
        UnmapOrder {
            unmap: function("unmap_range"),
            release: sym(elf, "release_areas"),
            shootdown: sym(elf, "tlb_shootdown"),
            shot: [false; grail::MAX_HARTS],
            unmaps: 0,
        }
    }

    fn step(&mut self, cpu: &Cpu) {
        let h = cpu.id as usize;
        if cpu.pc == self.unmap.start {
            self.shot[h] = false;
        } else if cpu.pc == self.shootdown && self.unmap.contains(&cpu.x[1]) {
            self.shot[h] = true;
        } else if cpu.pc == self.release && self.unmap.contains(&cpu.x[1]) {
            assert!(self.shot[h], "hart {h}'s munmap freed pages before its shootdown");
            self.unmaps += 1;
        }
    }
}

/// Watches thread_join() (proc.c) free the slot of the thread it joined,
/// under proc_lock: the thread has ended, and holds no kernel stack any
/// more (the scheduler freed it as the thread switched away for the last
/// time). `joined` counts them.
struct Joins {
    free: u64,
    join: Range<u64>,
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
            peek64(cpu, t + THREAD_KSTACK),
            0,
            "an ended thread kept its kernel stack"
        );
        self.joined += 1;
    }
}

/// Runs `line` once and checks it printed `last` as its last line, so that
/// the run counted afterwards starts with every table it needs already
/// made, the kernel stack caches full and the shell's stack grown.
fn warm_up(m: &mut Machine, tx: &std::sync::mpsc::Sender<u8>, line: &str, last: &str) {
    run_command(m, tx, line, |_| {});
    let out = printed(m);
    assert_eq!(command_output(&out, line).last().copied(), Some(last), "{out}");
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
        let want = "threads: 4 threads, 300 rounds: counter 1200, spun 1200: ok";
        warm_up(&mut m, &tx, LINE, want);
        let before = kstacks::pages(&mut m, &committed_elf());
        check_only_the_shell(&m, &elf);
        let mut locks = LockWatch::new(&elf);
        let mut side = SideBySide::new(&elf, "threads", harts);
        run_command(&mut m, &tx, LINE, |cpu| {
            locks.step(cpu);
            side.step(cpu);
        });
        let out = printed(&m);
        assert_eq!(command_output(&out, LINE), [want], "{harts} harts");
        assert_eq!(
            kstacks::pages(&mut m, &committed_elf()),
            before,
            "{harts} harts: pages leaked"
        );
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
        let checks = format!("threadtest: {THREADTEST_CHECKS} checks, 0 failed");
        warm_up(&mut m, &tx, "threadtest", &checks);
        let before = kstacks::pages(&mut m, &committed_elf());
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
        assert!(
            lines[1].starts_with("  scause 0x000000000000000f  sepc 0x"),
            "{lines:#?}"
        );
        assert!(lines[1].ends_with("  stval 0x0000000000000000"), "{lines:#?}");
        assert_eq!(lines[2], "  va 0x0000000000000000 is not mapped");
        assert_eq!(lines[3], checks, "{harts} harts: {lines:#?}");
        assert_eq!(
            kstacks::pages(&mut m, &committed_elf()),
            before,
            "{harts} harts: pages leaked"
        );
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

#[test]
fn munmap_shoots_down_every_hart_and_processes_share_objects_on_one_and_four_harts() {
    // Issue #179: `maptest threads`' four threads map, fill and unmap
    // memory at once, each taking addresses another has just unmapped; on 4
    // harts they run side by side. Each new mapping reads zero and a
    // thread's writes stay its own; every munmap frees its pages only after
    // its shootdown, and every shootdown (munmap's and mprotect's) reaches
    // every other hart running the process before it returns (two of them
    // at once at least once). `maptest`'s shared
    // object is seen by two processes running at the same time on two
    // harts. Afterwards every page and thread slot is back.
    let threads = format!("maptest: {MAPTEST_THREADS_CHECKS} checks, 0 failed");
    let shared = format!("maptest: {MAPTEST_CHECKS} checks, 0 failed");
    for harts in [1, 4] {
        let (tx, rx) = std::sync::mpsc::channel();
        let (mut m, elf) = load(harts, rx);
        step_to_prompt(&mut m);
        warm_up(&mut m, &tx, "maptest threads", &threads);
        warm_up(&mut m, &tx, "maptest", &shared);
        let before = kstacks::pages(&mut m, &committed_elf());

        let mut locks = LockWatch::new(&elf);
        let mut shootdowns = Shootdowns::new(&elf, &m, harts);
        let mut side = SideBySide::new(&elf, "maptest", harts);
        let mut order = UnmapOrder::new(&elf);
        run_command(&mut m, &tx, "maptest threads", |cpu| {
            locks.step(cpu);
            shootdowns.step(cpu);
            side.step(cpu);
            order.step(cpu);
        });
        let out = printed(&m);
        assert_eq!(
            command_output(&out, "maptest threads"),
            [&threads],
            "{harts} harts"
        );
        // Every round's munmap, and the counter's two.
        assert!(order.unmaps >= 4 * 40 + 2, "{} unmaps", order.unmaps);
        check_only_the_shell(&m, &elf);

        let mut processes = SideBySide::new(&elf, "maptest", harts);
        run_command(&mut m, &tx, "maptest", |cpu| {
            locks.step(cpu);
            shootdowns.step(cpu);
            processes.step(cpu);
        });
        let out = printed(&m);
        assert_eq!(command_output(&out, "maptest"), [&shared], "{harts} harts");
        assert_eq!(
            kstacks::pages(&mut m, &committed_elf()),
            before,
            "{harts} harts: pages leaked"
        );
        check_only_the_shell(&m, &elf);
        if harts == 1 {
            assert_eq!((side.most, processes.apart), (1, 1));
        } else {
            assert!(side.most >= 2, "the threads never ran side by side");
            assert!(
                shootdowns.shared > 0,
                "no shootdown reached another hart running the process"
            );
            assert!(
                shootdowns.most >= 2,
                "no shootdown found two other harts running the process"
            );
            assert_eq!(processes.apart, 2, "maptest and its child never ran at once");
        }
        exit_shell(&mut m, &tx);
    }
}

/// The checks `threadtest` and `threadtest fs` make: each must run every one.
const THREADTEST_CHECKS: u32 = 100;
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
        let checks = format!("threadtest fs: {THREADTEST_FS_CHECKS} checks, 0 failed");
        warm_up(&mut m, &tx, "threadtest fs", &checks);
        let pages = kstacks::pages(&mut m, &committed_elf());
        let mut locks = LockWatch::new(&elf);
        run_command(&mut m, &tx, "threadtest fs", |cpu| locks.step(cpu));
        let out = printed(&m);
        assert_eq!(command_output(&out, "threadtest fs"), [&checks], "{harts} harts");
        assert_eq!(
            kstacks::pages(&mut m, &committed_elf()),
            pages,
            "{harts} harts: pages leaked"
        );
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

#[test]
fn poll_loses_no_wakeup_on_one_and_four_harts() {
    // Issue #180: two pairs of threads pass a byte back and forth, one pair
    // over pipes and one over a local socket pair, each thread waiting for
    // it in poll() on its own end and on 40 copies of a pipe nothing is
    // written to: a wakeup lost would leave its pair stopped, and the
    // command would never end. On 4 harts the threads run side by side, so
    // a write's wakeup comes from another hart while its reader may still
    // be scanning its files.
    const LINE: &str = "polltest stress 300";
    let want = "polltest: stress 300 rounds, 0 failed";
    for harts in [1, 4] {
        let (tx, rx) = std::sync::mpsc::channel();
        let (mut m, elf) = load(harts, rx);
        step_to_prompt(&mut m);
        warm_up(&mut m, &tx, LINE, want);
        let mut side = SideBySide::new(&elf, "polltest", harts);
        run_command(&mut m, &tx, LINE, |cpu| side.step(cpu));
        let out = printed(&m);
        assert_eq!(command_output(&out, LINE), [want], "{harts} harts");
        check_only_the_shell(&m, &elf);
        if harts > 1 {
            assert!(side.most >= 2, "the threads never ran side by side");
        }
        exit_shell(&mut m, &tx);
    }
}
