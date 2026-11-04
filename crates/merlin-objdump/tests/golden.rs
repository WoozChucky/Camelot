//! Compares merlin-objdump with llvm-objdump output committed in
//! tests/golden/ (regenerate with scripts/gen-objdump-golden.sh; the fixture
//! sources and the commands are documented there).
//!
//! Lines must match exactly. Where llvm-objdump prints `<unknown>` (Xcal
//! instructions, data words, reserved fence encodings), our lines (`xcal.*`,
//! `.word`, `.byte`, `fence`, ...) must match the ones listed for that golden
//! file in tests/golden/unknown.txt, which is kept by hand.

use merlin_objdump::cli::{parse_args, Command};
use std::path::Path;

/// (golden file name, input relative to the repository root, options).
/// Keep in sync with scripts/gen-objdump-golden.sh.
const CASES: &[(&str, &str, &[&str])] = &[
    (
        "hello-elf.d",
        "crates/merlin-objdump/tests/fixtures/hello.elf",
        &["-d"],
    ),
    (
        "hello-elf.d-no-raw",
        "crates/merlin-objdump/tests/fixtures/hello.elf",
        &["-d", "--no-show-raw-insn"],
    ),
    (
        "hello-elf.d-no-aliases",
        "crates/merlin-objdump/tests/fixtures/hello.elf",
        &["-d", "-M", "no-aliases"],
    ),
    (
        "hello-elf.x",
        "crates/merlin-objdump/tests/fixtures/hello.elf",
        &["-x"],
    ),
    (
        "hello-elf.s",
        "crates/merlin-objdump/tests/fixtures/hello.elf",
        &["-s"],
    ),
    (
        "hello-o.dr",
        "crates/merlin-objdump/tests/fixtures/hello.o",
        &["-dr"],
    ),
    (
        "hello-o.x",
        "crates/merlin-objdump/tests/fixtures/hello.o",
        &["-x"],
    ),
    ("isa-o.d", "crates/merlin-objdump/tests/fixtures/isa.o", &["-d"]),
    (
        "isa-o.d-no-aliases",
        "crates/merlin-objdump/tests/fixtures/isa.o",
        &["-d", "-M", "no-aliases", "--no-show-raw-insn"],
    ),
    (
        "relocs-o.d",
        "crates/merlin-objdump/tests/fixtures/relocs.o",
        &["-d"],
    ),
    (
        "relocs-o.dr",
        "crates/merlin-objdump/tests/fixtures/relocs.o",
        &["-dr"],
    ),
    (
        "relocs-o.r",
        "crates/merlin-objdump/tests/fixtures/relocs.o",
        &["-r"],
    ),
    (
        "relocs-o.t",
        "crates/merlin-objdump/tests/fixtures/relocs.o",
        &["-t"],
    ),
    (
        "relocs-o.h",
        "crates/merlin-objdump/tests/fixtures/relocs.o",
        &["-h"],
    ),
    (
        "relocs-o.x",
        "crates/merlin-objdump/tests/fixtures/relocs.o",
        &["-x"],
    ),
    (
        "relocs-o.s",
        "crates/merlin-objdump/tests/fixtures/relocs.o",
        &["-s"],
    ),
    ("misc-o.x", "crates/merlin-objdump/tests/fixtures/misc.o", &["-x"]),
    (
        "misc-o.dr",
        "crates/merlin-objdump/tests/fixtures/misc.o",
        &["-dr"],
    ),
    (
        "debug-o.d",
        "crates/merlin-objdump/tests/fixtures/debug.o",
        &["-d"],
    ),
    (
        "debug-o.h",
        "crates/merlin-objdump/tests/fixtures/debug.o",
        &["-h"],
    ),
    (
        "debug-o.dl",
        "crates/merlin-objdump/tests/fixtures/debug.o",
        &["-d", "-l"],
    ),
    (
        "lines-elf.dl",
        "crates/merlin-objdump/tests/fixtures/lines.elf",
        &["-d", "-l"],
    ),
    (
        "subprograms-o.dl",
        "crates/merlin-objdump/tests/fixtures/subprograms.o",
        &["-d", "-l"],
    ),
    (
        "linked-elf.d",
        "crates/merlin-objdump/tests/fixtures/linked.elf",
        &["-d"],
    ),
    (
        "linked-elf.x",
        "crates/merlin-objdump/tests/fixtures/linked.elf",
        &["-x"],
    ),
    ("rv32ui-p-add.d", "tests/riscv-tests/rv32ui-p-add", &["-d"]),
    ("rv32mi-p-csr.d", "tests/riscv-tests/rv32mi-p-csr", &["-d"]),
    (
        "rv32si-p-dirty.dx",
        "tests/riscv-tests/rv32si-p-dirty",
        &["-d", "-x"],
    ),
    ("rv32ua-p-lrsc.d", "tests/riscv-tests/rv32ua-p-lrsc", &["-d"]),
    // RV64: ELF64 files, in golden/rv64/.
    ("rv64/hello-elf.d", RV64_HELLO_ELF, &["-d"]),
    (
        "rv64/hello-elf.d-no-raw",
        RV64_HELLO_ELF,
        &["-d", "--no-show-raw-insn"],
    ),
    (
        "rv64/hello-elf.d-no-aliases",
        RV64_HELLO_ELF,
        &["-d", "-M", "no-aliases"],
    ),
    ("rv64/hello-elf.x", RV64_HELLO_ELF, &["-x"]),
    ("rv64/hello-elf.s", RV64_HELLO_ELF, &["-s"]),
    ("rv64/hello-o.dr", RV64_HELLO_O, &["-dr"]),
    ("rv64/hello-o.x", RV64_HELLO_O, &["-x"]),
    ("rv64/isa-o.d", RV64_ISA, &["-d"]),
    (
        "rv64/isa-o.d-no-aliases",
        RV64_ISA,
        &["-d", "-M", "no-aliases", "--no-show-raw-insn"],
    ),
    ("rv64/relocs-o.dr", RV64_RELOCS, &["-dr"]),
    ("rv64/relocs-o.r", RV64_RELOCS, &["-r"]),
    ("rv64/relocs-o.t", RV64_RELOCS, &["-t"]),
    ("rv64/relocs-o.h", RV64_RELOCS, &["-h"]),
    ("rv64/relocs-o.x", RV64_RELOCS, &["-x"]),
    ("rv64/relocs-o.s", RV64_RELOCS, &["-s"]),
    ("rv64/misc-o.x", RV64_MISC, &["-x"]),
    ("rv64/misc-o.dr", RV64_MISC, &["-dr"]),
    ("rv64/debug-o.d", RV64_DEBUG, &["-d"]),
    ("rv64/debug-o.h", RV64_DEBUG, &["-h"]),
    ("rv64/debug-o.dl", RV64_DEBUG, &["-d", "-l"]),
    ("rv64/lines-elf.dl", RV64_LINES, &["-d", "-l"]),
    (
        "rv64/units-o.dl",
        "crates/merlin-objdump/tests/fixtures/rv64/units.o",
        &["-d", "-l"],
    ),
    (
        "rv64/units-dwarf4-o.dl",
        "crates/merlin-objdump/tests/fixtures/rv64/units-dwarf4.o",
        &["-d", "-l"],
    ),
    (
        "rv64/units-aranges-o.dl",
        "crates/merlin-objdump/tests/fixtures/rv64/units-aranges.o",
        &["-d", "-l"],
    ),
    (
        "rv64/nocu-o.dl",
        "crates/merlin-objdump/tests/fixtures/rv64/nocu.o",
        &["-d", "-l"],
    ),
    (
        "rv64/subprograms-o.dl",
        "crates/merlin-objdump/tests/fixtures/rv64/subprograms.o",
        &["-d", "-l"],
    ),
    (
        "rv64/units-elf.dl",
        "crates/merlin-objdump/tests/fixtures/rv64/units.elf",
        &["-d", "-l"],
    ),
    ("rv64/linked-elf.d", RV64_LINKED, &["-d"]),
    ("rv64/linked-elf.x", RV64_LINKED, &["-x"]),
    ("rv64/linked-elf.s", RV64_LINKED, &["-s"]),
    (
        "rv64/rv64ui-p-add.d",
        "tests/riscv-tests/rv64/rv64ui-p-add",
        &["-d"],
    ),
    (
        "rv64/rv64mi-p-csr.d",
        "tests/riscv-tests/rv64/rv64mi-p-csr",
        &["-d"],
    ),
    (
        "rv64/rv64si-p-dirty.dx",
        "tests/riscv-tests/rv64/rv64si-p-dirty",
        &["-d", "-x"],
    ),
    (
        "rv64/rv64ua-p-lrsc.d",
        "tests/riscv-tests/rv64/rv64ua-p-lrsc",
        &["-d"],
    ),
];

