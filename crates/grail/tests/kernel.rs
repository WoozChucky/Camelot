//! The Camelot kernel (`kernel/kernel.elf`, committed) run through the `grail`
//! binary, with `--max-steps` as a guard: booting with paging on (with several
//! `--ram` sizes), the timer ticks, the shell driven through stdin, and the
//! panics from its deliberate faults. The kernel's own tests (traps,
//! delegation, the register dump, the page table, the console) are in
//! `crates/xtask/tests/kernel.rs`.

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
            "{}timer: 10 Hz tick, 10000000 Hz timebase\n{}{SHELL_BANNER}{PROMPT}halt\n",
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
            "{}timer: 10 Hz tick, 10000000 Hz timebase\n{}{SHELL_BANNER}{PROMPT}halt\n",
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
    // The page is mapped, read and execute only.
    assert!(out.contains(" -A--X-RV\n"), "{out}");
}
