//! The Camelot kernel (`kernel/kernel.elf`, committed) run through the `grail`
//! binary, with `--max-steps` as a guard: booting with paging on (with several
//! `--ram` sizes), the timer ticks, the shell driven through stdin, the panics
//! from its deliberate faults, user programs taking turns or getting killed,
//! and image files attached with `--disk`. The kernel's own tests (traps,
//! delegation, the register dump, the page table, the console, processes,
//! disks) are in `crates/xtask/tests/kernel.rs`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Far more than a run needs (a boot is about 4.5 million steps with 128 MiB
/// of RAM, nearly all of them setting up memory; `wfi` skips ahead to each
/// tick), so hitting it means the kernel hung. Generous because stdin reaches
/// the machine through a thread: until the bytes arrive the kernel idles
/// through timer ticks, which costs steps.
const MAX_STEPS: &str = "200000000";

// Kernel constants, from the kernel sources named in each comment. grail
// can't depend on xtask, so crates/xtask/tests/kernel.rs has its own copies:
// keep them in sync.

/// Exit code of a kernel panic (`PANIC_EXIT_CODE` in kernel/include/trap.h).
const PANIC_EXIT: i32 = 0xfe;

/// Values of the kernel's `boot_fault` (kernel/main.c).
const BOOT_FAULT_ILLEGAL: u32 = 2;
const BOOT_FAULT_LOAD: u32 = 3;
const BOOT_FAULT_STORE_TEXT: u32 = 4;

/// What the kernel prints once paging is on (kernel/main.c, kernel/vmtest.c).
fn memory_lines(ram_mib: u32) -> String {
    format!("ram: {ram_mib} MiB at 0x80000000\npaging: on (Sv32)\nvm self-test: 26/26 passed\n")
}

fn kernel_elf() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../kernel/kernel.elf")
}

/// Runs `elf` on the `grail` binary with `stdin` as its standard input.
/// Returns the exit code and stdout.
fn grail(elf: &Path, stdin: &[u8]) -> (Option<i32>, String) {
    grail_with(&[], elf, stdin)
}

/// [`grail`] with extra command-line `args` before the image.
fn grail_with(args: &[&str], elf: &Path, stdin: &[u8]) -> (Option<i32>, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_grail"))
        .args(["--max-steps", MAX_STEPS])
        .args(args)
        .arg(elf)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("cannot run grail");
    // A kernel that panics may exit before reading it all: ignore a broken pipe.
    let _ = child.stdin.take().unwrap().write_all(stdin);
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("step limit"), "kernel hung:\n{stdout}{stderr}");
    (out.status.code(), stdout)
}

/// A copy of the kernel image with `boot_fault` set to `fault`, written to a
/// scratch file.
fn kernel_with_fault(fault: u32) -> PathBuf {
    let mut file = std::fs::read(kernel_elf()).unwrap();
    let elf = elf32::Elf::parse(&file).unwrap();
    let addr = elf
        .symbols()
        .unwrap()
        .into_iter()
        .find(|s| s.name == "boot_fault")
        .expect("no boot_fault symbol")
        .value;
    // The file offset of `addr`: in the loadable segment that holds it.
    let seg = elf
        .segments
        .iter()
        .find(|p| p.ty == elf32::PT_LOAD && (p.vaddr..p.vaddr + p.filesz).contains(&addr))
        .expect("boot_fault is not in the image");
    let off = (seg.offset + (addr - seg.vaddr)) as usize;
    assert_eq!(
        file[off..off + 4],
        1u32.to_le_bytes(),
        "boot_fault should ship as BOOT_FAULT_NONE"
    );
    file[off..off + 4].copy_from_slice(&fault.to_le_bytes());

    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("grail-kernel");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("kernel-fault-{fault}.elf"));
    std::fs::write(&path, file).unwrap();
    path
}

/// The ten tick lines the kernel prints before it starts the shell (or faults).
fn ticks() -> String {
    (1..=10).map(|n| format!("tick {n}\n")).collect()
}

/// What the kernel says after the timer line when no disk is attached
/// (kernel/main.c).
const NO_DISK: &str = "disk: none\n";

/// What the shell prints before its first prompt, and the prompt
/// (kernel/main.c).
const SHELL_BANNER: &str = "Type help for the list of commands.\n";
const PROMPT: &str = "camelot> ";

/// `out` from the shell's banner on.
fn shell_output(out: &str) -> &str {
    let at = out
        .find(SHELL_BANNER)
        .unwrap_or_else(|| panic!("the shell never started:\n{out}"));
    &out[at..]
}

