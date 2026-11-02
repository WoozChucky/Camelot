//! Stress test: every riscv-tests program, relinked by merlin-ld from the
//! object it was built from (`tests/riscv-tests/obj`, written by
//! `scripts/build-riscv-tests.sh` with `-mno-relax`), must give the same
//! loaded bytes as the committed ld.lld executable (`tests/riscv-tests/<name>`)
//! and pass in Grail.

use grail::{Machine, RunResult};
use merlin_ld::{Input, Options, ScriptSource};
use std::path::{Path, PathBuf};

/// Not run, as in Grail's own riscv-tests runner: it expects misaligned
/// accesses to work in hardware, which Excalibur traps instead (spec §4).
/// Its bytes are still compared.
const NOT_RUN: &[&str] = &["rv32ui-p-ma_data"];

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/riscv-tests")
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// `(vaddr, memsz, file bytes)` of every PT_LOAD of an executable.
fn load_images(elf: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    let (phoff, phnum) = (u32_at(elf, 28) as usize, u16_at(elf, 44) as usize);
    (0..phnum)
        .map(|i| phoff + 32 * i)
        .filter(|&p| u32_at(elf, p) == 1)
        .map(|p| {
            let (off, filesz) = (u32_at(elf, p + 4) as usize, u32_at(elf, p + 16) as usize);
            (
                u32_at(elf, p + 8),
                u32_at(elf, p + 20),
                elf[off..off + filesz].to_vec(),
            )
        })
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
    let (ours, theirs) = (load_images(&linked.elf), load_images(&lld));
    if ours.len() != theirs.len() {
        return Err(format!("{} PT_LOADs, ld.lld has {}", ours.len(), theirs.len()));
    }
    for ((va, msz, a), (vb, mszb, b)) in ours.iter().zip(&theirs) {
        if (va, msz, a.len()) != (vb, mszb, b.len()) {
            return Err(format!(
                "segment {va:#x} ({msz:#x} bytes, {:#x} in the file), ld.lld: {vb:#x} ({mszb:#x}, {:#x})",
                a.len(),
                b.len()
            ));
        }
        if let Some(i) = (0..a.len()).find(|&i| a[i] != b[i]) {
            return Err(format!("first differing byte at {:#x}", va + i as u32));
        }
    }
    if NOT_RUN.contains(&name) {
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
        .filter(|n| n.starts_with("rv32"))
        .collect();
    elfs.sort();
    assert_eq!(names, elfs);
    assert!(names.len() > 60, "{} objects", names.len());

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
}
