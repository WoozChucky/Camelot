//! # The encoder
//!
//! Turns one instruction (mnemonic + parsed operands) into machine words.
//! Everything around it (lexing, parsing, labels, sections, directives,
//! relocations, ELF output) lives elsewhere; this module is the whole ISA:
//! RV32I, M, A, Zicsr, Zifencei, the privileged instructions, Xcal (spec section
//! 9), the standard pseudo-instructions and the `.insn` directive.
//!
//! ## Contract with the assembler driver ([`crate::asm`])
//!
//! * [`encode`] returns one [`Emit`] per 32-bit word, in order.
//!   Pseudo-instructions return several (`li` with a big constant, `call`,
//!   `la`, ...).
//! * The number of words never depends on a label's address (the assembler is
//!   single pass). It may depend on constants: `li a0, 5` is 1 word,
//!   `li a0, 0x12345678` is 2.
//! * An operand that refers to a label becomes a fixup: the immediate bits are
//!   left zero and the [`Emit`] carries the relocation that fills them in. The
//!   driver later resolves the ones clang would resolve (pc-relative references
//!   to local labels in the same section) and turns the rest into ELF
//!   relocations.
//! * The low half of a pc-relative pair (`la`, `lw a0, sym`) targets the
//!   `auipc` it belongs to, `Target::Auipc(index_of_the_auipc_word)`, as the
//!   psABI requires (`R_RISCV_PCREL_LO12_*` points at the auipc, not at the
//!   symbol).
//! * Anything wrong (operand count or kind, immediate out of range, unknown
//!   mnemonic) is an `Err(message)`; the driver adds the line number.
//!
//! ## Reading order
//!
//! 1. The six field packers ([`r_type`] ... [`j_type`]) and the immediate
//!    scramblers they use ([`i_imm`] ... [`j_imm`]). They are the mirror image
//!    of the `imm_*` functions in `crates/excalibur/src/decode.rs`.
//! 2. [`hi_lo`], the split of a 32-bit value into a 20-bit upper part and a
//!    sign-extended 12-bit lower part, used by `li`, `%hi`/`%lo` and the
//!    linker-style patching in `asm.rs`.
//! 3. Operand evaluation (`Imm`, `imm12`, `imm20`, `branch_target`, ...):
//!    where range checks and relocation modifiers are handled.
//! 4. [`encode`] itself: real instructions grouped by format, then the
//!    pseudo-instructions, then `.insn`.

use crate::ast::{Expr, Modifier, Operand};
use crate::reloc::{Emit, Fixup, RelocKind, Target};
use crate::Ctx;
use excalibur::{csr, Reg};

// Major opcodes, bits [6:0] (spec section 4): the decoder's own constants.
// custom-0 holds the Xcal instructions (spec section 9).
use excalibur::opcode::{
    OP_AMO as AMO, OP_AUIPC as AUIPC, OP_BRANCH as BRANCH, OP_CUSTOM0 as CUSTOM_0, OP_IMM, OP_JAL as JAL,
    OP_JALR as JALR, OP_LOAD as LOAD, OP_LUI as LUI, OP_MISC_MEM as MISC_MEM, OP_OP as OP, OP_STORE as STORE,
    OP_SYSTEM as SYSTEM,
};

// funct3 values the pseudo-instructions need by name.
const ADDI: u32 = 0b000;
const SLTIU: u32 = 0b011;
const XORI: u32 = 0b100;
const ANDI: u32 = 0b111;
const SLLI: u32 = 0b001;
const BEQ: u32 = 0b000;
const BNE: u32 = 0b001;
const BLT: u32 = 0b100;
const BGE: u32 = 0b101;
const BLTU: u32 = 0b110;
const BGEU: u32 = 0b111;
const CSRRW: u32 = 0b001;
const CSRRS: u32 = 0b010;
const CSRRC: u32 = 0b011;
/// The immediate CSR forms (`csrrwi` ...) are the register forms with bit 2 of funct3 set.
const CSR_IMM: u32 = 0b100;

// Registers the pseudo-instructions use.
const ZERO: Reg = 0;
const RA: Reg = 1;
const T1: Reg = 6;

// ===================================================================
// Field packers
//
// Every RV32 instruction is one of six formats. They all keep opcode, rd,
// rs1, rs2 and funct3 in the same bit positions, so decoding can read
// registers before knowing the format; the price is that immediates are
// split up ("scrambled") differently in each format. Each `x_imm` function
// below takes an immediate and returns just the bits it occupies in an
// X-type word, so `asm.rs` can also use them to patch a resolved label into
// a word the encoder left zero.
// ===================================================================

/// R-type: register-register operations (OP, including the M extension, and
/// the A extension, whose funct7 is funct5 plus the aq and rl bits).
///
/// ```text
///  31     25 24  20 19  15 14  12 11   7 6      0
/// | funct7  | rs2  | rs1  |funct3|  rd  | opcode |
/// ```
pub fn r_type(opcode: u32, funct3: u32, funct7: u32, rd: Reg, rs1: Reg, rs2: Reg) -> u32 {
    funct7 << 25 | (rs2 as u32) << 20 | (rs1 as u32) << 15 | funct3 << 12 | (rd as u32) << 7 | opcode
}

/// I-type: a register and a 12-bit signed immediate (OP-IMM, loads, `jalr`,
/// SYSTEM, Xcal).
///
/// ```text
///  31          20 19  15 14  12 11   7 6      0
/// |   imm[11:0]  | rs1  |funct3|  rd  | opcode |
/// ```
pub fn i_type(opcode: u32, funct3: u32, rd: Reg, rs1: Reg, imm: i32) -> u32 {
    i_imm(imm) | (rs1 as u32) << 15 | funct3 << 12 | (rd as u32) << 7 | opcode
}

/// The bits a 12-bit immediate occupies in an I-type word: simply the top 12.
pub fn i_imm(imm: i32) -> u32 {
    (imm as u32 & 0xfff) << 20
}

/// S-type: stores. There is no rd, so its slot holds the low 5 immediate
/// bits; rs1 and rs2 stay where R-type has them.
///
/// ```text
///  31     25 24  20 19  15 14  12 11      7 6      0
/// |imm[11:5]| rs2  | rs1  |funct3|imm[4:0]| opcode |
/// ```
pub fn s_type(opcode: u32, funct3: u32, rs1: Reg, rs2: Reg, imm: i32) -> u32 {
    s_imm(imm) | (rs2 as u32) << 20 | (rs1 as u32) << 15 | funct3 << 12 | opcode
}

/// The bits a 12-bit immediate occupies in an S-type word.
pub fn s_imm(imm: i32) -> u32 {
    let i = imm as u32;
    ((i >> 5) & 0x7f) << 25 | (i & 0x1f) << 7
}

