//! The RV64 kernel on several harts (#156): the runs of smp.rs and
//! threads.rs on `kernel/kernel-rv64.elf`, with the programs built for RV64.
//! Four CPU-bound workers run side by side, one on each hart, and count
//! right; the lock stress test and systest hold up on four harts; the disk
//! kernel runs programs from the RV64 root image's `/bin` on four; threads
//! add up under a mutex on one hart and on four, and threads using the
//! filesystem end with their process, leaving the disk consistent. Every
//! page is back afterwards ([`pages`]). What smp.rs and
//! threads.rs check of the kernel's own structures (which hart holds which
//! lock, the TLB shootdowns, the thread slots) is RV32's layout; the RV64
//! kernel's are checked by its self-tests and by tests/kernel_rv64.rs.

use camelot_fs::fsck;
use camelot_fs::mkfs::Options;
use grail::cpu::Mode;
use grail::{DiskConfig, Machine, MachineConfig, RunResult};
use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender};
use xtask::kernel::{self, BOOT_MAX_STEPS};
use xtask::repo_root;

const PROMPT: &str = "$ ";
const MIB: usize = 1 << 20;
/// Steps for a command (the longest here, stress on one hart's worth of
/// turns, takes about 60 million).
const RUN_MAX_STEPS: u64 = 300_000_000;
/// Where the kernel image is linked and where it is loaded.
const KERNEL_LINK_ADDR: u64 = 0xffff_ffff_8000_0000;
const LOAD_ADDR: u64 = 0x8000_0000;

