//! Bad instructions must be rejected with an error on their own source line,
//! never silently encoded with truncated fields. `CASES` are the base set's;
//! `CASES64` RV64's own: the shift amounts, the 32-bit and doubleword
//! instructions' ranges, and the CSRs and counter reads that exist on RV32 only.

use merlin_as::elf::riscv_attributes;
use std::process::Command;

/// Each case is assembled on line 3, after two lines that cannot fail.
const LINE: usize = 3;

/// (source line, why it must fail, a piece of the error message that says so).
const CASES: &[(&str, &str, &str)] = &[
    // 12-bit signed immediates: -2048..=2047
    (
        "addi a0, a0, 2048",
        "I-type immediate above 2047",
        "immediate 2048 out of range (-2048..=2047)",
    ),
    (
        "addi a0, a0, -2049",
        "I-type immediate below -2048",
        "immediate -2049 out of range",
    ),
    (
        "slti a0, a0, 0x1000",
        "I-type immediate too wide",
        "immediate 4096 out of range",
    ),
    (
        "jalr ra, 2048(t0)",
        "jalr offset out of range",
        "immediate 2048 out of range",
    ),
    (
        "lw a0, 2048(sp)",
        "load offset out of range",
        "immediate 2048 out of range",
    ),
    (
        "lbu a0, -2049(sp)",
        "load offset out of range",
        "immediate -2049 out of range",
    ),
    (
        "sw a0, 2048(sp)",
        "store offset out of range",
        "immediate 2048 out of range",
    ),
    (
        "sb a0, -2049(sp)",
        "store offset out of range",
        "immediate -2049 out of range",
    ),
    (
        "srli a0, a0, -1",
        "negative shamt",
        "shift amount -1 out of range",
    ),
    (
        "srai a0, a0, 64",
        "shamt above 31",
        "shift amount 64 out of range",
    ),
    // Branches: even offsets in -4096..=4094
    (
        "beq a0, a1, 4096",
        "branch offset above 4094",
        "branch offset 4096 out of range (-4096..=4094)",
    ),
    (
        "bne a0, a1, -4098",
        "branch offset below -4096",
        "branch offset -4098 out of range",
    ),
    (
        "blt a0, a1, 3",
        "odd branch offset",
        "branch offset 3 must be even",
    ),
    (
        "beqz a0, 4096",
        "pseudo-branch offset out of range",
        "branch offset 4096 out of range",
    ),
    // Jumps: even offsets in -1MiB..=1MiB-2
    (
        "jal ra, 1048576",
        "jal offset above 1048574",
        "jump offset 1048576 out of range (-1048576..=1048574)",
    ),
    (
        "jal ra, -1048578",
        "jal offset below -1048576",
        "jump offset -1048578 out of range",
    ),
    ("jal ra, 5", "odd jal offset", "jump offset 5 must be even"),
    (
        "j 1048576",
        "j offset out of range",
        "jump offset 1048576 out of range",
    ),
    // U-type: 20-bit field
    (
        "lui a0, 0x100000",
        "lui immediate wider than 20 bits",
        "upper immediate 0x100000 out of range (0..=0xfffff)",
    ),
    (
        "lui a0, -1",
        "negative lui immediate",
        "upper immediate -0x1 out of range",
    ),
    (
        "auipc a0, 0x100000",
        "auipc immediate wider than 20 bits",
        "upper immediate 0x100000 out of range",
    ),
    // CSRs: 12-bit address, 5-bit zero-extended immediate
    (
        "csrrw a0, 4096, a1",
        "CSR number above 0xfff",
        "CSR number 4096 out of range (0..=0xfff)",
    ),
    (
        "csrrw a0, nosuchcsr, a1",
        "unknown CSR name",
        "unknown CSR 'nosuchcsr'",
    ),
    (
        "csrrwi a0, mstatus, 32",
        "CSR immediate above 31",
        "CSR immediate 32 out of range (0..=31)",
    ),
    (
        "csrrsi a0, mstatus, -1",
        "negative CSR immediate",
        "CSR immediate -1 out of range",
    ),
    (
        "csrwi mstatus, 32",
        "CSR immediate above 31",
        "CSR immediate 32 out of range",
    ),
    // Wrong operand count or kind
    ("add a0, a1", "missing operand", "usage: add rd, rs1, rs2"),
    ("add a0, a1, a2, a3", "extra operand", "usage: add rd, rs1, rs2"),
    (
        "addi a0, a1, a2",
        "register where an immediate belongs",
        "expected an immediate, found a register",
    ),
    (
        "sub a0, a1, 5",
        "immediate where a register belongs (sub has no immediate form)",
        "expected a register, found an immediate",
    ),
    ("sgt a0, a1", "missing operand", "usage: sgt rd, rs, rt"),
    (
        "sgtu a0, a1, 5",
        "immediate where a register belongs (sgtu is sltu, not sltiu)",
        "expected a register, found an immediate",
    ),
    // Deliberately stricter than LLVM, which reads `a1` here as a symbol named
    // a1 (`lw rd, symbol`). In merlin-as register names are never symbols.
    (
        "lw a0, a1",
        "register where a memory operand or symbol belongs",
        "expected offset(register) or a symbol, found a register",
    ),
    (
        "ecall a0",
        "operand on a no-operand instruction",
        "ecall takes no operands",
    ),
    (
        "xcal.dbg 5",
        "xcal.dbg needs a register",
        "expected a register, found an immediate",
    ),
    ("xcal.halt", "xcal.halt needs a register", "usage: xcal.halt rs1"),
    ("ret a0", "operand on ret", "ret takes no operands"),
    // Relocation modifiers in the wrong place, and labels where only a constant
    // fits: each would otherwise be encoded with the wrong relocation, or none.
    (
        "addi a0, a0, %hi(target)",
        "%hi where a 12-bit field belongs",
        "%hi() is a 20-bit upper part",
    ),
    (
        "lw a0, %hi(target)(a1)",
        "%hi as a load offset",
        "%hi() is a 20-bit upper part",
    ),
    (
        "lui a0, %lo(target)",
        "%lo where a 20-bit field belongs",
        "lui takes a number in 0..=0xfffff or %hi(sym)",
    ),
    (
        "lui a0, %pcrel_hi(target)",
        "lui needs %hi, not %pcrel_hi",
        "lui takes a number in 0..=0xfffff or %hi(sym)",
    ),
    (
        "auipc a0, %hi(target)",
        "auipc needs %pcrel_hi, not %hi",
        "auipc takes a number in 0..=0xfffff or %pcrel_hi(sym)",
    ),
    (
        "auipc a0, %pcrel_lo(target)",
        "%pcrel_lo where a 20-bit field belongs",
        "auipc takes a number in 0..=0xfffff or %pcrel_hi(sym)",
    ),
    (
        "beq a0, a1, %lo(target)",
        "modifier on a branch target",
        "a branch target can't use %lo()",
    ),
    (
        "jal ra, %lo(target)",
        "modifier on a jump target",
        "a jump target can't use %lo()",
    ),
    (
        "slli a0, a0, %lo(target)",
        "a shift amount must be a constant",
        "shift amount must be a number",
    ),
    (
        "li a0, target",
        "li takes a constant (la loads an address)",
        "la loads an address",
    ),
    (
        "li a0, start",
        "li of a label",
        "li takes a constant (use la to load an address)",
    ),
    (
        "addi a0, a0, start",
        "a label as a 12-bit immediate",
        "a label can't be a 12-bit immediate",
    ),
    // `start:` labels this addi, which is not an auipc with %pcrel_hi
    // (llvm-mc: "could not find corresponding %pcrel_hi").
    (
        "addi a0, a0, %pcrel_lo(start)",
        "%pcrel_lo label without a %pcrel_hi",
        "%pcrel_lo(start): no auipc with %pcrel_hi at that label",
    ),
    (
        "la a0, 0x1000",
        "la takes a symbol (li loads a number)",
        "la needs a symbol, not a number",
    ),
    (
        "call 0x100",
        "call takes a symbol",
        "call needs a symbol, not a number",
    ),
    (
        "addi a0, a0, %pcrel_lo(0x10)",
        "%pcrel_lo of a number",
        "%pcrel_lo() needs a symbol, not a number",
    ),
    (
        "fence rw, ri",
        "fence set letters out of order",
        "letters taken in order from 'iorw'",
    ),
    (
        "fence rw",
        "fence takes zero or two sets",
        "usage: fence [pred, succ]",
    ),
    (
        "sw a0, target",
        "a store to a symbol needs a scratch register",
        "a store to a symbol needs a scratch register",
    ),
    // .insn: every field is range checked
    (
        ".insn i 0x0b, 8, x0, a0, 0",
        ".insn funct3 above 7",
        "funct3 8 out of range (0..=7)",
    ),
    (
        ".insn r 0x33, 0, 128, a0, a1, a2",
        ".insn funct7 above 127",
        "funct7 128 out of range (0..=127)",
    ),
    (
        ".insn r 0x08, 0, 0, a0, a1, a2",
        ".insn opcode of a 16-bit instruction",
        "opcode 0x8 is not a 32-bit opcode",
    ),
    (
        ".insn i 0x13, 0, a0, a1, 2048",
        ".insn immediate out of range",
        "immediate 2048 out of range",
    ),
    (
        ".insn s 0x23, 2, a0, a1",
        ".insn s needs offset(register)",
        "expected offset(register), found a register",
    ),
    (
        ".insn u 0x37, a0, 0x100000",
        ".insn u immediate wider than 20 bits",
        "upper immediate 0x100000 out of range",
    ),
    (
        ".insn b 0x63, 0, a0, a1, 3",
        ".insn b odd offset",
        "branch offset 3 must be even",
    ),
    (
        ".insn j 0x6f, ra, 8",
        ".insn format merlin-as does not support",
        "unknown .insn format 'j'",
    ),
    (
        ".insn 0x0001",
        "raw .insn of a 16-bit instruction",
        "0x00000001 is not a 32-bit instruction",
    ),
    (
        ".insn 2, 0x0001",
        "raw .insn length other than 4",
        ".insn length must be 4",
    ),
    // Unknown mnemonics
    (
        "frobnicate a0, a1",
        "unknown mnemonic",
        "unknown instruction 'frobnicate'",
    ),
    (
        "mulhw a0, a1, a2",
        "no such M mnemonic (mulh has no 32-bit form)",
        "unknown instruction 'mulhw'",
    ),
    (
        "c.addi a0, 1",
        "compressed mnemonic (no C extension)",
        "unknown instruction 'c.addi'",
    ),
    (
        "amoadd.q a0, a1, (a2)",
        "a 128-bit A mnemonic (RV128)",
        "unknown instruction 'amoadd.q'",
    ),
    (
        "amocas.w a0, a1, (a2)",
        "Zacas mnemonic (not in Excalibur)",
        "unknown instruction 'amocas.w'",
    ),
    (
        "amoswap.w.rlaq a0, a1, (a2)",
        "ordering suffix out of order",
        "unknown instruction 'amoswap.w.rlaq'",
    ),
    (
        "lr.w.aq.rl a0, (a1)",
        "ordering suffix split in two",
        "unknown instruction 'lr.w.aq.rl'",
    ),
    // A extension operands
    (
        "lr.w a0, 4(a1)",
        "A instructions have no offset field",
        "lr.w has no offset field: write the address as (rs1)",
    ),
    (
        "amoadd.w.aqrl a0, a1, -4(a2)",
        "A instructions have no offset field",
        "amoadd.w.aqrl has no offset field",
    ),
    // Only a literal 0, as in LLVM (an integer token, not an expression).
    (
        "lr.w a0, -0(a1)",
        "an offset expression equal to 0",
        "lr.w has no offset field",
    ),
    (
        "lr.w a0, 1-1(a1)",
        "an offset expression equal to 0",
        "lr.w has no offset field",
    ),
    (
        "sc.w a0, a1, 0+0(a2)",
        "an offset expression equal to 0",
        "sc.w has no offset field",
    ),
    (
        "lr.w a0, +0(a1)",
        "a unary plus folds to 0 but is not an integer token",
        "lr.w has no offset field",
    ),
    (
        "lr.w a0, (0)(a1)",
        "a parenthesized 0 is an expression",
        "lr.w has no offset field",
    ),
    (
        "lr.w a0, ((0))(a1)",
        "a doubly parenthesized 0 is an expression",
        "lr.w has no offset field",
    ),
    (
        "amoswap.w a0, a1, +(0)(a2)",
        "a unary plus on a parenthesized 0",
        "amoswap.w has no offset field",
    ),
    (
        "sc.w a0, a1, a2",
        "address given as a plain register",
        "expected offset(register), found a register: sc.w takes its address as (rs1)",
    ),
    ("lr.w a0, a1, (a2)", "lr.w has no rs2", "usage: lr.w rd, (rs1)"),
    (
        "sc.w a0, (a1)",
        "sc.w without its source register",
        "usage: sc.w rd, rs2, (rs1)",
    ),
    (
        "amoor.w.rl a0, 5, (a2)",
        "rs2 must be a register",
        "expected a register, found an immediate",
    ),
];