/// B-type: conditional branches. Like S-type, but the immediate is an even
/// byte offset in -4096..=4094, so bit 0 is not stored and bit 12 is. The
/// bits are placed so that most of them sit where S-type has them, and the
/// sign (bit 12) is always bit 31, as in every format.
///
/// ```text
///   31   30     25 24  20 19  15 14  12 11     8   7    6      0
/// |[12]|imm[10:5] | rs2  | rs1  |funct3|imm[4:1]|[11]| opcode |
/// ```
pub fn b_type(opcode: u32, funct3: u32, rs1: Reg, rs2: Reg, imm: i32) -> u32 {
    b_imm(imm) | (rs2 as u32) << 20 | (rs1 as u32) << 15 | funct3 << 12 | opcode
}

/// The bits a branch offset occupies in a B-type word.
pub fn b_imm(imm: i32) -> u32 {
    let i = imm as u32;
    ((i >> 12) & 1) << 31 | ((i >> 5) & 0x3f) << 25 | ((i >> 1) & 0xf) << 8 | ((i >> 11) & 1) << 7
}

/// U-type: `lui` and `auipc`. The 20-bit immediate is the upper 20 bits of a
/// 32-bit value whose low 12 bits are zero.
///
/// ```text
///  31                  12 11   7 6      0
/// |      imm[31:12]      |  rd  | opcode |
/// ```
pub fn u_type(opcode: u32, rd: Reg, imm20: u32) -> u32 {
    u_imm(imm20) | (rd as u32) << 7 | opcode
}

/// The bits a 20-bit upper immediate occupies in a U-type word.
pub fn u_imm(imm20: u32) -> u32 {
    (imm20 & 0xf_ffff) << 12
}

/// J-type: `jal`. An even byte offset in -1 MiB..=1 MiB-2. Bits 19:12 stay
/// where U-type has them; the rest is shuffled so that bit 20 (the sign) is
/// bit 31 and bits 10:1 line up with I-type's immediate.
///
/// ```text
///   31   30        21  20   19      12 11   7 6      0
/// |[20]| imm[10:1]   |[11]| imm[19:12] |  rd  | opcode |
/// ```
pub fn j_type(opcode: u32, rd: Reg, imm: i32) -> u32 {
    j_imm(imm) | (rd as u32) << 7 | opcode
}

/// The bits a jump offset occupies in a J-type word.
pub fn j_imm(imm: i32) -> u32 {
    let i = imm as u32;
    ((i >> 20) & 1) << 31 | ((i >> 1) & 0x3ff) << 21 | ((i >> 11) & 1) << 20 | ((i >> 12) & 0xff) << 12
}

/// Splits a 32-bit value into the 20-bit upper part for `lui`/`auipc` and the
/// 12-bit lower part for the `addi`, load, store or `jalr` that completes it.
///
/// The catch: the second instruction *sign-extends* its 12-bit immediate. If
/// bit 11 of the value is set, the low part is negative (low 12 bits minus
/// 4096), so the upper part must be one more to make up for it:
///
/// ```text
/// hi = (value + 0x800) >> 12      lo = value - (hi << 12), in -2048..=2047
///
/// 0x12345678: lo = 0x678,  hi = 0x12345
/// 0x12345fff: lo = -1,     hi = 0x12346   (0x12346000 - 1 = 0x12345fff)
/// ```
///
/// Everything wraps at 32 bits: `0x7ffff800` gives `hi = 0x80000`, `lo = -2048`.
pub fn hi_lo(value: i32) -> (u32, i32) {
    let lo = (value << 20) >> 20; // the low 12 bits, sign-extended
    let hi = (value.wrapping_sub(lo) as u32) >> 12; // exact: value - lo ends in 12 zero bits
    (hi, lo)
}

// ===================================================================
// Operands
// ===================================================================

/// An immediate operand once the expression has been looked at.
enum Imm {
    /// A number: a literal, an `.equ` constant, or `%hi`/`%lo` of one.
    Num(i64),
    /// A label plus addend (`loop`, `msg+4`, `1f`): branch and jump targets,
    /// and the symbol of `la`, `call`, `lw rd, sym`...
    Label(String, i64),
    /// `%hi(sym)`, `%lo(sym)`, `%pcrel_hi(sym)` or `%pcrel_lo(label)`: a field
    /// the linker fills in.
    Reloc(Modifier, String, i64),
}

fn imm(e: &Expr, ctx: &Ctx) -> Result<Imm, String> {
    if let Some(v) = ctx.constant(e) {
        return Ok(Imm::Num(v));
    }
    if let Expr::Mod(m, inner) = e {
        if let Some(v) = ctx.constant(inner) {
            // `%hi(0x12345678)`: no symbol, so split the number right here.
            let v = word(v, &format!("{}() value", m.name()))?;
            return match m {
                Modifier::Hi => Ok(Imm::Num(hi_lo(v).0 as i64)),
                Modifier::Lo => Ok(Imm::Num(hi_lo(v).1 as i64)),
                _ => Err(format!("{}() needs a symbol, not a number", m.name())),
            };
        }
        let (name, addend) = ctx
            .symbol(inner)
            .ok_or_else(|| format!("{}() needs a symbol or a number", m.name()))?;
        return Ok(Imm::Reloc(*m, name, addend));
    }
    match ctx.symbol(e) {
        Some((name, addend)) => Ok(Imm::Label(name, addend)),
        None => Err("expected a number, a constant or symbol+offset".into()),
    }
}

/// A 32-bit value, signed or unsigned (`-1` and `0xffffffff` are the same word).
fn word(v: i64, what: &str) -> Result<i32, String> {
    if (-(1 << 31)..1 << 32).contains(&v) {
        Ok(v as u32 as i32)
    } else {
        Err(format!("{what} {v} does not fit in 32 bits"))
    }
}

/// A signed value of `bits` bits (two's complement range).
fn signed(v: i64, bits: u32, what: &str) -> Result<i32, String> {
    let half = 1i64 << (bits - 1);
    if (-half..half).contains(&v) {
        Ok(v as i32)
    } else {
        Err(format!("{what} {v} out of range ({}..={})", -half, half - 1))
    }
}

/// An unsigned value of `bits` bits. Wide fields are shown in hex, as people write them.
fn unsigned(v: i64, bits: u32, what: &str) -> Result<u32, String> {
    let max = (1i64 << bits) - 1;
    if (0..=max).contains(&v) {
        Ok(v as u32)
    } else if bits >= 12 {
        let v = if v < 0 {
            format!("-{:#x}", v.unsigned_abs())
        } else {
            format!("{v:#x}")
        };
        Err(format!("{what} {v} out of range (0..={max:#x})"))
    } else {
        Err(format!("{what} {v} out of range (0..={max})"))
    }
}

/// A pc-relative branch or jump offset: even, `bits` bits signed. (The
/// formats don't store bit 0, so the largest offset is 2^(bits-1) - 2.)
fn offset(v: i64, bits: u32, what: &str) -> Result<i32, String> {
    let half = 1i64 << (bits - 1);
    if !(-half..half).contains(&v) {
        Err(format!("{what} {v} out of range ({}..={})", -half, half - 2))
    } else if v % 2 != 0 {
        Err(format!("{what} {v} must be even"))
    } else {
        Ok(v as i32)
    }
}

/// An immediate field, ready to pack: its value, or zero plus the relocation
/// that will fill it in.
struct Field {
    value: i32,
    fixup: Option<Fixup>,
}