fn committed() -> Vec<u8> {
    let path = repo_root().join("kernel").join(kernel::KERNEL_ELF_RV64);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The committed RV64 kernel on `harts` harts with `disks`, the UART
/// reading `input`; and its symbols.
fn load(harts: usize, input: Receiver<u8>, disks: Vec<DiskConfig>) -> (Machine, grail::elf::Elf) {
    let file = committed();
    let elf = grail::elf::parse(&file).unwrap();
    let mut m = Machine::new(MachineConfig {
        harts,
        ram_size: 16 * MIB,
        xlen: grail::Xlen::Rv64,
        disks,
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

/// The byte at `va` in the kernel image, which is linked at
/// KERNEL_LINK_ADDR and loaded at LOAD_ADDR.
fn image_byte(m: &Machine, va: u64) -> u8 {
    let pa = va - KERNEL_LINK_ADDR + LOAD_ADDR;
    (m.cpu.bus.peek(pa & !3).unwrap() >> (8 * (pa & 3))) as u8
}

/// The pages the kernel has not lost: the free ones (`nfree`,
/// kernel/kalloc.c) and those of every kernel stack mapped in the vmalloc
/// area (`kstack_state`, kernel/vm.c: a slot in use, or freed and kept
/// mapped, in its hart's cache or waiting for kstack_reclaim(), as Linux
/// keeps freed thread stacks: KSTACK_SIZE, 4 pages, each). A process that
/// ends gives its pages back, but its kernel stack may stay mapped for the
/// next one, so nfree alone need not come back to where it was. Nor do the
/// tables of the kernel's page table that mapping a stack slot no thread
/// had used before makes: they stay, as Linux's kernel page tables do. So
/// a test runs its command once before it counts ([`warm_up`]).
fn pages(m: &Machine, elf: &grail::elf::Elf) -> u32 {
    let file = committed();
    let parsed = elf::Elf::parse(&file).unwrap();
    let state = parsed
        .symbols()
        .unwrap()
        .into_iter()
        .find(|s| s.name == "kstack_state")
        .expect("no kstack_state");
    let mapped = (0..state.size)
        .filter(|&i| image_byte(m, state.value + i) != 0)
        .count() as u32;
    let va = elf.symbol("nfree").expect("no nfree");
    let free = m.cpu.bus.peek(va - KERNEL_LINK_ADDR + LOAD_ADDR).unwrap();
    free + 4 * mapped
}

/// Steps `m` until the shell has printed its first prompt.
fn step_to_prompt(m: &mut Machine) {
    for _ in 0..2 * BOOT_MAX_STEPS {
        if m.cpu.bus.uart.captured.ends_with(PROMPT.as_bytes()) {
            return;
        }
        m.cpu.step();
    }
    panic!("no prompt:\n{}", printed(m));
}

/// Types `line` and steps `m`, calling `each` before every hart's turn,
/// until the shell prompts again.
fn run_command(m: &mut Machine, tx: &Sender<u8>, line: &str, mut each: impl FnMut(&grail::cpu::Cpu)) {
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

/// Runs `line` once and checks it printed what it prints when it is
/// counted (`last`, its last line), so that the run counted afterwards
/// starts with every table it needs already made ([`pages`]).
fn warm_up(m: &mut Machine, tx: &Sender<u8>, line: &str, last: &str) {
    run_command(m, tx, line, |_| {});
    let out = printed(m);
    assert_eq!(command_output(&out, line).last().copied(), Some(last), "{out}");
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
    for b in b"exit 0\n" {
        tx.send(*b).unwrap();
    }
    assert_eq!(
        m.run(Some(RUN_MAX_STEPS), |_| {}),
        RunResult::Exited(0),
        "{}",
        printed(m)
    );
}

/// Which harts are in U-mode, as each takes its turn: the most at once, and
/// every hart that was.
struct UserHarts {
    now: Vec<bool>,
    most: usize,
    ran: HashSet<usize>,
}

impl UserHarts {
    fn new(harts: usize) -> UserHarts {
        UserHarts {
            now: vec![false; harts],
            most: 0,
            ran: HashSet::new(),
        }
    }

    fn step(&mut self, cpu: &grail::cpu::Cpu) {
        let h = cpu.id as usize;
        self.now[h] = cpu.mode == Mode::U;
        if self.now[h] {
            self.ran.insert(h);
        }
        self.most = self.most.max(self.now.iter().filter(|&&u| u).count());
    }
}

/// The number of primes in `range`, by a sieve.
fn primes_in(range: std::ops::Range<u32>) -> u32 {
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
    // primes 4 40000: four workers, CPU-bound, the only programs running
    // meanwhile (the shell and primes itself wait for them). At some point
    // all four harts are in U-mode at once, every hart runs one, and each
    // worker's count, and the total, are right.
    const LINE: &str = "primes 4 40000";
    const LIMIT: u32 = 40000;
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, _) = load(4, rx, Vec::new());
    step_to_prompt(&mut m);
    let mut user = UserHarts::new(4);
    run_command(&mut m, &tx, LINE, |cpu| user.step(cpu));
    assert_eq!(user.most, 4, "the workers never ran on four harts at once");
    assert_eq!(user.ran.len(), 4, "U-mode ran on harts {:?}", user.ran);
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
    exit_shell(&mut m, &tx);
}

/// Checks stress's output for `workers` workers and `rounds` rounds
/// (smp.rs's check_stress_output).
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
    // stress's six workers allocating and freeing pages, printing and
    // writing down one pipe, with four harts going through the locks at
    // once: every worker's output complete and in order, every page back,
    // and U-mode code on every hart.
    const LINE: &str = "stress 6 3";
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut m, elf) = load(4, rx, Vec::new());
    step_to_prompt(&mut m);
    warm_up(&mut m, &tx, LINE, "stress: 6 workers, 3 rounds, 3600 bytes: ok");
    let before = pages(&m, &elf);
    let mut user = UserHarts::new(4);
    run_command(&mut m, &tx, LINE, |cpu| user.step(cpu));
    let out = printed(&m);
    check_stress_output(&command_output(&out, LINE), 6, 3);
    assert_eq!(pages(&m, &elf), before, "pages leaked");
    assert_eq!(user.ran.len(), 4, "stress ran on harts {:?}", user.ran);
    exit_shell(&mut m, &tx);
}

#[test]
fn systest_passes_on_four_harts() {
    let (mut m, _) = load(4, kernel::typed(b"systest\nexit 0\n"), Vec::new());
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

/// The `harts` lines after its header (kernel/user/harts.c): hart, pid
/// ("-" while idle), ticks, switches.
fn parse_harts(lines: &[&str]) -> Vec<(u32, String, u32, u32)> {
    assert_eq!(lines.first(), Some(&"HART   PID    TICKS SWITCHES"), "{lines:#?}");
    lines[1..]
        .iter()
        .map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (
                f[0].parse().unwrap(),
                f[1].to_string(),
                f[2].parse().unwrap(),
                f[3].parse().unwrap(),
            )
        })
        .collect()
}

#[test]
fn the_disk_kernel_runs_programs_from_bin_on_four_harts() {
    // The RV64 root image (cargo xtask fs --xlen 64) boots the RV64 disk
    // kernel on four harts; its shell runs harts and stress from /bin, and
    // the disk is consistent afterwards.
    let image =
        xtask::fs::build_bootable_for(&repo_root(), xtask::fs::IMAGE_SIZE, kernel::Xlen::Rv64).unwrap();
    let mut m = Machine::new(MachineConfig {
        harts: 4,
        xlen: grail::Xlen::Rv64,
        disks: vec![DiskConfig::memory(image)],
        ..MachineConfig::default()
    })
    .unwrap();
    m.boot_from_disk().unwrap();
    m.cpu.bus.uart.capture = true;
    m.set_uart_input(kernel::typed(b"harts\nstress 6 3\nexit 0\n"));
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

#[test]
fn threads_add_up_under_a_mutex_on_one_and_four_harts() {
    // threads 4 300: four threads update shared data under a mutex (and a
    // spinlock) and are joined; the result is right on one hart and on
    // four, where they run side by side (the process's only code then:
    // more than one hart in U-mode at once), and every page is back.
    const LINE: &str = "threads 4 300";
    for harts in [1, 4] {
        let (tx, rx) = std::sync::mpsc::channel();
        let (mut m, elf) = load(harts, rx, Vec::new());
        step_to_prompt(&mut m);
        warm_up(
            &mut m,
            &tx,
            LINE,
            "threads: 4 threads, 300 rounds: counter 1200, spun 1200: ok",
        );
        let before = pages(&m, &elf);
        let mut user = UserHarts::new(harts);
        run_command(&mut m, &tx, LINE, |cpu| user.step(cpu));
        let out = printed(&m);
        assert_eq!(
            command_output(&out, LINE),
            ["threads: 4 threads, 300 rounds: counter 1200, spun 1200: ok"],
            "{harts} harts"
        );
        assert_eq!(pages(&m, &elf), before, "{harts} harts: pages leaked");
        if harts == 1 {
            assert_eq!(user.most, 1);
        } else {
            assert!(user.most >= 2, "the threads never ran side by side");
        }
        exit_shell(&mut m, &tx);
    }
}

/// The checks `threadtest fs` makes (threads.rs's THREADTEST_FS_CHECKS).
const THREADTEST_FS_CHECKS: u32 = 16;

#[test]
fn threads_using_the_filesystem_end_with_their_process_on_one_and_four_harts() {
    // threadtest fs on the RV64 root filesystem (threads.rs's test of the
    // same name): exec, exit, kill, close and chdir while other threads of
    // the process read and write files. Every one ends, every page is back,
    // and the disk is consistent and holds what it did.
    let opts = Options {
        size: xtask::fs::IMAGE_SIZE,
        ..Options::default()
    };
    let image = xtask::fs::build_for(&repo_root(), &opts, kernel::Xlen::Rv64).unwrap();
    let before = fsck::check(&image).unwrap_or_else(|p| panic!("{p:#?}"));
    for harts in [1, 4] {
        let (tx, rx) = std::sync::mpsc::channel();
        let (mut m, elf) = load(harts, rx, vec![DiskConfig::memory(image.clone())]);
        step_to_prompt(&mut m);
        let fs_line = format!("threadtest fs: {THREADTEST_FS_CHECKS} checks, 0 failed");
        warm_up(&mut m, &tx, "threadtest fs", &fs_line);
        let start = pages(&m, &elf);
        run_command(&mut m, &tx, "threadtest fs", |_| {});
        let out = printed(&m);
        assert_eq!(
            command_output(&out, "threadtest fs"),
            [format!("threadtest fs: {THREADTEST_FS_CHECKS} checks, 0 failed")],
            "{harts} harts"
        );
        assert_eq!(pages(&m, &elf), start, "{harts} harts: pages leaked");
        exit_shell(&mut m, &tx);
        let d = &mut m.cpu.bus.disks[0];
        let mut disk = vec![0; d.capacity() as usize * 512];
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