#[test]
fn kernel_ticks_ten_times_then_halts_from_the_shell() {
    let (code, out) = grail(&kernel_elf(), b"halt\n");
    assert_eq!(code, Some(0), "output:\n{out}");
    assert!(out.starts_with("Camelot v0.1.0 booting...\n"), "{out}");
    assert!(
        out.ends_with(&format!(
            "{}timer: 10 Hz tick, 10000000 Hz timebase\n{NO_DISK}{}{SHELL_BANNER}{PROMPT}halt\n",
            memory_lines(128),
            ticks()
        )),
        "{out}"
    );
}

#[test]
fn shell_runs_ticks_then_halt_from_stdin() {
    // Issue #18's acceptance test. The kernel's console echoes each command,
    // then the shell prints its output.
    let (code, out) = grail(&kernel_elf(), b"ticks\nhalt\n");
    assert_eq!(code, Some(0), "output:\n{out}");
    let rest = shell_output(&out)
        .strip_prefix(&format!("{SHELL_BANNER}{PROMPT}ticks\nticks: "))
        .unwrap_or_else(|| panic!("{out}"));
    let (n, rest) = rest.split_once('\n').unwrap();
    // At least the ten boot ticks; more if stdin was slow to reach the machine.
    let n: u32 = n.parse().unwrap_or_else(|_| panic!("ticks: {n:?}"));
    assert!(n >= 10, "{out}");
    assert_eq!(rest, format!("{PROMPT}halt\n"));
}

#[test]
fn shell_accepts_windows_line_ends() {
    // Text piped on Windows may end its lines with CR LF, and a raw console
    // sends CR alone for Enter: both must end a line exactly once.
    let (code, out) = grail(&kernel_elf(), b"help\r\nnope\rhalt\r\n");
    assert_eq!(code, Some(0), "output:\n{out}");
    let want = format!(
        "{SHELL_BANNER}{PROMPT}help\n\
         help    this list\n\
         ticks   timer ticks since boot (10 per second)\n\
         run     run user programs side by side (run PROG...; run alone lists them)\n\
         halt    stop the machine\n\
         {PROMPT}nope\n\
         unknown command: nope (try help)\n\
         {PROMPT}halt\n"
    );
    assert_eq!(shell_output(&out), want);
}

#[test]
fn kernel_boots_with_paging_on_any_ram_size() {
    // The page allocator takes the RAM size from sysinfo, so --ram changes
    // what the kernel manages; the self-tests and the ticks run with paging
    // on either way, and so does the shell.
    for mib in ["1", "16", "300"] {
        let (code, out) = grail_with(&["--ram", mib], &kernel_elf(), b"halt\n");
        assert_eq!(code, Some(0), "--ram {mib}, output:\n{out}");
        let want = format!(
            "{}timer: 10 Hz tick, 10000000 Hz timebase\n{NO_DISK}{}{SHELL_BANNER}{PROMPT}halt\n",
            memory_lines(mib.parse().unwrap()),
            ticks()
        );
        assert!(out.ends_with(&want), "--ram {mib}:\n{out}");
    }
}

#[test]
fn illegal_instruction_panics() {
    let (code, out) = grail(&kernel_with_fault(BOOT_FAULT_ILLEGAL), b"");
    assert_eq!(code, Some(PANIC_EXIT), "output:\n{out}");
    let report = format!(
        "{}panic: illegal instruction\n  scause 0x00000002  sepc 0x",
        ticks()
    );
    assert!(out.contains(&report), "{out}");
    assert!(out.contains("  stval 0xc0001073  "), "{out}");
    // The register dump, with the values illegal_instruction put there.
    assert!(out.contains("   ra 01010101"), "{out}");
    assert!(out.contains("  t6 1f1f1f1f\n"), "{out}");
}

#[test]
fn unmapped_load_panics() {
    let (code, out) = grail(&kernel_with_fault(BOOT_FAULT_LOAD), b"");
    assert_eq!(code, Some(PANIC_EXIT), "output:\n{out}");
    // Paging is on: an address the page table doesn't map is a page fault.
    let report = format!("{}panic: load page fault\n  scause 0x0000000d  sepc 0x", ticks());
    assert!(out.contains(&report), "{out}");
    assert!(out.contains("  stval 0x00000100  "), "{out}");
    assert!(out.contains("\n  va 0x00000100 is not mapped\n"), "{out}");
}