impl Field {
    fn num(value: i32) -> Field {
        Field { value, fixup: None }
    }

    fn reloc(kind: RelocKind, name: String, addend: i64) -> Field {
        let target = Target::Symbol { name, addend };
        Field {
            value: 0,
            fixup: Some(Fixup { kind, target }),
        }
    }

    /// The finished word, carrying this field's relocation if it has one.
    fn emit(self, word: u32) -> Emit {
        Emit {
            word,
            fixup: self.fixup,
        }
    }
}

/// A 12-bit signed immediate (I-type, or S-type when `store`): a number,
/// `%lo(sym)` or `%pcrel_lo(label)`.
fn imm12(e: &Expr, ctx: &Ctx, store: bool) -> Result<Field, String> {
    let (lo, pcrel_lo) = if store {
        (RelocKind::Lo12S, RelocKind::PcrelLo12S)
    } else {
        (RelocKind::Lo12I, RelocKind::PcrelLo12I)
    };
    match imm(e, ctx)? {
        Imm::Num(v) => Ok(Field::num(signed(v, 12, "immediate")?)),
        Imm::Reloc(Modifier::Lo, name, addend) => Ok(Field::reloc(lo, name, addend)),
        Imm::Reloc(Modifier::PcrelLo, name, addend) => Ok(Field::reloc(pcrel_lo, name, addend)),
        Imm::Reloc(m, ..) => Err(format!(
            "{}() is a 20-bit upper part and belongs in {}, not in a 12-bit immediate",
            m.name(),
            if m == Modifier::Hi { "lui" } else { "auipc" }
        )),
        Imm::Label(name, _) if ctx.undefined(&name) => Err(not_yet_constant(&name)),
        Imm::Label(..) => Err("a label can't be a 12-bit immediate: use %lo(sym) or %pcrel_lo(label)".into()),
    }
}

/// The error for a symbol where only a constant fits, when that symbol isn't
/// defined yet: most likely a constant whose `.equ` comes later.
fn not_yet_constant(name: &str) -> String {
    format!("'{name}' is not a constant (yet): .equ/.set constants must be defined before use")
}

/// The 20-bit immediate of `lui` (a number or `%hi(sym)`) or `auipc` (a
/// number or `%pcrel_hi(sym)`).
fn imm20(e: &Expr, ctx: &Ctx, opcode: u32) -> Result<Field, String> {
    let (m, kind, mnemonic) = if opcode == LUI {
        (Modifier::Hi, RelocKind::Hi20, "lui")
    } else {
        (Modifier::PcrelHi, RelocKind::PcrelHi20, "auipc")
    };
    match imm(e, ctx)? {
        Imm::Num(v) => Ok(Field::num(unsigned(v, 20, "upper immediate")? as i32)),
        Imm::Reloc(got, name, addend) if got == m => Ok(Field::reloc(kind, name, addend)),
        _ => Err(format!(
            "{mnemonic} takes a number in 0..=0xfffff or {}(sym)",
            m.name()
        )),
    }
}

/// A branch target: a label (relocated) or an even byte offset in -4096..=4094.
fn branch_target(e: &Expr, ctx: &Ctx) -> Result<Field, String> {
    match imm(e, ctx)? {
        Imm::Num(v) => Ok(Field::num(offset(v, 13, "branch offset")?)),
        Imm::Label(name, addend) => Ok(Field::reloc(RelocKind::Branch, name, addend)),
        Imm::Reloc(m, ..) => Err(format!("a branch target can't use {}()", m.name())),
    }
}

/// A `jal` target: a label (relocated) or an even byte offset within ±1 MiB.
fn jump_target(e: &Expr, ctx: &Ctx) -> Result<Field, String> {
    match imm(e, ctx)? {
        Imm::Num(v) => Ok(Field::num(offset(v, 21, "jump offset")?)),
        Imm::Label(name, addend) => Ok(Field::reloc(RelocKind::Jal, name, addend)),
        Imm::Reloc(m, ..) => Err(format!("a jump target can't use {}()", m.name())),
    }
}

/// A plain number in `0..2^bits` (shift amounts, CSR immediates, `.insn` fields).
fn uimm(e: &Expr, ctx: &Ctx, bits: u32, what: &str) -> Result<u32, String> {
    match ctx.constant(e) {
        Some(v) => unsigned(v, bits, what),
        None => Err(format!("{what} must be a number")),
    }
}

/// A symbol for `la`, `call`, `lw rd, sym`...: never a number or a modifier.
fn symbol(op: &Operand, ctx: &Ctx, mnemonic: &str) -> Result<Target, String> {
    match imm(expr(op)?, ctx)? {
        Imm::Label(name, addend) => Ok(Target::Symbol { name, addend }),
        Imm::Num(_) => Err(format!("{mnemonic} needs a symbol, not a number")),
        Imm::Reloc(m, ..) => Err(format!("{mnemonic} takes a plain symbol, not {}()", m.name())),
    }
}

fn what(op: &Operand) -> &'static str {
    match op {
        Operand::Reg(_) => "a register",
        Operand::Expr(_) => "an immediate",
        Operand::Mem { .. } => "a memory operand",
    }
}

fn reg(op: &Operand) -> Result<Reg, String> {
    match op {
        Operand::Reg(r) => Ok(*r),
        _ => Err(format!("expected a register, found {}", what(op))),
    }
}

fn expr(op: &Operand) -> Result<&Expr, String> {
    match op {
        Operand::Expr(e) => Ok(e),
        _ => Err(format!("expected an immediate, found {}", what(op))),
    }
}

fn mem(op: &Operand) -> Result<(&Expr, Reg), String> {
    match op {
        Operand::Mem { offset, base, .. } => Ok((offset, *base)),
        _ => Err(format!("expected offset(register), found {}", what(op))),
    }
}

/// A CSR operand: a name (`mstatus`) or a number in 0..=0xfff.
fn csr_addr(op: &Operand, ctx: &Ctx) -> Result<u32, String> {
    let e = expr(op).map_err(|_| format!("expected a CSR name or number, found {}", what(op)))?;
    if let Some(addr) = ctx.csr(e) {
        return Ok(addr as u32);
    }
    match (e, ctx.constant(e)) {
        (_, Some(v)) => Err(format!("CSR number {v} out of range (0..=0xfff)")),
        (Expr::Sym(name), None) => Err(format!("unknown CSR '{name}'")),
        _ => Err("expected a CSR name or number".into()),
    }
}

/// A fence's predecessor or successor set: letters taken in order from
/// `iorw` (device input, device output, memory reads, memory writes), or `0`.
fn fence_set(op: &Operand, ctx: &Ctx) -> Result<u32, String> {
    let e = expr(op)?;
    if ctx.constant(e) == Some(0) {
        return Ok(0);
    }
    let bad = || "a fence set is 0 or letters taken in order from 'iorw'".to_string();
    let Expr::Sym(s) = e else { return Err(bad()) };
    let mut bits = 0u32;
    let mut last = 16; // each letter must be lower in the set than the one before
    for c in s.chars() {
        let bit = match c {
            'i' => 8,
            'o' => 4,
            'r' => 2,
            'w' => 1,
            _ => return Err(bad()),
        };
        if bit >= last {
            return Err(bad());
        }
        bits |= bit;
        last = bit;
    }
    Ok(bits)
}