/// The same for RV64.
const CASES64: &[(&str, &str, &str)] = &[
    // Shift amounts: 0..=63, and 0..=31 for the W shifts
    (
        "slli a0, a0, 64",
        "shamt above 63",
        "shift amount 64 out of range (0..=63)",
    ),
    (
        "srai a0, a0, -1",
        "negative shamt",
        "shift amount -1 out of range (0..=63)",
    ),
    (
        "slliw a0, a0, 32",
        "W shift amount above 31",
        "shift amount 32 out of range (0..=31)",
    ),
    (
        "sraiw a0, a1, -1",
        "negative W shift amount",
        "shift amount -1 out of range (0..=31)",
    ),
    (
        "srliw a0, a1, x",
        "a W shift amount must be a constant",
        "shift amount must be a number",
    ),
    // addiw, ld, lwu and sd: 12-bit signed immediates, as addi, lw and sw
    (
        "addiw a0, a1, 2048",
        "addiw immediate above 2047",
        "immediate 2048 out of range (-2048..=2047)",
    ),
    (
        "addiw a0, a1, -2049",
        "addiw immediate below -2048",
        "immediate -2049 out of range",
    ),
    (
        "ld a0, 2048(sp)",
        "ld offset out of range",
        "immediate 2048 out of range",
    ),
    (
        "lwu a0, -2049(sp)",
        "lwu offset out of range",
        "immediate -2049 out of range",
    ),
    (
        "sd a0, 2048(sp)",
        "sd offset out of range",
        "immediate 2048 out of range",
    ),
    // The W operations take registers only (LLVM reads `addw a0, a1, 5` as
    // addiw; merlin-as, as for `add a0, a1, 5`, does not).
    (
        "addw a0, a1, 5",
        "addw with an immediate",
        "expected a register, found an immediate",
    ),
    ("sext.w a0", "sext.w without its source", "usage: sext.w rd, rs"),
    // The .d atomics: no offset, ordering suffixes in order
    (
        "lr.d a0, 4(a1)",
        "A instructions have no offset field",
        "lr.d has no offset field: write the address as (rs1)",
    ),
    (
        "amoadd.d.rlaq a0, a1, (a2)",
        "ordering suffix out of order",
        "unknown instruction 'amoadd.d.rlaq'",
    ),
    // RV32 only: the high halves of the counters, and the CSRs that hold
    // the high half of a 64-bit register on RV32
    (
        "rdcycleh a0",
        "rdcycleh is RV32 only",
        "'rdcycleh' exists only on RV32 (on RV64, rdcycle reads all 64 bits)",
    ),
    (
        "rdtimeh a0",
        "rdtimeh is RV32 only",
        "'rdtimeh' exists only on RV32 (on RV64, rdtime reads all 64 bits)",
    ),
    (
        "rdinstreth a0",
        "rdinstreth is RV32 only",
        "'rdinstreth' exists only on RV32 (on RV64, rdinstret reads all 64 bits)",
    ),
    (
        "csrr a0, cycleh",
        "cycleh is RV32 only",
        "CSR 'cycleh' exists only on RV32",
    ),
    (
        "csrr a0, timeh",
        "timeh is RV32 only",
        "CSR 'timeh' exists only on RV32",
    ),
    (
        "csrr a0, instreth",
        "instreth is RV32 only",
        "CSR 'instreth' exists only on RV32",
    ),
    (
        "csrw mcycleh, a0",
        "mcycleh is RV32 only",
        "CSR 'mcycleh' exists only on RV32",
    ),
    (
        "csrw minstreth, a0",
        "minstreth is RV32 only",
        "CSR 'minstreth' exists only on RV32",
    ),
    (
        "csrs mstatush, a0",
        "mstatush is RV32 only",
        "CSR 'mstatush' exists only on RV32",
    ),
    (
        "csrc menvcfgh, a0",
        "menvcfgh is RV32 only",
        "CSR 'menvcfgh' exists only on RV32",
    ),
    (
        "csrr a0, hpmcounter3h",
        "hpmcounter3h is RV32 only",
        "CSR 'hpmcounter3h' exists only on RV32",
    ),
    (
        "csrrw a0, mhpmcounter31h, a1",
        "mhpmcounter31h is RV32 only",
        "CSR 'mhpmcounter31h' exists only on RV32",
    ),
    (
        "csrr a0, pmpcfg1",
        "pmpcfg1 is RV32 only (pmpcfg0 holds 8 entries on RV64)",
        "CSR 'pmpcfg1' exists only on RV32",
    ),
    (
        "csrrsi a0, pmpcfg3, 1",
        "pmpcfg3 is RV32 only",
        "CSR 'pmpcfg3' exists only on RV32",
    ),
];

