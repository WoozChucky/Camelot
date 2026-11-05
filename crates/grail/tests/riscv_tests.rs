//! Runs the prebuilt riscv-tests ELFs in `tests/riscv-tests/` (see
//! `scripts/build-riscv-tests.sh`) plus the hello example, and checks each exits 0.
//! They run in M-mode, on machines without firmware.

use grail::{Machine, RunResult};
use std::path::{Path, PathBuf};

/// Excluded on purpose: assumes hardware misaligned access, which Excalibur traps
/// instead (spec section 4).
const EXCLUDED: &[&str] = &["rv64ui-p-ma_data"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run(path: &Path) -> (RunResult, Vec<u8>) {
    let file = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.cpu.bus.uart.capture = true;
    m.load_elf(&file).unwrap();
    let r = m.run(Some(5_000_000), |_| {});
    (r, std::mem::take(&mut m.cpu.bus.uart.captured))
}

fn run_suite(prefix: &str) {
    let dir = repo_root().join("tests/riscv-tests");
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
        "Hello from Camelot!\n[dbg] pc=0000000080000018 a0 = 0x0000000000000014 (20)\n"
    );
}