// ===================================================================
// Mnemonic tables: (funct3, funct7) for each real instruction, by format
//
// Data rather than code, so that `cargo xtask kernel` can write the same
// tables into the C header of the assembler that runs on Camelot
// (`kernel/user/as/tables.h`), which then can't drift from this one.
// ===================================================================

/// OP: `rd = rs1 <op> rs2`, (mnemonic, funct3, funct7). The M extension
/// shares the opcode with funct7 = 1.
pub const OP_FUNCT: &[(&str, u32, u32)] = &[
    ("add", 0b000, 0b0000000),
    ("sub", 0b000, 0b0100000),
    ("sll", 0b001, 0b0000000),
    ("slt", 0b010, 0b0000000),
    ("sltu", 0b011, 0b0000000),
    ("xor", 0b100, 0b0000000),
    ("srl", 0b101, 0b0000000),
    ("sra", 0b101, 0b0100000),
    ("or", 0b110, 0b0000000),
    ("and", 0b111, 0b0000000),
    ("mul", 0b000, 0b0000001),
    ("mulh", 0b001, 0b0000001),
    ("mulhsu", 0b010, 0b0000001),
    ("mulhu", 0b011, 0b0000001),
    ("div", 0b100, 0b0000001),
    ("divu", 0b101, 0b0000001),
    ("rem", 0b110, 0b0000001),
    ("remu", 0b111, 0b0000001),
];

/// OP-IMM with a 12-bit immediate: `rd = rs1 <op> imm`, (mnemonic, funct3).
pub const OP_IMM_FUNCT3: &[(&str, u32)] = &[
    ("addi", ADDI),
    ("slti", 0b010),
    ("sltiu", SLTIU),
    ("xori", XORI),
    ("ori", 0b110),
    ("andi", ANDI),
];

/// OP-IMM shifts, (mnemonic, funct3, funct7): the immediate is a 5-bit shift
/// amount, and the top 7 bits of the I-type immediate act as a funct7
/// (0100000 selects arithmetic).
pub const SHIFT_FUNCT: &[(&str, u32, u32)] = &[
    ("slli", SLLI, 0b0000000),
    ("srli", 0b101, 0b0000000),
    ("srai", 0b101, 0b0100000),
];

/// Loads, (mnemonic, funct3).
pub const LOAD_FUNCT3: &[(&str, u32)] = &[
    ("lb", 0b000),
    ("lh", 0b001),
    ("lw", 0b010),
    ("lbu", 0b100),
    ("lhu", 0b101),
];

/// Stores, (mnemonic, funct3).
pub const STORE_FUNCT3: &[(&str, u32)] = &[("sb", 0b000), ("sh", 0b001), ("sw", 0b010)];

/// Conditional branches, (mnemonic, funct3).
pub const BRANCH_FUNCT3: &[(&str, u32)] = &[
    ("beq", BEQ),
    ("bne", BNE),
    ("blt", BLT),
    ("bge", BGE),
    ("bltu", BLTU),
    ("bgeu", BGEU),
];

/// The six `csrr*` instructions, (mnemonic, funct3): bit 2 is set for the
/// immediate forms.
pub const CSR_FUNCT3: &[(&str, u32)] = &[
    ("csrrw", CSRRW),
    ("csrrs", CSRRS),
    ("csrrc", CSRRC),
    ("csrrwi", CSRRW | CSR_IMM),
    ("csrrsi", CSRRS | CSR_IMM),
    ("csrrci", CSRRC | CSR_IMM),
];

/// A extension: funct5 (bits [31:27]) of each `.w` mnemonic, without its
/// ordering suffix.
pub const ATOMIC_FUNCT5: &[(&str, u32)] = &[
    ("lr.w", 0b00010),
    ("sc.w", 0b00011),
    ("amoswap.w", 0b00001),
    ("amoadd.w", 0b00000),
    ("amoxor.w", 0b00100),
    ("amoand.w", 0b01100),
    ("amoor.w", 0b01000),
    ("amomin.w", 0b10000),
    ("amomax.w", 0b10100),
    ("amominu.w", 0b11000),
    ("amomaxu.w", 0b11100),
];

/// SYSTEM instructions with no operands: their funct12 (the I-type immediate).
pub const SYSTEM_FUNCT12: &[(&str, u32)] = &[
    ("ecall", 0x000),
    ("ebreak", 0x001),
    ("sret", 0x102),
    ("wfi", 0x105),
    ("mret", 0x302),
];

/// The entry of `table` for mnemonic `m`.
fn find<T: Copy>(table: &[(&str, T)], m: &str) -> Option<T> {
    table.iter().find(|e| e.0 == m).map(|e| e.1)
}

/// The entry of a three-column table for mnemonic `m`.
fn find2(table: &[(&str, u32, u32)], m: &str) -> Option<(u32, u32)> {
    table.iter().find(|e| e.0 == m).map(|e| (e.1, e.2))
}

fn op_funct(m: &str) -> Option<(u32, u32)> {
    find2(OP_FUNCT, m)
}

fn op_imm_funct3(m: &str) -> Option<u32> {
    find(OP_IMM_FUNCT3, m)
}

fn shift_funct(m: &str) -> Option<(u32, u32)> {
    find2(SHIFT_FUNCT, m)
}

fn load_funct3(m: &str) -> Option<u32> {
    find(LOAD_FUNCT3, m)
}

fn store_funct3(m: &str) -> Option<u32> {
    find(STORE_FUNCT3, m)
}

fn branch_funct3(m: &str) -> Option<u32> {
    find(BRANCH_FUNCT3, m)
}

fn csr_funct3(m: &str) -> Option<u32> {
    find(CSR_FUNCT3, m)
}

/// Splits an A mnemonic into its base (`amoadd.w`) and its ordering bits
/// aq|rl, from the suffix: none, `.aq` (10), `.rl` (01) or `.aqrl` (11).
fn atomic(m: &str) -> Option<(&str, u32, u32)> {
    let (base, aqrl) = [(".aqrl", 0b11), (".aq", 0b10), (".rl", 0b01)]
        .into_iter()
        .find_map(|(suffix, bits)| Some((m.strip_suffix(suffix)?, bits)))
        .unwrap_or((m, 0b00));
    Some((base, find(ATOMIC_FUNCT5, base)?, aqrl))
}

fn system_funct12(m: &str) -> Option<i32> {
    find(SYSTEM_FUNCT12, m).map(|f| f as i32)
}

// ===================================================================
// The encoder
// ===================================================================

fn one(word: u32) -> Result<Vec<Emit>, String> {
    Ok(vec![Emit::word(word)])
}