fn source(case: &str) -> String {
    format!("    .text\nstart:\n    {case}\n")
}

#[test]
fn bad_instructions_are_rejected_on_their_line() {
    check_cases(CASES);
}

#[test]
fn bad_rv64_instructions_are_rejected_on_their_line() {
    check_cases(CASES64);
}

fn check_cases(cases: &[(&str, &str, &str)]) {
    let mut fails = Vec::new();
    for &(case, why, says) in cases {
        match merlin_as::assemble(&source(case)) {
            Ok(_) => fails.push(format!("  {case:<30} assembled, but should fail: {why}")),
            Err(errs) => {
                if let Some(e) = errs.iter().find(|e| e.line != LINE) {
                    fails.push(format!(
                        "  {case:<30} error on line {}, want {LINE}: {}",
                        e.line, e.msg
                    ));
                } else if !errs[0].msg.contains(says) {
                    fails.push(format!(
                        "  {case:<30} error '{}', want one saying '{says}' ({why})",
                        errs[0].msg
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

/// A constant used before its `.equ` (which the spec forbids) is reported as
/// such, not as an address given to `li`.
#[test]
fn constants_used_before_their_equ_are_named() {
    let errs = merlin_as::assemble("    li a0, LATER\n    addi a0, a0, LATER\n.equ LATER, 8\n").unwrap_err();
    let got: Vec<(usize, &str)> = errs.iter().map(|e| (e.line, e.msg.as_str())).collect();
    let says = "'LATER' is not a constant (yet): .equ/.set constants must be defined before use";
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0].0, 1);
    assert!(got[0].1.starts_with(says), "{}", got[0].1);
    assert_eq!(got[1], (2, says));
    // Defined first, it is a plain number.
    assert!(merlin_as::assemble(".equ LATER, 8\n    li a0, LATER\n    addi a0, a0, LATER\n").is_ok());
}

/// An A instruction's offset is a literal 0 or nothing, as in LLVM: a
/// constant symbol equal to 0 is refused like any other expression.
#[test]
fn atomic_offsets_are_a_literal_zero() {
    let errs = merlin_as::assemble(".equ Z, 0\n    amoadd.w a0, a1, Z(a2)\n").unwrap_err();
    assert_eq!(
        (errs[0].line, errs[0].msg.as_str()),
        (2, "amoadd.w has no offset field: write the address as (rs1)")
    );
    let same = |src: &str| merlin_as::assemble(src).unwrap().sections[0].data.clone();
    let want = same("    lr.w a0, (a1)\n");
    assert_eq!(want, 0x1005_a52fu32.to_le_bytes());
    for src in [
        "    lr.w a0, 0(a1)\n",
        "    lr.w a0, 0x0(a1)\n",
        "    lr.w a0, 0 (a1)\n",
        "    lr.w a0, ( a1 )\n",
    ] {
        assert_eq!(same(src), want, "{src:?}");
    }
}

/// An unknown mnemonic is named in the error.
#[test]
fn unknown_mnemonics_are_named() {
    for m in ["frobnicate", "mulhw", "c.addi", "amoadd.q"] {
        let errs = merlin_as::assemble(&format!("    {m} a0, a1, a2\n")).unwrap_err();
        assert_eq!(errs[0].msg, format!("unknown instruction '{m}'"));
    }
}

/// RV64's own instructions (the 32-bit operations, the doubleword loads,
/// stores and atomics, and their pseudo-instructions) assemble.
#[test]
fn rv64_instructions_assemble() {
    let lines = [
        "addiw a0, a1, 1",
        "slliw a0, a1, 1",
        "srliw a0, a1, 1",
        "sraiw a0, a1, 1",
        "addw a0, a1, a2",
        "subw a0, a1, a2",
        "sllw a0, a1, a2",
        "srlw a0, a1, a2",
        "sraw a0, a1, a2",
        "mulw a0, a1, a2",
        "divw a0, a1, a2",
        "divuw a0, a1, a2",
        "remw a0, a1, a2",
        "remuw a0, a1, a2",
        "ld a0, 8(a1)",
        "lwu a0, 8(a1)",
        "sd a0, 8(a1)",
        "ld a0, target",
        "lwu a0, target",
        "sd a0, target, t0",
        "lr.d a0, (a1)",
        "sc.d.aq a0, a1, (a2)",
        "amoswap.d a0, a1, (a2)",
        "amoadd.d.rl a0, a1, (a2)",
        "amoxor.d a0, a1, (a2)",
        "amoand.d a0, a1, (a2)",
        "amoor.d a0, a1, (a2)",
        "amomin.d a0, a1, (a2)",
        "amomax.d a0, a1, (a2)",
        "amominu.d a0, a1, (a2)",
        "amomaxu.d.aqrl a0, a1, (a2)",
        "sext.w a0, a1",
        "negw a0, a1",
        "zext.w a0, a1",
    ];
    for line in lines {
        assert!(merlin_as::assemble(line).is_ok(), "{line}");
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

/// `-march=` names the ISA, RV64's, which is also the default: an RV64 source
/// is written as an ELF64 object, and `cycleh`, the high half of the cycle
/// counter, a CSR on RV32 only, is an error. An ISA merlin-as does not
/// implement, an RV32 one among them, is a usage error (status 2), not an
/// assembly error.
#[test]
fn cli_takes_the_isa_from_march() {
    let dir = std::env::temp_dir().join(format!("merlin-as-march-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cycleh = dir.join("cycleh.s");
    std::fs::write(&cycleh, "    csrr a0, cycleh\n").unwrap();
    let ld = dir.join("ld.s");
    std::fs::write(&ld, "    ld a0, 0(a1)\n").unwrap();
    let output = dir.join("out.o");
    // (exit status, stderr, the object if one was written).
    let run = |march: Option<&str>, input: &std::path::Path| {
        let _ = std::fs::remove_file(&output);
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_merlin-as"));
        cmd.args(march).arg("-o").arg(&output).arg(input);
        let out = cmd.output().unwrap();
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            std::fs::read(&output).ok(),
        )
    };
    let cycleh_rv64 = run(Some("-march=rv64ima_zicsr_zifencei"), &cycleh);
    let cycleh_default = run(None, &cycleh);
    let rv32 = run(Some("-march=rv32ima_zicsr_zifencei"), &ld);
    let other = run(Some("-march=rv64gc"), &ld);
    let rv64_ld = run(Some("-march=rv64ima_zicsr_zifencei"), &ld);
    let default_ld = run(None, &ld);
    let _ = std::fs::remove_dir_all(&dir);

    for (code, stderr, object) in [cycleh_rv64, cycleh_default] {
        assert_eq!((code, object), (Some(1), None), "stderr: {stderr}");
        assert!(
            stderr.starts_with(&format!("{}:1: error: ", cycleh.display())),
            "{stderr:?}"
        );
        assert!(stderr.contains("'cycleh'"), "{stderr:?}");
    }
    for (code, stderr, object, march) in [
        (rv32.0, rv32.1, rv32.2, "rv32ima_zicsr_zifencei"),
        (other.0, other.1, other.2, "rv64gc"),
    ] {
        assert_eq!((code, object), (Some(2), None), "stderr: {stderr}");
        assert_eq!(
            stderr.trim_end(),
            format!("merlin-as: unsupported -march '{march}' (merlin-as takes rv64ima_zicsr_zifencei)")
        );
    }

    // ld: an ELF64 object (EI_CLASS 2) whose .riscv.attributes names the RV64
    // ISA, as clang's does for the same -march, with -march or without.
    for (code, stderr, object) in [rv64_ld, default_ld] {
        assert_eq!((code, stderr.as_str()), (Some(0), ""));
        let bytes = object.expect("an object");
        assert_eq!(bytes[4], 2, "ELFCLASS64");
        assert_eq!(attributes(&bytes), riscv_attributes(merlin_as::elf::ARCH));
    }
}

/// The contents of an object's `.riscv.attributes`.
fn attributes(object: &[u8]) -> Vec<u8> {
    let file = elf::Elf::parse(object).expect("parses");
    let section = file.sections.iter().find(|s| s.name == ".riscv.attributes");
    file.section_data(section.expect(".riscv.attributes"))
        .unwrap()
        .to_vec()
}

/// The ISA comes only from the options. In LLVM `.option arch`,
/// `.option rvc`, `.attribute arch` and `.attribute 5` change the ISA (a new
/// `$x<isa>`, another `Tag_RISCV_arch`); merlin-as has no other ISA than
/// `-march`'s, so one that names another is an error on its line, and one
/// that names the same (clang's `-S` output: `.attribute 5, "rv64i2p1_..."`,
/// or `.option arch, -a`, which changes nothing in LLVM either) is accepted
/// and changes nothing: the object's `.riscv.attributes` is -march's ISA.
#[test]
fn option_and_attribute_leave_the_isa_alone() {
    let (march, other) = ("rv64ima_zicsr_zifencei", "rv32ima_zicsr_zifencei");
    let arch = merlin_as::elf::ARCH;
    // The same ISA, in every spelling: no error, and -march's attributes.
    let same = format!(
        "    .option arch, {march}\n    .option arch, +m, -c\n    .option arch, -a\n    .option push\n    \
         .option norvc\n    .option pop\n    .attribute arch, \"{arch}\"\n    .attribute 5, \"{march}\"\n    \
         .attribute 4, 16\n    nop\n"
    );
    let object = merlin_as::assemble_to_elf(&same).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(object[4], 2, "EI_CLASS: ELFCLASS64");
    assert_eq!(attributes(&object), riscv_attributes(arch));

    // Another ISA, each line an error of its own; RV32 among them, whose
    // `rdcycleh` stays an error after it.
    let another = [
        format!(".option arch, {other}"),
        ".option arch, +zicond".into(),
        ".option arch, -m".into(),
        ".option arch, rv64imac_zicsr_zifencei".to_string(),
        ".option rvc".into(),
        format!(".attribute arch, \"{other}\""),
        ".attribute 5, \"rv64i2p1_m2p0\"".to_string(),
    ];
    let src: String = another.iter().map(|l| format!("    {l}\n")).collect();
    let errs = merlin_as::assemble(&format!("{src}    rdcycleh a0\n")).unwrap_err();
    let lines: Vec<usize> = errs.iter().map(|e| e.line).collect();
    assert_eq!(lines, (1..=another.len() + 1).collect::<Vec<_>>(), "{errs:?}");
    for (e, line) in errs.iter().zip(&another) {
        let directive = if line.starts_with(".option rvc") {
            ".option rvc"
        } else if line.starts_with(".option") {
            ".option arch"
        } else {
            ".attribute arch"
        };
        let msg =
            format!("{directive}: merlin-as assembles only -march's ISA ({arch}), and this names another");
        assert_eq!(e.msg, msg, "{line}");
    }
    // LLVM's own errors.
    let errs = merlin_as::assemble("    .option arch, -zmmul\n    .option arch, +m2p0\n").unwrap_err();
    let msgs: Vec<&str> = errs.iter().map(|e| e.msg.as_str()).collect();
    assert_eq!(
        msgs,
        [
            "can't disable zmmul extension; m extension requires zmmul extension",
            "extension version number parsing not currently implemented"
        ]
    );
}
