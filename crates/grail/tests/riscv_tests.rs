//! Runs the prebuilt riscv-tests ELFs in `tests/riscv-tests/` (RV32) and
//! `tests/riscv-tests/rv64/` (see `scripts/build-riscv-tests.sh`) plus the examples, and
//! checks each exits 0. The RV64 ones are ELF64 files, which make the machine RV64.

use grail::cpu::Cpu;
use grail::{Machine, RunResult, Xlen};
use std::path::{Path, PathBuf};

/// Excluded on purpose: assumes hardware misaligned access, which Excalibur traps
/// instead (spec section 4).
const EXCLUDED: &[&str] = &["rv32ui-p-ma_data", "rv64ui-p-ma_data"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run(path: &Path) -> (RunResult, Vec<u8>) {
    let file = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.cpu.bus.uart.capture = true;
    m.load_elf(&file).unwrap();
    let rv32 = m.cpu.xlen() == Xlen::Rv32;
    let r = m.run(Some(5_000_000), |cpu| {
        if rv32 {
            check_rv32_form(cpu);
        }
    });
    (r, std::mem::take(&mut m.cpu.bus.uart.captured))
}

/// An RV32 hart keeps its registers sign-extended from bit 31 and its pc below 4 GiB
/// (`crates/grail/src/cpu/mod.rs`). Checked before every step of every RV32 program:
/// the hart's own `debug_assert` of it is compiled out of the release builds the
/// suite runs in.
fn check_rv32_form(cpu: &Cpu) {
    for h in cpu.harts() {
        let bad = h.x.iter().position(|&v| v != v as i32 as u64);
        assert!(
            bad.is_none() && h.pc >> 32 == 0,
            "hart {}: an RV32 register or pc out of its form: pc {:#x}, x {:x?}",
            h.id,
            h.pc,
            h.x
        );
    }
}

fn run_suite(prefix: &str) {
    let dir = match &prefix[..4] {
        "rv64" => repo_root().join("tests/riscv-tests/rv64"),
        _ => repo_root().join("tests/riscv-tests"),
    };
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with(prefix) && !EXCLUDED.contains(&n.as_str()))
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no {prefix} tests found in {}", dir.display());

    let failures: Vec<String> = names
        .iter()
        .filter_map(|n| match run(&dir.join(n)).0 {
            RunResult::Exited(0) => None,
            RunResult::Exited(t) => Some(format!("{n}: failed test case {t}")),
            RunResult::StepLimit => Some(format!("{n}: did not finish")),
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} failed:\n{}",
        failures.len(),
        names.len(),
        failures.join("\n")
    );
}

#[test]
fn rv32ui() {
    run_suite("rv32ui-p-");
}

#[test]
fn rv32um() {
    run_suite("rv32um-p-");
}

#[test]
fn rv32ua() {
    run_suite("rv32ua-p-");
}

#[test]
fn rv32mi() {
    run_suite("rv32mi-p-");
}

#[test]
fn rv32si() {
    run_suite("rv32si-p-");
}

#[test]
fn rv64ui() {
    run_suite("rv64ui-p-");
}

#[test]
fn rv64um() {
    run_suite("rv64um-p-");
}

#[test]
fn rv64ua() {
    run_suite("rv64ua-p-");
}

#[test]
fn rv64mi() {
    run_suite("rv64mi-p-");
}

#[test]
fn rv64si() {
    run_suite("rv64si-p-");
}

#[test]
fn hello_example() {
    let (r, out) = run(&repo_root().join("examples/hello/hello.elf"));
    assert_eq!(r, RunResult::Exited(0));
    let out = String::from_utf8(out).unwrap();
    assert_eq!(
        out,
        "Hello from Camelot!\n[dbg] pc=80000018 a0 = 0x00000014 (20)\n"
    );
}