/// Encodes one instruction or pseudo-instruction. See the module docs.
pub fn encode(m: &str, ops: &[Operand], ctx: &Ctx) -> Result<Vec<Emit>, String> {
    // Wrong operand count: show the expected form.
    let usage = |form: &str| -> Result<Vec<Emit>, String> {
        if form.is_empty() {
            Err(format!("{m} takes no operands"))
        } else {
            Err(format!("usage: {m} {form}"))
        }
    };

    // ----- R-type: OP and M -----
    if let Some((funct3, funct7)) = op_funct(m) {
        let [rd, rs1, rs2] = ops else {
            return usage("rd, rs1, rs2");
        };
        return one(r_type(OP, funct3, funct7, reg(rd)?, reg(rs1)?, reg(rs2)?));
    }

    // ----- I-type: OP-IMM -----
    if let Some(funct3) = op_imm_funct3(m) {
        let [rd, rs1, imm] = ops else {
            return usage("rd, rs1, imm");
        };
        let (rd, rs1, f) = (reg(rd)?, reg(rs1)?, imm12(expr(imm)?, ctx, false)?);
        let word = i_type(OP_IMM, funct3, rd, rs1, f.value);
        return Ok(vec![f.emit(word)]);
    }
    if let Some((funct3, funct7)) = shift_funct(m) {
        let [rd, rs1, shamt] = ops else {
            return usage("rd, rs1, shamt");
        };
        let shamt = uimm(expr(shamt)?, ctx, 5, "shift amount")?;
        // funct7 sits in imm[11:5], the shift amount in imm[4:0].
        let imm = (funct7 << 5 | shamt) as i32;
        return one(i_type(OP_IMM, funct3, reg(rd)?, reg(rs1)?, imm));
    }

    // ----- loads (I-type) and stores (S-type) -----
    if let Some(funct3) = load_funct3(m) {
        return load(m, funct3, ops, ctx);
    }
    if let Some(funct3) = store_funct3(m) {
        return store(m, funct3, ops, ctx);
    }

    // ----- B-type: conditional branches -----
    if let Some(funct3) = branch_funct3(m) {
        let [rs1, rs2, target] = ops else {
            return usage("rs1, rs2, offset");
        };
        return branch(funct3, reg(rs1)?, reg(rs2)?, target, ctx);
    }

    // ----- CSRs (I-type, SYSTEM): the CSR number is the immediate -----
    if let Some(funct3) = csr_funct3(m) {
        let [rd, csr, src] = ops else {
            return usage(if funct3 & CSR_IMM != 0 {
                "rd, csr, uimm5"
            } else {
                "rd, csr, rs1"
            });
        };
        return csr_op(funct3, reg(rd)?, csr, src, ctx);
    }

    // ----- SYSTEM instructions without operands -----
    if let Some(funct12) = system_funct12(m) {
        let [] = ops else { return usage("") };
        return one(i_type(SYSTEM, 0, ZERO, ZERO, funct12));
    }

    // ----- A extension (R-type, AMO opcode) -----
    // funct7 is funct5 followed by the aq and rl bits; funct3 = 010 (word).
    // lr.w has no rs2 (the field is zero).
    if let Some((base, funct5, aqrl)) = atomic(m) {
        let (rd, rs2, addr) = match (base, ops) {
            ("lr.w", [rd, addr]) => (rd, ZERO, addr),
            ("lr.w", _) => return usage("rd, (rs1)"),
            (_, [rd, rs2, addr]) => (rd, reg(rs2)?, addr),
            _ => return usage("rd, rs2, (rs1)"),
        };
        let rs1 = atomic_address(m, addr)?;
        return one(r_type(AMO, 0b010, funct5 << 2 | aqrl, reg(rd)?, rs1, rs2));
    }

    match m {
        // ----- U-type -----
        "lui" | "auipc" => {
            let [rd, imm] = ops else { return usage("rd, imm20") };
            let opcode = if m == "lui" { LUI } else { AUIPC };
            let (rd, f) = (reg(rd)?, imm20(expr(imm)?, ctx, opcode)?);
            let word = u_type(opcode, rd, f.value as u32);
            Ok(vec![f.emit(word)])
        }

        // ----- J-type and jalr -----
        // `jal offset` links in ra, as `call` does.
        "jal" => match ops {
            [target] => jal(RA, target, ctx),
            [rd, target] => jal(reg(rd)?, target, ctx),
            _ => usage("[rd,] offset"),
        },
        "jalr" => match ops {
            // `jalr rs`: call through a register, link in ra.
            [rs] => one(i_type(JALR, 0, RA, reg(rs)?, 0)),
            [rd, Operand::Mem { offset, base, .. }] => jalr(reg(rd)?, *base, offset, ctx),
            [rd, rs1, Operand::Expr(offset)] => jalr(reg(rd)?, reg(rs1)?, offset, ctx),
            _ => usage("rd, offset(rs1)"),
        },

        // ----- fences (MISC-MEM) -----
        // fm | pred | succ | rs1 | funct3 | rd | opcode, with fm, rs1 and rd zero.
        "fence" => {
            let (pred, succ) = match ops {
                [] => (0b1111, 0b1111),
                [pred, succ] => (fence_set(pred, ctx)?, fence_set(succ, ctx)?),
                _ => return usage("[pred, succ]"),
            };
            one(i_type(MISC_MEM, 0, ZERO, ZERO, (pred << 4 | succ) as i32))
        }
        // fence.tso: fm = 1000, pred = succ = rw.
        "fence.tso" => {
            let [] = ops else { return usage("") };
            one(0b1000 << 28 | 0b0011 << 24 | 0b0011 << 20 | MISC_MEM)
        }
        "fence.i" => {
            let [] = ops else { return usage("") };
            one(i_type(MISC_MEM, 0b001, ZERO, ZERO, 0))
        }
        // sfence.vma [rs1[, rs2]]: R-type, funct7 = 0001001.
        "sfence.vma" => {
            let (rs1, rs2) = match ops {
                [] => (ZERO, ZERO),
                [rs1] => (reg(rs1)?, ZERO),
                [rs1, rs2] => (reg(rs1)?, reg(rs2)?),
                _ => return usage("[rs1[, rs2]]"),
            };
            one(r_type(SYSTEM, 0, 0b0001001, ZERO, rs1, rs2))
        }

        // ----- Xcal (spec section 9): custom-0, I-type with rd = 0, imm = 0 -----
        "xcal.dbg" | "xcal.halt" => {
            let [rs1] = ops else { return usage("rs1") };
            let funct3 = if m == "xcal.dbg" { 0b000 } else { 0b001 };
            one(i_type(CUSTOM_0, funct3, ZERO, reg(rs1)?, 0))
        }

        // ===== pseudo-instructions (RISC-V Assembly Programmer's Manual) =====
        "nop" => {
            let [] = ops else { return usage("") };
            one(i_type(OP_IMM, ADDI, ZERO, ZERO, 0)) // addi zero, zero, 0
        }
        "li" => {
            let [rd, value] = ops else { return usage("rd, imm") };
            let rd = reg(rd)?;
            match imm(expr(value)?, ctx)? {
                Imm::Num(v) => li(rd, word(v, "li value")?),
                Imm::Label(name, _) if ctx.undefined(&name) => Err(format!(
                    "{} (li loads a number; la loads an address)",
                    not_yet_constant(&name)
                )),
                _ => Err("li takes a constant (use la to load an address)".into()),
            }
        }
        // Register-to-register pseudo-instructions: (rd, rs) into one real instruction.
        "mv" | "not" | "neg" | "seqz" | "snez" | "sltz" | "sgtz" | "zext.b" => {
            let [rd, rs] = ops else { return usage("rd, rs") };
            let (rd, rs) = (reg(rd)?, reg(rs)?);
            let slt = |rs1, rs2| r_type(OP, 0b010, 0, rd, rs1, rs2);
            one(match m {
                "mv" => i_type(OP_IMM, ADDI, rd, rs, 0),         // addi rd, rs, 0
                "not" => i_type(OP_IMM, XORI, rd, rs, -1),       // xori rd, rs, -1
                "neg" => r_type(OP, 0, 0b0100000, rd, ZERO, rs), // sub rd, zero, rs
                "seqz" => i_type(OP_IMM, SLTIU, rd, rs, 1),      // sltiu rd, rs, 1
                "snez" => r_type(OP, 0b011, 0, rd, ZERO, rs),    // sltu rd, zero, rs
                "sltz" => slt(rs, ZERO),                         // slt rd, rs, zero
                "sgtz" => slt(ZERO, rs),                         // slt rd, zero, rs
                _ => i_type(OP_IMM, ANDI, rd, rs, 0xff),         // zext.b: andi rd, rs, 255
            })
        }
        "j" => {
            let [target] = ops else { return usage("offset") };
            jal(ZERO, target, ctx)
        }
        "jr" => {
            let [rs] = ops else { return usage("rs") };
            one(i_type(JALR, 0, ZERO, reg(rs)?, 0))
        }
        "ret" => {
            let [] = ops else { return usage("") };
            one(i_type(JALR, 0, ZERO, RA, 0)) // jalr zero, 0(ra)
        }
        // Compare against zero: (real branch, rs1, rs2) with zero on one side.
        "beqz" | "bnez" | "bltz" | "bgez" | "blez" | "bgtz" => {
            let [rs, target] = ops else {
                return usage("rs, offset");
            };
            let rs = reg(rs)?;
            let (funct3, rs1, rs2) = match m {
                "beqz" => (BEQ, rs, ZERO),
                "bnez" => (BNE, rs, ZERO),
                "bltz" => (BLT, rs, ZERO),
                "bgez" => (BGE, rs, ZERO),
                "blez" => (BGE, ZERO, rs), // 0 >= rs
                _ => (BLT, ZERO, rs),      // bgtz: 0 < rs
            };
            branch(funct3, rs1, rs2, target, ctx)
        }
        // The missing comparisons, by swapping operands: bgt a, b == blt b, a.
        "bgt" | "ble" | "bgtu" | "bleu" => {
            let [rs, rt, target] = ops else {
                return usage("rs, rt, offset");
            };
            let funct3 = match m {
                "bgt" => BLT,
                "ble" => BGE,
                "bgtu" => BLTU,
                _ => BGEU,
            };
            branch(funct3, reg(rt)?, reg(rs)?, target, ctx)
        }
        // Pc-relative address: auipc rd, %pcrel_hi(sym); addi rd, rd, %pcrel_lo(that auipc).
        // Without PIC, `la` is the same as `lla`.
        "la" | "lla" => {
            let [rd, sym] = ops else {
                return usage("rd, symbol");
            };
            let (rd, target) = (reg(rd)?, symbol(sym, ctx, m)?);
            Ok(vec![
                Emit::with_fixup(u_type(AUIPC, rd, 0), RelocKind::PcrelHi20, target),
                Emit::with_fixup(
                    i_type(OP_IMM, ADDI, rd, rd, 0),
                    RelocKind::PcrelLo12I,
                    Target::Auipc(0),
                ),
            ])
        }
        // Far calls: auipc + jalr with one R_RISCV_CALL_PLT on the pair.
        // `call` links in ra and uses it as the scratch register; `tail` is a
        // jump that keeps ra and goes through t1.
        "call" | "tail" => {
            let [sym] = ops else { return usage("symbol") };
            let target = symbol(sym, ctx, m)?;
            let (scratch, link) = if m == "call" { (RA, RA) } else { (T1, ZERO) };
            Ok(vec![
                Emit::with_fixup(u_type(AUIPC, scratch, 0), RelocKind::CallPlt, target),
                Emit::word(i_type(JALR, 0, link, scratch, 0)),
            ])
        }
        // CSR shorthands: rd or rs1 is zero.
        "csrr" => {
            let [rd, csr] = ops else { return usage("rd, csr") };
            csr_op(CSRRS, reg(rd)?, csr, &Operand::Reg(ZERO), ctx)
        }
        "csrw" | "csrs" | "csrc" | "csrwi" | "csrsi" | "csrci" => {
            let [csr, src] = ops else { return usage("csr, rs") };
            let mut funct3 = match &m[..4] {
                "csrw" => CSRRW,
                "csrs" => CSRRS,
                _ => CSRRC,
            };
            // `csrw mstatus, 8` means csrwi, as in LLVM.
            if m.ends_with('i') || matches!(src, Operand::Expr(_)) {
                funct3 |= CSR_IMM;
            }
            csr_op(funct3, ZERO, csr, src, ctx)
        }
        // Counter reads: csrrs rd, <counter>, zero.
        "rdcycle" | "rdtime" | "rdinstret" | "rdcycleh" | "rdtimeh" | "rdinstreth" => {
            let [rd] = ops else { return usage("rd") };
            let counter = match m {
                "rdcycle" => csr::CYCLE,
                "rdtime" => csr::TIME,
                "rdinstret" => csr::INSTRET,
                "rdcycleh" => csr::CYCLEH,
                "rdtimeh" => csr::TIMEH,
                _ => csr::INSTRETH,
            };
            one(i_type(SYSTEM, CSRRS, reg(rd)?, ZERO, counter as i32))
        }
        // The canonical illegal instruction: csrrw zero, cycle, zero (cycle is read-only).
        "unimp" => {
            let [] = ops else { return usage("") };
            one(i_type(SYSTEM, CSRRW, ZERO, ZERO, csr::CYCLE as i32))
        }

        _ if m == ".insn" || m.starts_with(".insn ") => insn(m, ops, ctx),
        _ => Err(format!("unknown instruction '{m}'")),
    }
}