const RV64_HELLO_ELF: &str = "crates/merlin-objdump/tests/fixtures/rv64/hello.elf";
const RV64_HELLO_O: &str = "crates/merlin-objdump/tests/fixtures/rv64/hello.o";
const RV64_ISA: &str = "crates/merlin-objdump/tests/fixtures/rv64/isa.o";
const RV64_RELOCS: &str = "crates/merlin-objdump/tests/fixtures/rv64/relocs.o";
const RV64_MISC: &str = "crates/merlin-objdump/tests/fixtures/rv64/misc.o";
const RV64_DEBUG: &str = "crates/merlin-objdump/tests/fixtures/rv64/debug.o";
const RV64_LINES: &str = "crates/merlin-objdump/tests/fixtures/rv64/lines.elf";
const RV64_LINKED: &str = "crates/merlin-objdump/tests/fixtures/rv64/linked.elf";

fn root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn run(input: &str, args: &[&str]) -> String {
    let args = args.iter().map(|s| s.to_string()).chain([input.to_string()]);
    let Ok(Command::Dump { opts, .. }) = parse_args(args) else {
        panic!("bad arguments for {input}");
    };
    let bytes = std::fs::read(root().join(input)).unwrap();
    merlin_objdump::dump(input, &bytes, &opts).unwrap()
}