#[test]
fn store_to_kernel_text_panics() {
    let (code, out) = grail(&kernel_with_fault(BOOT_FAULT_STORE_TEXT), b"");
    assert_eq!(code, Some(PANIC_EXIT), "output:\n{out}");
    let report = format!("{}panic: store page fault\n  scause 0x0000000f  sepc 0x", ticks());
    assert!(out.contains(&report), "{out}");
    // The page is mapped, read and execute only, and the line shows the PTE
    // as the kernel page table held it when the store faulted. (Whether A is
    // set depends on whether code on that page ran with paging on, which is
    // up to the layout: the xtask test of the same name works it out. The
    // store set neither A nor D, since it faulted.) The same kernel, run
    // in-process, shows that PTE in RAM as the trap is taken.
    let va = out
        .lines()
        .find_map(|l| l.strip_prefix("  va 0x")?.split_once(' ').map(|(va, _)| va))
        .map(|va| u32::from_str_radix(va, 16).unwrap())
        .unwrap_or_else(|| panic!("no mapping line:\n{out}"));
    let file = std::fs::read(kernel_with_fault(BOOT_FAULT_STORE_TEXT)).unwrap();
    let mut m = grail::Machine::new(grail::MachineConfig::default()).unwrap();
    m.cpu.bus.uart.capture = true;
    m.load_elf(&file).unwrap();
    const STORE_PAGE_FAULT: u32 = 15;
    // (The vm self-test's probes make store page faults too, elsewhere.)
    let mut at_fault = None;
    let r = m.run(Some(MAX_STEPS.parse().unwrap()), |cpu| {
        if at_fault.is_none() && cpu.csr.scause == STORE_PAGE_FAULT && cpu.csr.stval == va {
            let peek = |addr: u32| cpu.bus.peek(addr).unwrap();
            let root = (cpu.csr.satp & 0x3f_ffff) << 12;
            let table = (peek(root + 4 * (va >> 22)) >> 10) << 12;
            at_fault = Some(peek(table + 4 * (va >> 12 & 0x3ff)));
        }
    });
    assert_eq!(r, grail::RunResult::Exited(PANIC_EXIT as u32));
    let pte = at_fault.expect("no store page fault on that page in-process");
    let flags: String = "DAGUXWRV"
        .chars()
        .enumerate()
        .map(|(i, c)| if pte >> (7 - i) & 1 != 0 { c } else { '-' })
        .collect();
    assert_eq!(&flags[2..], "--X-RV", "pte {pte:#010x}");
    assert!(!flags.starts_with('D'), "pte {pte:#010x}");
    let line = format!("  va {va:#010x} maps to pa {va:#010x}, pte {pte:#010x} {flags}\n");
    assert!(out.contains(&line), "want {line}in:\n{out}");
}

// ---- User processes (issue #20) ----

/// The lines `run` prints from its command to the next prompt: what the
/// processes and the kernel printed about them.
fn run_output<'a>(out: &'a str, command: &str) -> Vec<&'a str> {
    let start = format!("{PROMPT}{command}\n");
    let at = out
        .find(&start)
        .unwrap_or_else(|| panic!("no `{command}` in:\n{out}"))
        + start.len();
    let rest = &out[at..];
    rest[..rest
        .find(PROMPT)
        .unwrap_or_else(|| panic!("`{command}` never ended:\n{out}"))]
        .lines()
        .collect()
}

#[test]
fn user_programs_run_interleaved() {
    // ping and pong each print four lines about half a tick apart, from
    // U-mode in their own address spaces. The scheduler switches on every
    // timer tick, so their lines alternate in pairs.
    let (code, out) = grail(&kernel_elf(), b"run ping pong\nhalt\n");
    assert_eq!(code, Some(0), "output:\n{out}");
    let lines = run_output(&out, "run ping pong");
    let printed: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| l.starts_with("ping ") || l.starts_with("pong "))
        .collect();
    assert_eq!(printed.len(), 8, "{out}");
    // Each in order, and they take turns more than once.
    for name in ["ping", "pong"] {
        let own: Vec<&str> = printed.iter().copied().filter(|l| l.starts_with(name)).collect();
        let want: Vec<String> = (1..=4).map(|i| format!("{name} {i}")).collect();
        assert_eq!(own, want, "{out}");
    }
    let turns = printed.windows(2).filter(|w| w[0][..4] != w[1][..4]).count();
    assert!(turns >= 3, "only {turns} switches:\n{out}");
    // ping, created first, goes first; both exit normally.
    assert_eq!(printed[0], "ping 1", "{out}");
    for exit in ["pid 1 (ping) exited: 0", "pid 2 (pong) exited: 0"] {
        assert!(lines.contains(&exit), "{exit} missing:\n{out}");
    }
}

