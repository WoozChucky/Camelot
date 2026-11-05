//! Stress test: every riscv-tests program, relinked by merlin-ld from the
//! object it was built from (`scripts/build-riscv-tests.sh`, with
//! `-mno-relax`), must give the same loaded bytes as the committed ld.lld
//! executable and pass in Grail. The suites are RV64's, in
//! `tests/riscv-tests`, with their objects and the env's `link.ld` in
//! `obj/`.
//!
//! The executables were linked by the LLD merlin-ld follows (23.1.4), and
//! relinking their objects with it gives the same files, so their program
//! headers are compared whole too: offsets, load addresses, sizes, flags
//! and alignment. The objects carry only PC-relative relocations
//! (PCREL_HI20 and its LO12_I and LO12_S): R_RISCV_64 and the range edges
//! are the fixtures' (`tests/link.rs`).

use grail::{Machine, RunResult};
use merlin_ld::{Input, Options, ScriptSource};
use std::path::{Path, PathBuf};

/// Not run, as in Grail's own riscv-tests runner: it expects misaligned
/// accesses to work in hardware, which Excalibur traps instead (spec §4).
/// Its bytes are still compared.
const NOT_RUN: &str = "rv64ui-p-ma_data";

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/riscv-tests")
}

/// The PT_LOADs of an executable, with their file bytes.
fn loads(file: &[u8]) -> Result<Vec<(elf::ProgramHeader, Vec<u8>)>, String> {
    let e = elf::Elf::parse(file).map_err(|e| e.to_string())?;
    e.segments
        .iter()
        .filter(|p| p.ty == elf::PT_LOAD)
        .map(|p| Ok((p.clone(), e.segment_data(p).map_err(|e| e.to_string())?.to_vec())))
        .collect()
}

/// Links `name.o` and checks it; the error says what went wrong.
fn check(name: &str, opts: &Options) -> Result<(), String> {
    let obj = dir().join("obj").join(format!("{name}.o"));
    let inputs = [Input {
        name: format!("{name}.o"),
        data: std::fs::read(&obj).map_err(|e| format!("{}: {e}", obj.display()))?,
    }];
    let linked = merlin_ld::link(&inputs, opts).map_err(|e| format!("link: {e:?}"))?;
    let lld = std::fs::read(dir().join(name)).map_err(|e| format!("ld.lld executable: {e}"))?;
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
        if a != b {
            return Err(format!("segment {va:#x}: {a:x?}, ld.lld: {b:x?}"));
        }
        if let Some(i) = (0..a_bytes.len()).find(|&i| a_bytes[i] != b_bytes[i]) {
            return Err(format!("first differing byte at {:#x}", va + i as u64));
        }
    }
    if name == NOT_RUN {
        return Ok(());
    }
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.cpu.bus.uart.capture = true;
    m.load_elf(&linked.elf).map_err(|e| format!("load: {e}"))?;
    match m.run(Some(5_000_000), |_| {}) {
        RunResult::Exited(0) => Ok(()),
        RunResult::Exited(t) => Err(format!("failed test case {t}")),
        RunResult::StepLimit => Err("did not finish".into()),
    }
}

#[test]
fn riscv_tests_relinked_by_merlin_ld() {
    let script = dir().join("obj/link.ld");
    let opts = Options {
        script: Some(ScriptSource {
            name: "link.ld".into(),
            text: std::fs::read_to_string(&script).unwrap(),
        }),
        entry: None,
    };
    let mut names: Vec<String> = std::fs::read_dir(dir().join("obj"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "o"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    // One object per prebuilt executable.
    let mut elfs: Vec<String> = std::fs::read_dir(dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with("rv64"))
        .collect();
    elfs.sort();
    assert_eq!(names, elfs);
    assert!(names.iter().any(|n| n == NOT_RUN));

    let failures: Vec<String> = names
        .iter()
        .filter_map(|n| check(n, &opts).err().map(|e| format!("{n}: {e}")))
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} failed:\n{}",
        failures.len(),
        names.len(),
        failures.join("\n")
    );
    assert!(names.len() > 100, "{} objects", names.len());
}