/// The address of a disassembly line (`80000000: ...`, `      1c: ...`).
/// Labels (`80000000 <_start>:`) and relocation lines (tab-indented) are not.
fn insn_addr(line: &str) -> Option<u64> {
    if line.starts_with('\t') {
        return None;
    }
    let (addr, _) = line.trim_start().split_once(':')?;
    u64::from_str_radix(addr, 16).ok()
}

/// The address of a label line (`80000000 <_start>:`).
fn label_addr(line: &str) -> Option<u64> {
    let (addr, rest) = line.split_once(" <")?;
    rest.ends_with(">:").then(|| u64::from_str_radix(addr, 16).ok())?
}

/// Our exact lines for the instructions llvm-objdump prints as `<unknown>`,
/// for one golden file, in order (from golden/unknown.txt).
fn expected_unknown(name: &str) -> Vec<String> {
    let path = root().join("crates/merlin-objdump/tests/golden/unknown.txt");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once('\t'))
        .filter(|&(n, _)| n == name)
        .map(|(_, line)| line.to_string())
        .collect()
}

/// Compares our output with llvm's; returns a description of every difference.
/// `unknown` lists, in order, our exact lines where llvm prints `<unknown>`.
fn compare(ours: &str, llvm: &str, unknown: &[String]) -> Vec<String> {
    // Byte ranges llvm could not decode: from an `<unknown>` line to the next
    // address llvm prints. llvm steps through them in 1, 2, 4 or more bytes
    // (it guesses the length from the low bits), we step in words, so our
    // lines there are checked against `unknown` instead.
    let mut addrs: Vec<u64> = llvm
        .lines()
        .filter_map(|l| insn_addr(l).or_else(|| label_addr(l)))
        .collect();
    addrs.sort();
    addrs.dedup();
    let ranges: Vec<(u64, u64)> = llvm
        .lines()
        .filter(|l| l.ends_with("<unknown>"))
        .filter_map(insn_addr)
        .map(|a| (a, addrs.iter().copied().find(|&b| b > a).unwrap_or(u64::MAX)))
        .collect();
    let is_unknown = |a: u64| ranges.iter().any(|&(start, end)| (start..end).contains(&a));
    let mut problems = Vec::new();
    let llvm_lines: Vec<&str> = llvm.lines().filter(|l| !l.ends_with("<unknown>")).collect();
    let mut our_lines = Vec::new();
    let mut our_unknown = Vec::new();
    for l in ours.lines() {
        match insn_addr(l) {
            Some(a) if is_unknown(a) => our_unknown.push(l),
            _ => our_lines.push(l),
        }
    }
    for (i, (o, g)) in our_lines.iter().zip(&llvm_lines).enumerate() {
        if o != g {
            problems.push(format!("line {}:\n  ours: {o:?}\n  llvm: {g:?}", i + 1));
        }
    }
    if our_lines.len() != llvm_lines.len() {
        problems.push(format!(
            "line count: ours {} llvm {}",
            our_lines.len(),
            llvm_lines.len()
        ));
    }
    for (o, e) in our_unknown.iter().zip(unknown) {
        if o != e {
            problems.push(format!(
                "where llvm has <unknown>:\n  ours:     {o:?}\n  expected: {e:?}"
            ));
        }
    }
    if our_unknown.len() != unknown.len() {
        problems.push(format!(
            "where llvm has <unknown>: we print {} lines, unknown.txt lists {}:\n{}",
            our_unknown.len(),
            unknown.len(),
            our_unknown.join("\n")
        ));
    }
    problems
}