fn branch(funct3: u32, rs1: Reg, rs2: Reg, target: &Operand, ctx: &Ctx) -> Result<Vec<Emit>, String> {
    let f = branch_target(expr(target)?, ctx)?;
    let word = b_type(BRANCH, funct3, rs1, rs2, f.value);
    Ok(vec![f.emit(word)])
}

fn jal(rd: Reg, target: &Operand, ctx: &Ctx) -> Result<Vec<Emit>, String> {
    let f = jump_target(expr(target)?, ctx)?;
    let word = j_type(JAL, rd, f.value);
    Ok(vec![f.emit(word)])
}

fn jalr(rd: Reg, rs1: Reg, offset: &Expr, ctx: &Ctx) -> Result<Vec<Emit>, String> {
    let f = imm12(offset, ctx, false)?;
    let word = i_type(JALR, 0, rd, rs1, f.value);
    Ok(vec![f.emit(word)])
}

/// `lw rd, offset(rs1)`, or `lw rd, symbol`: a pc-relative pair through rd,
/// `auipc rd, %pcrel_hi(symbol)` + `lw rd, %pcrel_lo(that auipc)(rd)`.
fn load(m: &str, funct3: u32, ops: &[Operand], ctx: &Ctx) -> Result<Vec<Emit>, String> {
    let [rd, src] = ops else {
        return Err(format!("usage: {m} rd, offset(rs1)  or  {m} rd, symbol"));
    };
    let rd = reg(rd)?;
    match src {
        Operand::Mem { offset, base, .. } => {
            let f = imm12(offset, ctx, false)?;
            let word = i_type(LOAD, funct3, rd, *base, f.value);
            Ok(vec![f.emit(word)])
        }
        Operand::Expr(_) => Ok(vec![
            Emit::with_fixup(u_type(AUIPC, rd, 0), RelocKind::PcrelHi20, symbol(src, ctx, m)?),
            Emit::with_fixup(
                i_type(LOAD, funct3, rd, rd, 0),
                RelocKind::PcrelLo12I,
                Target::Auipc(0),
            ),
        ]),
        Operand::Reg(_) => Err(format!(
            "expected offset(register) or a symbol, found {}",
            what(src)
        )),
    }
}

