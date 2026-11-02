//! Bad instructions must be rejected with an error on their own source line,
//! never silently encoded with truncated fields.

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
    // Deliberately stricter than LLVM, which reads `a1` here as a symbol named
    // a1 (`lw rd, symbol`). In merlin-as register names are never symbols.
    ("lw a0, a1", "register where a memory operand or symbol belongs"),
    ("ecall a0", "operand on a no-operand instruction"),
    ("xcal.dbg 5", "xcal.dbg needs a register"),
    ("xcal.halt", "xcal.halt needs a register"),
    ("ret a0", "operand on ret"),
    // Relocation modifiers in the wrong place, and labels where only a constant
    // fits: each would otherwise be encoded with the wrong relocation, or none.
    ("addi a0, a0, %hi(target)", "%hi where a 12-bit field belongs"),
    ("lw a0, %hi(target)(a1)", "%hi as a load offset"),
    ("lui a0, %lo(target)", "%lo where a 20-bit field belongs"),
    ("lui a0, %pcrel_hi(target)", "lui needs %hi, not %pcrel_hi"),
    ("auipc a0, %hi(target)", "auipc needs %pcrel_hi, not %hi"),
    (
        "auipc a0, %pcrel_lo(target)",
        "%pcrel_lo where a 20-bit field belongs",
    ),
    ("beq a0, a1, %lo(target)", "modifier on a branch target"),
    ("jal ra, %lo(target)", "modifier on a jump target"),
    ("slli a0, a0, %lo(target)", "a shift amount must be a constant"),
    ("li a0, target", "li takes a constant (la loads an address)"),
    // `start:` labels this addi, which is not an auipc with %pcrel_hi
    // (llvm-mc: "could not find corresponding %pcrel_hi").
    (
        "addi a0, a0, %pcrel_lo(start)",
        "%pcrel_lo label without a %pcrel_hi",
    ),
    ("la a0, 0x1000", "la takes a symbol (li loads a number)"),
    ("call 0x100", "call takes a symbol"),
    ("lui a0, %hi(0x100000000)", "%hi of a number wider than 32 bits"),
    ("addi a0, a0, %pcrel_lo(0x10)", "%pcrel_lo of a number"),
    ("fence rw, ri", "fence set letters out of order"),
    ("fence rw", "fence takes zero or two sets"),
    ("sw a0, target", "a store to a symbol needs a scratch register"),
    // .insn: every field is range checked
    (".insn i 0x0b, 8, x0, a0, 0", ".insn funct3 above 7"),
    (".insn r 0x33, 0, 128, a0, a1, a2", ".insn funct7 above 127"),
    (
        ".insn r 0x08, 0, 0, a0, a1, a2",
        ".insn opcode of a 16-bit instruction",
    ),
    (".insn i 0x13, 0, a0, a1, 2048", ".insn immediate out of range"),
    (".insn s 0x23, 2, a0, a1", ".insn s needs offset(register)"),
    (
        ".insn u 0x37, a0, 0x100000",
        ".insn u immediate wider than 20 bits",
    ),
    (".insn b 0x63, 0, a0, a1, 3", ".insn b odd offset"),
    (".insn j 0x6f, ra, 8", ".insn format merlin-as does not support"),
    (".insn 0x0001", "raw .insn of a 16-bit instruction"),
    (".insn 2, 0x0001", "raw .insn length other than 4"),
    // Unknown mnemonics
    ("frobnicate a0, a1", "unknown mnemonic"),
    ("mulw a0, a1, a2", "RV64-only mnemonic"),
    ("c.addi a0, 1", "compressed mnemonic (no C extension)"),
    ("lr.w a0, (a1)", "A extension mnemonic (not in Excalibur)"),
];

fn source(case: &str) -> String {
    format!("    .text\nstart:\n    {case}\n")
}

#[test]
fn bad_instructions_are_rejected_on_their_line() {
    let mut fails = Vec::new();
    for &(case, why) in CASES {
        match merlin_as::assemble(&source(case)) {
            Ok(_) => fails.push(format!("  {case:<30} assembled, but should fail: {why}")),
            Err(errs) => {
                if let Some(e) = errs.iter().find(|e| e.line != LINE) {
                    fails.push(format!(
                        "  {case:<30} error on line {}, want {LINE}: {}",
                        e.line, e.msg
                    ));
                }
                // Shown with --nocapture, to review the wording.
                println!("{case:<30} {}", errs[0].msg);
            }
        }
    }
    assert!(
        fails.is_empty(),
        "{} bad lines handled wrongly:\n{}",
        fails.len(),
        fails.join("\n")
    );
}

/// An unknown mnemonic is named in the error.
#[test]
fn unknown_mnemonics_are_named() {
    for m in ["frobnicate", "mulw", "c.addi", "lr.w"] {
        let errs = merlin_as::assemble(&format!("    {m} a0, a1, a2\n")).unwrap_err();
        assert_eq!(errs[0].msg, format!("unknown instruction '{m}'"));
    }
}

/// Branches and jumps to labels in the same section are resolved by the
/// assembler (as clang does), so their range is checked there, and reported
/// on the line of the branch, not of the label.
#[test]
fn local_targets_out_of_range_are_reported_on_the_branch() {
    let src = "    .text\nstart:\n    beq a0, a1, far\n    .space 4092\nfar:\n    nop\n";
    let errs = merlin_as::assemble(src).unwrap_err();
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!(errs[0].line, 3);
    assert_eq!(
        errs[0].msg,
        "'far': branch target out of range: +4096 bytes (limit -4096..=+4094)"
    );
    // One word less is the largest forward branch.
    assert!(merlin_as::assemble(&src.replace("4092", "4088")).is_ok());

    // -1 MiB is the furthest a jal reaches back; +1 MiB is one byte pair too far.
    let src = "back:\n    .space 1048576\n    j back\n    jal far\n    .space 1048572\nfar:\n";
    let errs = merlin_as::assemble(src).unwrap_err();
    let got: Vec<(usize, &str)> = errs.iter().map(|e| (e.line, e.msg.as_str())).collect();
    assert_eq!(
        got,
        [(
            4,
            "'far': jump target out of range: +1048576 bytes (limit -1048576..=+1048574)"
        )]
    );

    // A misaligned label (after data in .text) can't be a branch target.
    let errs = merlin_as::assemble("    beqz a0, odd\n    .byte 0\nodd:\n").unwrap_err();
    assert_eq!(
        (errs[0].line, errs[0].msg.as_str()),
        (1, "'odd': branch target at an odd distance (+5 bytes)")
    );
}

/// `%pcrel_lo` must name the label of an auipc with `%pcrel_hi`, also when
/// the pair is resolved by the assembler.
#[test]
fn pcrel_lo_of_a_resolved_pair_still_needs_its_auipc() {
    let src = "1:  auipc a0, %pcrel_hi(data)\n    addi a0, a0, %pcrel_lo(2f)\n2:  nop\ndata:\n";
    let errs = merlin_as::assemble(src).unwrap_err();
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!(errs[0].line, 2);
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
