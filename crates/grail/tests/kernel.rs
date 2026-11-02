//! The Camelot kernel (`kernel/kernel.elf`, committed) run through the `grail`
//! binary, with `--max-steps` as a guard: the timer ticks, and the panics from
//! its deliberate faults. The kernel's own tests (traps, delegation, the
//! register dump) are in `crates/xtask/tests/kernel.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Far more than a boot needs (a few tens of thousands of steps, since `wfi`
/// skips ahead to the next tick), so hitting it means the kernel hung.
const MAX_STEPS: &str = "2000000";

/// Exit code of a kernel panic (`PANIC_EXIT_CODE` in kernel/include/trap.h).
const PANIC_EXIT: i32 = 0xfe;

/// Values of the kernel's `boot_fault` (kernel/main.c).
const BOOT_FAULT_ILLEGAL: u32 = 2;
const BOOT_FAULT_LOAD: u32 = 3;

fn kernel_elf() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../kernel/kernel.elf")
}

fn grail(elf: &Path) -> (Option<i32>, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_grail"))
        .args(["--max-steps", MAX_STEPS])
        .arg(elf)
        .output()
        .expect("cannot run grail");
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

/// The ten tick lines the kernel prints before it halts (or faults).
fn ticks() -> String {
    (1..=10).map(|n| format!("tick {n}\n")).collect()
}

#[test]
fn kernel_ticks_ten_times_and_exits_0() {
    let (code, out) = grail(&kernel_elf());
    assert_eq!(code, Some(0), "output:\n{out}");
    assert!(out.starts_with("Camelot v0.1.0 booting...\n"), "{out}");
    assert!(
        out.ends_with(&format!("timer: 10 Hz tick, 10000000 Hz timebase\n{}", ticks())),
        "{out}"
    );
}

#[test]
fn illegal_instruction_panics() {
    let (code, out) = grail(&kernel_with_fault(BOOT_FAULT_ILLEGAL));
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
    let (code, out) = grail(&kernel_with_fault(BOOT_FAULT_LOAD));
    assert_eq!(code, Some(PANIC_EXIT), "output:\n{out}");
    let report = format!(
        "{}panic: load access fault\n  scause 0x00000005  sepc 0x",
        ticks()
    );
    assert!(out.contains(&report), "{out}");
    assert!(out.contains("  stval 0x00000100  "), "{out}");
}