/// `sw rs2, offset(rs1)`, or `sw rs2, symbol, rt`: a pc-relative pair through
/// the scratch register rt (rs2 holds the value, so it can't be used).
fn store(m: &str, funct3: u32, ops: &[Operand], ctx: &Ctx) -> Result<Vec<Emit>, String> {
    match ops {
        [_, Operand::Expr(_)] => Err(format!(
            "a store to a symbol needs a scratch register for the address: {m} rs2, symbol, rt"
        )),
        [rs2, addr] => {
            let (rs2, (offset, base)) = (reg(rs2)?, mem(addr)?);
            let f = imm12(offset, ctx, true)?;
            let word = s_type(STORE, funct3, base, rs2, f.value);
            Ok(vec![f.emit(word)])
        }
        [rs2, sym, rt] => {
            let (rs2, target, rt) = (reg(rs2)?, symbol(sym, ctx, m)?, reg(rt)?);
            Ok(vec![
                Emit::with_fixup(u_type(AUIPC, rt, 0), RelocKind::PcrelHi20, target),
                Emit::with_fixup(
                    s_type(STORE, funct3, rt, rs2, 0),
                    RelocKind::PcrelLo12S,
                    Target::Auipc(0),
                ),
            ])
        }
        _ => Err(format!("usage: {m} rs2, offset(rs1)  or  {m} rs2, symbol, rt")),
    }
}

/// The address operand of an A instruction: `(rs1)`. An A instruction has no
/// offset field, so, as in LLVM, the only offset accepted is a literal `0`
/// (`0(rs1)`, `0x0(rs1)`). LLVM takes one integer token there, not an
/// expression, so `-0(rs1)`, `+0(rs1)`, `(0)(rs1)`, `1-1(rs1)` or a symbol
/// equal to 0 are errors even though their value is 0. The parser folds
/// `+0` and `(0)` to `Num(0)`, so the check uses the operand's `int_token`,
/// which records how the offset was written.
fn atomic_address(m: &str, addr: &Operand) -> Result<Reg, String> {
    let (offset, base) = mem(addr).map_err(|e| format!("{e}: {m} takes its address as (rs1)"))?;
    let int_token = matches!(addr, Operand::Mem { int_token: true, .. });
    match offset {
        Expr::Num(0) if int_token => Ok(base),
        _ => Err(format!("{m} has no offset field: write the address as (rs1)")),
    }
}

/// A `csrr*` instruction: the CSR number is the I-type immediate, and rs1 is
/// a register or (for the `i` forms) a 5-bit zero-extended immediate.
fn csr_op(funct3: u32, rd: Reg, csr: &Operand, src: &Operand, ctx: &Ctx) -> Result<Vec<Emit>, String> {
    let addr = csr_addr(csr, ctx)?;
    let rs1 = if funct3 & CSR_IMM != 0 {
        uimm(expr(src)?, ctx, 5, "CSR immediate")? as Reg
    } else {
        reg(src)?
    };
    one(i_type(SYSTEM, funct3, rd, rs1, addr as i32))
}

/// `li rd, value`: a 32-bit constant in one or two instructions, chosen the
/// way LLVM does (the golden file comes from llvm-mc):
///
/// 1. fits in 12 signed bits: `addi rd, zero, value`;
/// 2. low 12 bits zero: `lui rd, value >> 12`;
/// 3. an even value `c << s` with `c` in -32..=31: `addi rd, zero, c` +
///    `slli rd, rd, s` (LLVM prefers it because with the C extension both
///    compress to 16 bits; `li a0, 2048` is `addi a0, zero, 1; slli a0, a0, 11`);
/// 4. anything else: `lui rd, hi` + `addi rd, rd, lo`, split by [`hi_lo`].
fn li(rd: Reg, value: i32) -> Result<Vec<Emit>, String> {
    let addi = |rs1, imm| Emit::word(i_type(OP_IMM, ADDI, rd, rs1, imm));
    let lui = |imm20| Emit::word(u_type(LUI, rd, imm20));
    if (-2048..=2047).contains(&value) {
        return Ok(vec![addi(ZERO, value)]);
    }
    let (hi, lo) = hi_lo(value);
    if lo == 0 {
        return Ok(vec![lui(hi)]);
    }
    let shift = value.trailing_zeros(); // 1..=11 for an even value here: lo != 0
    if shift > 0 && (-32..=31).contains(&(value >> shift)) {
        let slli = Emit::word(i_type(OP_IMM, SLLI, rd, rd, shift as i32));
        return Ok(vec![addi(ZERO, value >> shift), slli]);
    }
    Ok(vec![lui(hi), addi(rd, lo)])
}

