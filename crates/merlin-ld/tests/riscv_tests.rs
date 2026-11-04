//! Stress test: every riscv-tests program, relinked by merlin-ld from the
//! object it was built from (`scripts/build-riscv-tests.sh`, with
//! `-mno-relax`), must give the same loaded bytes as the committed ld.lld
//! executable and pass in Grail. Two suites: RV32 (`tests/riscv-tests`,
//! objects in `obj/`) and RV64 (`tests/riscv-tests/rv64`, objects in
//! `rv64/obj/`), each with its env's `link.ld`.
//!
//! The RV64 executables were linked by the LLD merlin-ld follows (23.1.4),
//! and relinking their objects with it gives the same files, so their
//! program headers are compared whole too: offsets, load addresses, sizes,
//! flags and alignment. The RV32 ones come from older LLDs (18 and 21), whose
//! `.comment` moves what follows it, so only their addresses and contents are.
//! The RV64 objects carry only PC-relative relocations (PCREL_HI20 and its
//! LO12_I and LO12_S): R_RISCV_64 and the range edges are the fixtures'
//! (`tests/rv64.rs`).

use grail::{Machine, RunResult};
use merlin_ld::{Input, Options, ScriptSource};
use std::path::{Path, PathBuf};

/// One suite: a directory of executables, with their objects and the
/// script in its `obj/`.
struct Suite {
    /// Relative to `tests/riscv-tests`.
    dir: &'static str,
    /// The name every executable of the suite starts with.
    prefix: &'static str,
    /// Not run, as in Grail's own riscv-tests runner: it expects misaligned
    /// accesses to work in hardware, which Excalibur traps instead (spec
    /// §4). Its bytes are still compared.
    not_run: &'static str,
    /// The references are from the same LLD: compare every program header
    /// field, not only the addresses and the bytes.
    same_lld: bool,
}

const RV32: Suite = Suite {
    dir: "",
    prefix: "rv32",
    not_run: "rv32ui-p-ma_data",
    same_lld: false,
};

const RV64: Suite = Suite {
    dir: "rv64",
    prefix: "rv64",
    not_run: "rv64ui-p-ma_data",
    same_lld: true,
};

fn dir(suite: &Suite) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/riscv-tests")
        .join(suite.dir)
}

/// The PT_LOADs of an executable (either class), with their file bytes.
fn loads(file: &[u8]) -> Result<Vec<(elf::ProgramHeader, Vec<u8>)>, String> {
    let e = elf::Elf::parse(file).map_err(|e| e.to_string())?;
    e.segments
        .iter()
        .filter(|p| p.ty == elf::PT_LOAD)
        .map(|p| Ok((p.clone(), e.segment_data(p).map_err(|e| e.to_string())?.to_vec())))
        .collect()
}

/// Links `name.o` and checks it; the error says what went wrong.
fn check(suite: &Suite, name: &str, opts: &Options) -> Result<(), String> {
    let obj = dir(suite).join("obj").join(format!("{name}.o"));
    let inputs = [Input {
        name: format!("{name}.o"),
        data: std::fs::read(&obj).map_err(|e| format!("{}: {e}", obj.display()))?,
    }];
    let linked = merlin_ld::link(&inputs, opts).map_err(|e| format!("link: {e:?}"))?;
    let lld = std::fs::read(dir(suite).join(name)).map_err(|e| format!("ld.lld executable: {e}"))?;
    let (ours, theirs) = (loads(&linked.elf)?, loads(&lld)?);
    if ours.len() != theirs.len() {
        return Err(format!("{} PT_LOADs, ld.lld has {}", ours.len(), theirs.len()));
    }
    for ((a, a_bytes), (b, b_bytes)) in ours.iter().zip(&theirs) {
        let (va, vb) = (a.vaddr, b.vaddr);
        if (va, a.memsz, a.filesz) != (vb, b.memsz, b.filesz) {
            return Err(format!(
                "segment {va:#x} ({:#x} bytes, {:#x} in the file), ld.lld: {vb:#x} ({:#x}, {:#x})",
                a.memsz, a.filesz, b.memsz, b.filesz
            ));
        }
        if suite.same_lld && a != b {
            return Err(format!("segment {va:#x}: {a:x?}, ld.lld: {b:x?}"));
        }
        if let Some(i) = (0..a_bytes.len()).find(|&i| a_bytes[i] != b_bytes[i]) {
            return Err(format!("first differing byte at {:#x}", va + i as u64));
        }
    }
    if name == suite.not_run {
        return Ok(());
    }
    // An ELF64 file makes the machine RV64.
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.cpu.bus.uart.capture = true;
    m.load_elf(&linked.elf).map_err(|e| format!("load: {e}"))?;
    match m.run(Some(5_000_000), |_| {}) {
        RunResult::Exited(0) => Ok(()),
        RunResult::Exited(t) => Err(format!("failed test case {t}")),
        RunResult::StepLimit => Err("did not finish".into()),
    }
}

/// Relinks every program of `suite`; returns how many there were.
fn relink(suite: &Suite) -> usize {
    let script = dir(suite).join("obj/link.ld");
    let opts = Options {
        script: Some(ScriptSource {
            name: "link.ld".into(),
            text: std::fs::read_to_string(&script).unwrap(),
        }),
        entry: None,
    };
    let mut names: Vec<String> = std::fs::read_dir(dir(suite).join("obj"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "o"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    // One object per prebuilt executable.
    let mut elfs: Vec<String> = std::fs::read_dir(dir(suite))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with(suite.prefix))
        .collect();
    elfs.sort();
    assert_eq!(names, elfs);
    assert!(names.iter().any(|n| n == suite.not_run));

    let failures: Vec<String> = names
        .iter()
        .filter_map(|n| check(suite, n, &opts).err().map(|e| format!("{n}: {e}")))
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} failed:\n{}",
        failures.len(),
        names.len(),
        failures.join("\n")
    );
    names.len()
}

#[test]
fn riscv_tests_relinked_by_merlin_ld() {
    let n = relink(&RV32);
    assert!(n > 60, "{n} objects");
}

#[test]
fn rv64_riscv_tests_relinked_by_merlin_ld() {
    let n = relink(&RV64);
    assert!(n > 100, "{n} objects");
}
