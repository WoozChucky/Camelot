//! Bad instructions must be rejected with an error on their own source line,
//! never silently encoded with truncated fields. `CASES` are RV32's; `CASES64`
//! are what RV64 changes: wider shift amounts, the RV64-only instructions'
//! ranges, and the CSRs and counter reads that exist on RV32 only.

use merlin_as::elf::riscv_attributes;
use merlin_as::{Options, Xlen};
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
    // Shift amounts: 0..=31 on RV32
    (
        "slli a0, a0, 32",
        "shamt above 31",
        "shift amount 32 out of range (0..=31)",
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
    // li takes a 32-bit value (signed or unsigned)
    (
        "li a0, 0x100000000",
        "li value wider than 32 bits",
        "li value 4294967296 does not fit in 32 bits",
    ),
    (
        "li a0, -2147483649",
        "li value below -2^31",
        "li value -2147483649 does not fit in 32 bits",
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
        "lui a0, %hi(0x100000000)",
        "%hi of a number wider than 32 bits",
        "%hi() value 4294967296 does not fit in 32 bits",
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
        "mulw a0, a1, a2",
        "RV64-only mnemonic",
        "unknown instruction 'mulw'",
    ),
    (
        "c.addi a0, 1",
        "compressed mnemonic (no C extension)",
        "unknown instruction 'c.addi'",
    ),
    (
        "amoadd.d a0, a1, (a2)",
        "RV64-only A mnemonic",
        "unknown instruction 'amoadd.d'",
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
    check_cases(CASES, Xlen::Rv32);
}

#[test]
fn bad_rv64_instructions_are_rejected_on_their_line() {
    check_cases(CASES64, Xlen::Rv64);
}

fn check_cases(cases: &[(&str, &str, &str)], xlen: Xlen) {
    let mut fails = Vec::new();
    for &(case, why, says) in cases {
        match merlin_as::assemble_with(&source(case), &Options { xlen }) {
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
    for m in ["frobnicate", "mulw", "c.addi", "amoadd.d"] {
        let errs = merlin_as::assemble(&format!("    {m} a0, a1, a2\n")).unwrap_err();
        assert_eq!(errs[0].msg, format!("unknown instruction '{m}'"));
    }
}

/// On RV32 an RV64-only mnemonic is just not an instruction: the same error
/// as any unknown mnemonic, whatever its operands. On RV64 each of the same
/// lines assembles.
#[test]
fn rv64_instructions_are_unknown_on_rv32() {
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
        let m = line.split_whitespace().next().unwrap();
        let errs = merlin_as::assemble(&format!("    {line}\n")).unwrap_err();
        assert_eq!(
            (errs.len(), errs[0].line, errs[0].msg.as_str()),
            (1, 1, format!("unknown instruction '{m}'").as_str()),
            "{line}"
        );
        let rv64 = Options { xlen: Xlen::Rv64 };
        assert!(merlin_as::assemble_with(line, &rv64).is_ok(), "{line} on RV64");
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

/// `-march=` picks the width for the whole file: `cycleh`, the high half of
/// the cycle counter, is a CSR on RV32 only, so the same source assembles for
/// one width and not the other, and an RV64 source that assembles is written
/// as an ELF64 object. An ISA merlin-as does not implement is a usage error
/// (status 2), not an assembly error.
#[test]
fn cli_takes_the_width_from_march() {
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
    let default = run(None, &cycleh);
    let rv32 = run(Some("-march=rv32ima_zicsr_zifencei"), &cycleh);
    let rv64 = run(Some("-march=rv64ima_zicsr_zifencei"), &cycleh);
    let other = run(Some("-march=rv64gc"), &cycleh);
    let rv64_ld = run(Some("-march=rv64ima_zicsr_zifencei"), &ld);
    let _ = std::fs::remove_dir_all(&dir);

    // Without -march and with RV32's: an ELF32 object (EI_CLASS 1).
    for (code, stderr, object) in [default, rv32] {
        assert_eq!((code, stderr.as_str()), (Some(0), ""));
        assert_eq!(object.expect("an object")[4], 1, "ELFCLASS32");
    }
    let (code, stderr, object) = rv64;
    assert_eq!((code, object), (Some(1), None), "stderr: {stderr}");
    assert!(
        stderr.starts_with(&format!("{}:1: error: ", cycleh.display())),
        "{stderr:?}"
    );
    assert!(stderr.contains("'cycleh'"), "{stderr:?}");
    let (code, stderr, object) = other;
    assert_eq!((code, object), (Some(2), None), "stderr: {stderr}");
    assert!(stderr.contains("unsupported -march 'rv64gc'"), "{stderr:?}");

    // RV64's ld: an ELF64 object (EI_CLASS 2) whose .riscv.attributes names
    // the RV64 ISA, as clang's does for the same -march.
    let (code, stderr, object) = rv64_ld;
    assert_eq!((code, stderr.as_str()), (Some(0), ""));
    let bytes = object.expect("an object");
    assert_eq!(bytes[4], 2, "ELFCLASS64");
    assert_eq!(attributes(&bytes), riscv_attributes(merlin_as::elf::ARCH_RV64));
}

/// The contents of an object's `.riscv.attributes`.
fn attributes(object: &[u8]) -> Vec<u8> {
    let file = elf::Elf::parse(object).expect("parses");
    let section = file.sections.iter().find(|s| s.name == ".riscv.attributes");
    file.section_data(section.expect(".riscv.attributes"))
        .unwrap()
        .to_vec()
}

/// The width and the ISA come only from the options: `.option` and
/// `.attribute`, which change them in LLVM (`.option arch`, `.option rvc`,
/// `.attribute arch`, or by its number, 5), are ignored. A source that
/// claims the other width assembles for its own: RV64's instructions stay
/// unknown on RV32 and RV32's counter halves on RV64, and the object's
/// `.riscv.attributes` is still the ISA of its width.
#[test]
fn option_and_attribute_leave_the_width_alone() {
    // Four lines that claim `isa`, so the instruction after them is line 5.
    let claim = |isa: &str, line: &str| {
        format!(
            "    .option arch, {isa}\n    .option rvc\n    .attribute arch, \"{isa}\"\n    \
             .attribute 5, \"{isa}\"\n    {line}\n"
        )
    };
    // (width, the other width's ISA, its instruction and why it is refused,
    // an instruction of this width).
    let cases = [
        (
            Xlen::Rv32,
            "rv64gc",
            "ld a0, 0(a1)",
            "unknown instruction 'ld'",
            "rdcycleh a0",
        ),
        (
            Xlen::Rv64,
            "rv32gc",
            "rdcycleh a0",
            "'rdcycleh' exists only on RV32 (on RV64, rdcycle reads all 64 bits)",
            "ld a0, 0(a1)",
        ),
    ];
    for (xlen, claimed, theirs, why, ours) in cases {
        let options = Options { xlen };
        let errs = merlin_as::assemble_with(&claim(claimed, theirs), &options).unwrap_err();
        assert_eq!(
            (errs.len(), errs[0].line, errs[0].msg.as_str()),
            (1, 5, why),
            "{xlen:?}"
        );
        let object = merlin_as::assemble_to_elf_with(&claim(claimed, ours), &options);
        let object = object.unwrap_or_else(|e| panic!("{xlen:?}: {e:?}"));
        let class = match xlen {
            Xlen::Rv32 => 1,
            Xlen::Rv64 => 2,
        };
        assert_eq!(object[4], class, "{xlen:?}: EI_CLASS");
        assert_eq!(
            attributes(&object),
            riscv_attributes(merlin_as::elf::arch(xlen)),
            "{xlen:?}"
        );
    }
}