/// `.insn`: an instruction written field by field, for encodings the
/// assembler has no mnemonic for (examples/xcal.inc expands `xcal.dbg a0` to
/// `.insn i 0x0b, 0, x0, a0, 0` so that clang can build the examples too). The parser
/// turns `.insn i ...` into the mnemonic `.insn i`. Forms, as in LLVM:
///
/// ```text
/// .insn r opcode, funct3, funct7, rd, rs1, rs2
/// .insn i opcode, funct3, rd, rs1, simm12      .insn i opcode, funct3, rd, simm12(rs1)
/// .insn s opcode, funct3, rs2, simm12(rs1)
/// .insn b opcode, funct3, rs1, rs2, offset
/// .insn u opcode, rd, imm20
/// .insn value                                  .insn 4, value
/// ```
///
/// (`.insn j` is left out: LLVM 21 packs its numeric offsets unscrambled.)
fn insn(m: &str, ops: &[Operand], ctx: &Ctx) -> Result<Vec<Emit>, String> {
    let format = m.strip_prefix(".insn").unwrap_or(m).trim();
    let field = |op: &Operand, bits: u32, what: &str| uimm(expr(op)?, ctx, bits, what);
    let opcode = |op: &Operand| {
        let v = field(op, 7, "opcode")?;
        if v & 3 != 3 {
            return Err(format!(
                "opcode {v:#x} is not a 32-bit opcode (its low two bits must be 11)"
            ));
        }
        Ok(v)
    };
    let usage = |form: &str| -> Result<Vec<Emit>, String> { Err(format!("usage: .insn {format} {form}")) };
    match format {
        "r" => {
            let [op, f3, f7, rd, rs1, rs2] = ops else {
                return usage("opcode, funct3, funct7, rd, rs1, rs2");
            };
            let (op, f3, f7) = (opcode(op)?, field(f3, 3, "funct3")?, field(f7, 7, "funct7")?);
            one(r_type(op, f3, f7, reg(rd)?, reg(rs1)?, reg(rs2)?))
        }
        "i" => {
            let (op, f3, rd, rs1, offset) = match ops {
                [op, f3, rd, Operand::Mem { offset, base, .. }] => (op, f3, rd, *base, offset),
                [op, f3, rd, rs1, imm] => (op, f3, rd, reg(rs1)?, expr(imm)?),
                _ => return usage("opcode, funct3, rd, rs1, simm12"),
            };
            let (op, f3, rd) = (opcode(op)?, field(f3, 3, "funct3")?, reg(rd)?);
            let f = imm12(offset, ctx, false)?;
            let word = i_type(op, f3, rd, rs1, f.value);
            Ok(vec![f.emit(word)])
        }
        "s" => {
            let [op, f3, rs2, addr] = ops else {
                return usage("opcode, funct3, rs2, simm12(rs1)");
            };
            let (op, f3, rs2) = (opcode(op)?, field(f3, 3, "funct3")?, reg(rs2)?);
            let (offset, base) = mem(addr)?;
            let f = imm12(offset, ctx, true)?;
            let word = s_type(op, f3, base, rs2, f.value);
            Ok(vec![f.emit(word)])
        }
        "b" => {
            let [op, f3, rs1, rs2, target] = ops else {
                return usage("opcode, funct3, rs1, rs2, offset");
            };
            let (op, f3) = (opcode(op)?, field(f3, 3, "funct3")?);
            let f = branch_target(expr(target)?, ctx)?;
            let word = b_type(op, f3, reg(rs1)?, reg(rs2)?, f.value);
            Ok(vec![f.emit(word)])
        }
        "u" => {
            let [op, rd, imm] = ops else {
                return usage("opcode, rd, imm20");
            };
            let (op, rd) = (opcode(op)?, reg(rd)?);
            one(u_type(op, rd, uimm(expr(imm)?, ctx, 20, "upper immediate")?))
        }
        "" => {
            let value = match ops {
                [value] => value,
                [len, value] => {
                    if ctx.constant(expr(len)?) != Some(4) {
                        return Err(".insn length must be 4 (there are no 16-bit instructions)".into());
                    }
                    value
                }
                _ => return Err("usage: .insn value  or  .insn 4, value".into()),
            };
            let w = field(value, 32, "instruction word")?;
            if w & 3 != 3 {
                return Err(format!(
                    "{w:#010x} is not a 32-bit instruction (its low two bits must be 11)"
                ));
            }
            one(w)
        }
        _ => Err(format!(
            "unknown .insn format '{format}' (merlin-as knows r, i, s, b and u)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use excalibur::{decode, AluOp, BranchOp, Inst, StoreOp};

    #[test]
    fn packers_round_trip_through_the_decoder() {
        // Every immediate bit pattern that matters: extremes, single bits, alternating.
        let imms12 = [-2048, -1, 0, 1, 2047, 0x555, -0x556, 0x400, 0x20];
        for imm in imms12 {
            assert_eq!(
                decode(i_type(OP_IMM, ADDI, 10, 11, imm)),
                Inst::OpImm {
                    op: AluOp::Add,
                    rd: 10,
                    rs1: 11,
                    imm
                }
            );
            assert_eq!(
                decode(s_type(STORE, 0b010, 2, 1, imm)),
                Inst::Store {
                    op: StoreOp::Sw,
                    rs1: 2,
                    rs2: 1,
                    imm
                }
            );
        }
        for imm in [-4096, -2, 0, 2, 4094, 0x800, 0xaaa, -0x556 * 2, 0x7fe] {
            assert_eq!(
                decode(b_type(BRANCH, BNE, 5, 6, imm)),
                Inst::Branch {
                    op: BranchOp::Bne,
                    rs1: 5,
                    rs2: 6,
                    imm
                }
            );
        }
        for imm in [-1 << 20, -2, 0, 2, (1 << 20) - 2, 0x800, 0x1000, 0xaaaaa, 0x55554] {
            assert_eq!(decode(j_type(JAL, 1, imm)), Inst::Jal { rd: 1, imm });
        }
        for imm20 in [0, 1, 0x80000, 0xfffff, 0x12345] {
            assert_eq!(
                decode(u_type(LUI, 3, imm20)),
                Inst::Lui {
                    rd: 3,
                    imm: imm20 << 12
                }
            );
        }
        assert_eq!(
            decode(r_type(OP, 0, 0b0100000, 1, 2, 3)),
            Inst::Op {
                op: AluOp::Sub,
                rd: 1,
                rs1: 2,
                rs2: 3
            }
        );
    }

    #[test]
    fn hi_lo_rounds_the_upper_part() {
        assert_eq!(hi_lo(0x1234_5678), (0x12345, 0x678));
        assert_eq!(hi_lo(0x1234_5fff), (0x12346, -1));
        assert_eq!(hi_lo(0x7fff_f800), (0x80000, -2048));
        assert_eq!(hi_lo(-1), (0, -1));
        assert_eq!(hi_lo(-2049), (0xfffff, 2047));
        assert_eq!(hi_lo(0x800), (1, -2048));
        // Whatever the value, lui + addi gets it back.
        for v in [
            0,
            1,
            -1,
            0x7ff,
            0x800,
            0xfff,
            -0x800,
            -0x801,
            i32::MIN,
            i32::MAX,
            0x1234_5fff,
        ] {
            let (hi, lo) = hi_lo(v);
            assert_eq!(((hi << 12) as i32).wrapping_add(lo), v, "{v:#x}");
            assert!((-2048..=2047).contains(&lo));
        }
    }

    #[test]
    fn li_picks_the_shortest_sequence() {
        let words = |v| li(10, v).unwrap().iter().map(|e| e.word).collect::<Vec<_>>();
        assert_eq!(words(0), [0x0000_0513]);
        assert_eq!(words(0x1000_0000), [0x1000_0537]);
        assert_eq!(words(2048), [0x0010_0513, 0x00b5_1513]);
        assert_eq!(words(0x1234_5678), [0x1234_5537, 0x6785_0513]);
        // Odd, so no shift form even though small after the rounding.
        assert_eq!(words(0x801), [0x0000_1537, 0x8015_0513]);
    }
}
