//! Bad instructions must be rejected with an error on their own source line,
//! never silently encoded with truncated fields.
//!
//! Same progress rules as `encode_golden`: a case whose only error is
//! "not implemented yet" is counted, not failed. `encode_errors_complete`
//! (ignored by default) requires every case to give a real error, including
//! unknown mnemonics, which today fall into the encoder's "not implemented yet"
//! arm and should end up as "unknown instruction"-style errors:
//!
//!     cargo test -p merlin-as --test encode_errors -- --nocapture
//!     cargo test -p merlin-as --test encode_errors -- --ignored

use merlin_as::encode::UNIMPLEMENTED;
use std::process::Command;

/// Each case is assembled on line 3, after two lines that cannot fail.
const LINE: usize = 3;

const CASES: &[(&str, &str)] = &[
    // 12-bit signed immediates: -2048..=2047
    ("addi a0, a0, 2048", "I-type immediate above 2047"),
    ("addi a0, a0, -2049", "I-type immediate below -2048"),
    ("slti a0, a0, 0x1000", "I-type immediate too wide"),
    ("jalr ra, 2048(t0)", "jalr offset out of range"),
    ("lw a0, 2048(sp)", "load offset out of range"),
    ("lbu a0, -2049(sp)", "load offset out of range"),
    ("sw a0, 2048(sp)", "store offset out of range"),
    ("sb a0, -2049(sp)", "store offset out of range"),
    // Shift amounts: 0..=31 on RV32
    ("slli a0, a0, 32", "shamt above 31"),
    ("srli a0, a0, -1", "negative shamt"),
    ("srai a0, a0, 64", "shamt above 31"),
    // Branches: even offsets in -4096..=4094
    ("beq a0, a1, 4096", "branch offset above 4094"),
    ("bne a0, a1, -4098", "branch offset below -4096"),
    ("blt a0, a1, 3", "odd branch offset"),
    ("beqz a0, 4096", "pseudo-branch offset out of range"),
    // Jumps: even offsets in -1MiB..=1MiB-2
    ("jal ra, 1048576", "jal offset above 1048574"),
    ("jal ra, -1048578", "jal offset below -1048576"),
    ("jal ra, 5", "odd jal offset"),
    ("j 1048576", "j offset out of range"),
    // U-type: 20-bit field
    ("lui a0, 0x100000", "lui immediate wider than 20 bits"),
    ("lui a0, -1", "negative lui immediate"),
    ("auipc a0, 0x100000", "auipc immediate wider than 20 bits"),
    // CSRs: 12-bit address, 5-bit zero-extended immediate
    ("csrrw a0, 4096, a1", "CSR number above 0xfff"),
    ("csrrw a0, nosuchcsr, a1", "unknown CSR name"),
    ("csrrwi a0, mstatus, 32", "CSR immediate above 31"),
    ("csrrsi a0, mstatus, -1", "negative CSR immediate"),
    ("csrwi mstatus, 32", "CSR immediate above 31"),
    // li takes a 32-bit value (signed or unsigned)
    ("li a0, 0x100000000", "li value wider than 32 bits"),
    ("li a0, -2147483649", "li value below -2^31"),
    // Wrong operand count or kind
    ("add a0, a1", "missing operand"),
    ("add a0, a1, a2, a3", "extra operand"),
    ("addi a0, a1, a2", "register where an immediate belongs"),
    (
        "sub a0, a1, 5",
        "immediate where a register belongs (sub has no immediate form)",
    ),
    ("lw a0, a1", "register where a memory operand or symbol belongs"),
    ("ecall a0", "operand on a no-operand instruction"),
    ("xcal.dbg 5", "xcal.dbg needs a register"),
    ("xcal.halt", "xcal.halt needs a register"),
    ("ret a0", "operand on ret"),
    // Unknown mnemonics
    ("frobnicate a0, a1", "unknown mnemonic"),
    ("mulw a0, a1, a2", "RV64-only mnemonic"),
    ("c.addi a0, 1", "compressed mnemonic (no C extension)"),
    ("lr.w a0, (a1)", "A extension mnemonic (not in Excalibur)"),
];

fn source(case: &str) -> String {
    format!("    .text\nstart:\n    {case}\n")
}

fn run(require_all: bool) {
    let (mut pass, mut todo, mut fails) = (0, Vec::new(), Vec::new());
    for &(case, why) in CASES {
        match merlin_as::assemble(&source(case)) {
            Ok(_) => fails.push(format!("  {case:<26} assembled, but should fail: {why}")),
            Err(errs) => {
                if let Some(e) = errs.iter().find(|e| e.line != LINE) {
                    fails.push(format!(
                        "  {case:<26} error on line {}, want {LINE}: {}",
                        e.line, e.msg
                    ));
                } else if errs.iter().all(|e| e.msg.starts_with(UNIMPLEMENTED)) {
                    todo.push(case);
                } else {
                    pass += 1;
                }
            }
        }
    }
    println!(
        "errors: {pass}/{} rejected, {} not implemented yet",
        CASES.len(),
        todo.len()
    );
    if !todo.is_empty() {
        let mut m: Vec<&str> = todo
            .iter()
            .map(|s| s.split_whitespace().next().unwrap_or(""))
            .collect();
        m.sort();
        m.dedup();
        println!("still to do: {}", m.join(" "));
    }
    assert!(
        fails.is_empty(),
        "{} bad lines handled wrongly:\n{}",
        fails.len(),
        fails.join("\n")
    );
    if require_all {
        assert!(todo.is_empty(), "{} cases not implemented yet", todo.len());
    }
}

#[test]
fn encode_errors_progress() {
    run(false);
}

#[test]
#[ignore = "passes once the encoder is complete"]
fn encode_errors_complete() {
    run(true);
}

/// Errors from several lines are all reported, in line order.
#[test]
fn every_bad_line_is_reported() {
    let src = "start:\n    frobnicate a0\n    .text\n    frobnicate a1\n";
    let errs = merlin_as::assemble(src).unwrap_err();
    let lines: Vec<usize> = errs.iter().map(|e| e.line).collect();
    assert_eq!(lines, [2, 4]);
}

/// The CLI prints `file:line: error: message`, the format editors jump to.
#[test]
fn cli_reports_file_and_line() {
    let dir = std::env::temp_dir().join(format!("merlin-as-errors-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("bad.s");
    let output = dir.join("bad.o");
    std::fs::write(&input, "    .text\nstart:\n    frobnicate a0, a1\n").unwrap();
    let _ = std::fs::remove_file(&output);

    let out = Command::new(env!("CARGO_BIN_EXE_merlin-as"))
        .arg("-o")
        .arg(&output)
        .arg(&input)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let want = format!("{}:3: error: ", input.display());
    let wrote_object = output.exists();
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(out.status.code(), Some(1), "stderr: {stderr}");
    assert!(stderr.starts_with(&want), "want prefix {want:?}, got {stderr:?}");
    assert!(
        stderr.contains("frobnicate"),
        "message should name the mnemonic: {stderr:?}"
    );
    assert!(!wrote_object, "no object should be written on error");
}