#[test]
fn matches_llvm_objdump() {
    let mut failures = Vec::new();
    for &(name, input, args) in CASES {
        let golden = root()
            .join("crates/merlin-objdump/tests/golden")
            .join(format!("{name}.txt"));
        let llvm = std::fs::read_to_string(&golden).unwrap_or_else(|e| panic!("{}: {e}", golden.display()));
        let ours = run(input, args);
        let problems = compare(&ours, &llvm, &expected_unknown(name));
        if !problems.is_empty() {
            let shown: Vec<String> = problems.iter().take(8).cloned().collect();
            failures.push(format!(
                "{name} ({input} {}): {} differences\n{}",
                args.join(" "),
                problems.len(),
                shown.join("\n")
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

/// scripts/gen-objdump-golden.sh rebuilds every fixture a golden file is made
/// from, so regenerating the goldens never reads a stale object.
#[test]
fn generator_script_builds_every_fixture() {
    let script = std::fs::read_to_string(root().join("scripts/gen-objdump-golden.sh")).unwrap();
    const FIX: &str = "crates/merlin-objdump/tests/fixtures/";
    let mut missing = Vec::new();
    for &(name, input, _) in CASES {
        let Some(file) = input.strip_prefix(FIX) else {
            continue; // the riscv-tests ELFs come from scripts/build-riscv-tests.sh
        };
        let output = format!("-o \"$FIX/{file}\"");
        if !script
            .lines()
            .any(|l| !l.trim_start().starts_with('#') && l.contains(&output))
        {
            missing.push(format!("{name}: nothing writes {file}"));
        }
        let consumer = format!("\"$FIX/{file}\"");
        assert!(
            script.lines().any(|l| (l.starts_with(&format!("gen {name} "))
                || l.starts_with(&format!("gen_lines {name} ")))
                && l.contains(&consumer)),
            "{name} is not generated from {file}"
        );
    }
    assert!(missing.is_empty(), "{missing:?}");
}

/// Acceptance check for issue #5: every instruction of hello.elf, mnemonic
/// and operands, against `llvm-objdump -d --no-show-raw-insn`.
#[test]
fn hello_elf_every_instruction() {
    let llvm =
        std::fs::read_to_string(root().join("crates/merlin-objdump/tests/golden/hello-elf.d-no-raw.txt"))
            .unwrap();
    let ours = run(
        "crates/merlin-objdump/tests/fixtures/hello.elf",
        &["-d", "--no-show-raw-insn"],
    );
    let insns = |text: &str| -> Vec<(u64, String)> {
        text.lines()
            .filter_map(|l| {
                let a = insn_addr(l)?;
                let body = l.split_once(':').unwrap().1;
                Some((a, body.split_whitespace().collect::<Vec<_>>().join(" ")))
            })
            .collect()
    };
    let (ours, llvm) = (insns(&ours), insns(&llvm));
    assert_eq!(ours.len(), llvm.len());
    assert_eq!(ours.len(), 19);
    for ((oa, o), (la, l)) in ours.iter().zip(&llvm) {
        assert_eq!(oa, la);
        match l.as_str() {
            // Grail's Xcal extensions: llvm does not know them.
            "<unknown>" => assert!(o.starts_with("xcal."), "{oa:x}: {o}"),
            _ => assert_eq!(o, l, "at {oa:x}"),
        }
    }
    assert!(ours.contains(&(0x8000_0014, "xcal.dbg a0".to_string())));
    assert!(ours.contains(&(0x8000_0018, "xcal.halt zero".to_string())));
}

/// Acceptance check: a clang-built relocatable object shows its relocations by name.
#[test]
fn object_relocations_by_name() {
    let ours = run("crates/merlin-objdump/tests/fixtures/relocs.o", &["-r"]);
    for name in [
        "R_RISCV_CALL_PLT",
        "R_RISCV_PCREL_HI20",
        "R_RISCV_PCREL_LO12_I",
        "R_RISCV_PCREL_LO12_S",
        "R_RISCV_HI20",
        "R_RISCV_LO12_I",
        "R_RISCV_LO12_S",
        "R_RISCV_BRANCH",
        "R_RISCV_JAL",
        "R_RISCV_32",
        "R_RISCV_RELAX",
    ] {
        assert!(ours.contains(&format!("{name:<24} ")), "{name} missing:\n{ours}");
    }
    let dr = run("crates/merlin-objdump/tests/fixtures/relocs.o", &["-dr"]);
    assert!(dr.contains("auipc\tra, 0x0\n\t\t\t00000000:  R_RISCV_CALL_PLT\text_func\n"));
    assert!(dr.contains("\t\t\t0000003c:  R_RISCV_LO12_S\text_data-0x4\n"));
}

/// The comparison itself must catch differences.
#[test]
fn compare_detects_differences() {
    let llvm = "\n00000000 <f>:\n       0: 00000013     \tnop\n       4: 0005000b     \t<unknown>\n";
    let same = "\n00000000 <f>:\n       0: 00000013     \tnop\n       4: 0005000b     \txcal.dbg\ta0\n";
    let unknown = ["       4: 0005000b     \txcal.dbg\ta0".to_string()];
    assert!(compare(same, llvm, &unknown).is_empty());
    let wrong = same.replace("nop", "addi\tzero, zero, 0x0");
    assert_eq!(compare(&wrong, llvm, &unknown).len(), 1);
    let bad_unknown = same.replace("xcal.dbg\ta0", "xcal.dbg\ta1");
    assert_eq!(compare(&bad_unknown, llvm, &unknown).len(), 1);
    assert_eq!(compare(same, llvm, &[]).len(), 1);
    assert_eq!(compare(&format!("{same}extra\n"), llvm, &unknown).len(), 1);
}