#[test]
fn faulting_user_programs_are_killed_and_the_kernel_carries_on() {
    // badload reads kernel memory (mapped in its page table, but without U),
    // badhalt executes xcal.halt (M-mode only). Both die, the kernel says
    // why, hello still runs, and so does the shell afterwards.
    let (code, out) = grail(&kernel_elf(), b"run badload badhalt hello\nticks\nhalt\n");
    assert_eq!(code, Some(0), "output:\n{out}");
    let lines = run_output(&out, "run badload badhalt hello");
    assert_eq!(
        lines,
        [
            "pid 1 (badload) killed: load page fault",
            "  scause 0x0000000d  sepc 0x00001004  stval 0x80000000",
            "  va 0x80000000 maps to pa 0x80000000, pte 0x2000004b -A--X-RV",
            "pid 2 (badhalt) killed: illegal instruction",
            "  scause 0x00000002  sepc 0x00001004  stval 0x0005100b",
            "hello from user mode",
            "pid 3 (hello) exited: 0",
        ],
        "{out}"
    );
    assert!(out.contains(&format!("{PROMPT}ticks\nticks: ")), "{out}");
    assert!(out.ends_with(&format!("{PROMPT}halt\n")), "{out}");
}

// ---- Disks (issue #65) ----

/// A scratch disk image of `sectors` sectors, every byte different from its
/// neighbours, written next to the other scratch files. Returns its path and
/// contents.
fn disk_image(name: &str, sectors: usize) -> (PathBuf, Vec<u8>) {
    let bytes: Vec<u8> = (0..sectors * 512).map(|i| (i * 13 + i / 512) as u8).collect();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("grail-kernel");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, &bytes).unwrap();
    (path, bytes)
}

/// Checks `out` has `blk self-test: N/N passed` and returns N.
fn blk_selftest_passed(out: &str) -> u32 {
    let at = out
        .find("blk self-test: ")
        .unwrap_or_else(|| panic!("no disk self-test:\n{out}"));
    let line = out[at..].lines().next().unwrap();
    let (passed, total) = line
        .strip_prefix("blk self-test: ")
        .and_then(|r| r.strip_suffix(" passed"))
        .and_then(|r| r.split_once('/'))
        .unwrap_or_else(|| panic!("{line}"));
    assert_eq!(passed, total, "{out}");
    passed.parse().unwrap()
}

#[test]
fn kernel_tests_an_image_file_disk_and_leaves_it_as_it_was() {
    // The boot self-test writes a scratch block of the disk and puts it back.
    let (path, bytes) = disk_image("rw.img", 128);
    let (code, out) = grail_with(&["--disk", path.to_str().unwrap()], &kernel_elf(), b"halt\n");
    assert_eq!(code, Some(0), "output:\n{out}");
    assert!(
        out.contains("timebase\ndisk 0: 128 sectors (64 KiB), irq 3\ntick 1\n"),
        "{out}"
    );
    // After the ticks, before the shell.
    assert!(out.contains("tick 10\nblk self-test: "), "{out}");
    assert!(blk_selftest_passed(&out) > 10, "{out}");
    assert!(
        out.ends_with(&format!(" passed\n{SHELL_BANNER}{PROMPT}halt\n")),
        "{out}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn kernel_lists_every_disk_and_leaves_a_read_only_one_alone() {
    let (ro, bytes) = disk_image("ro.img", 64);
    let (rw, _) = disk_image("second.img", 8);
    let (code, out) = grail_with(
        &["--disk-ro", ro.to_str().unwrap(), "--disk", rw.to_str().unwrap()],
        &kernel_elf(),
        b"halt\n",
    );
    assert_eq!(code, Some(0), "output:\n{out}");
    assert!(
        out.contains(
            "timebase\n\
             disk 0: 64 sectors (32 KiB), irq 3, read-only\n\
             disk 1: 8 sectors (4 KiB), irq 4\n\
             tick 1\n"
        ),
        "{out}"
    );
    // Disk 0 is the one tested: its write must fail with BLK_ERR_READ_ONLY.
    blk_selftest_passed(&out);
    assert_eq!(std::fs::read(&ro).unwrap(), bytes);
}
