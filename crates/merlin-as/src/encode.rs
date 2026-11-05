//! # The encoder
//!
//! Turns one instruction (mnemonic + parsed operands) into machine words.
//! Everything around it (lexing, parsing, labels, sections, directives,
//! relocations, ELF output) lives elsewhere; this module is the whole ISA:
//! RV64I, M, A, Zicsr, Zifencei, the privileged instructions, Xcal (spec
//! section 9), the standard pseudo-instructions and the `.insn` directive.
//!
//! ## RV64
//!
//! Every instruction word is 32 bits. What RV64 has beyond RV32's base set:
//!
//! * the 32-bit operations `addiw`, `slliw`, `addw`, `mulw`... (opcodes
//!   OP-IMM-32 and OP-32, their result sign-extended from bit 31), the
//!   doubleword loads and stores `ld`, `lwu`, `sd`, and the `.d` atomics. They
//!   live in tables of their own (`OP_32_FUNCT` ...), as in the ISA manual's
//!   chapter on RV64I;
//! * shift amounts of 6 bits (`slli a0, a0, 63`), 5 for the 32-bit shifts;
//! * `li` of any 64-bit constant, in up to 8 instructions.
//!
//! What only RV32 has, the high halves of the counters (`rdcycleh`, CSR
//! `cycleh`...), is an error that says so, as in LLVM: RV64's CSRs hold all 64
//! bits. (Until #159 merlin-as assembled RV32 too.)
//!
//! ## Contract with the assembler driver ([`crate::asm`])
//!
//! * [`encode`] returns one [`Emit`] per 32-bit word, in order.
//!   Pseudo-instructions return several (`li` with a big constant, `call`,
//!   `la`, ...).
//! * The number of words never depends on a label's address (the assembler is
//!   single pass). It may depend on constants: `li a0, 5` is 1 word,
//!   `li a0, 0x12345678` is 2, and on RV64 `li a0, 0x123456789abcdef0` is 8.
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
//! 2. [`hi_lo64`] (and [`hi_lo`], its 32-bit form), the split of a value into
//!    a 20-bit upper part and a sign-extended 12-bit lower part, used by
//!    `%hi`/`%lo` and the linker-style patching in `asm.rs`.
//! 3. Operand evaluation (`Imm`, `imm12`, `imm20`, `branch_target`, ...):
//!    where range checks and relocation modifiers are handled.
//! 4. [`encode`] itself: real instructions grouped by format, then the
//!    pseudo-instructions, then `.insn`.
//! 5. [`li_steps`], LLVM's choice of instructions for `li`.

use crate::ast::{Expr, Modifier, Operand};
use crate::reloc::{Emit, Fixup, RelocKind, Target};
use crate::Ctx;
use excalibur::{csr, Reg, Xlen};

// Major opcodes, bits [6:0] (spec section 4): the decoder's own constants.
// custom-0 holds the Xcal instructions (spec section 9). OP-IMM-32 and OP-32
// are RV64's 32-bit operations (`addiw`, `addw`, ...).
use excalibur::opcode::{
    OP_32, OP_AMO as AMO, OP_AUIPC as AUIPC, OP_BRANCH as BRANCH, OP_CUSTOM0 as CUSTOM_0, OP_IMM, OP_IMM_32,
    OP_JAL as JAL, OP_JALR as JALR, OP_LOAD as LOAD, OP_LUI as LUI, OP_MISC_MEM as MISC_MEM, OP_OP as OP,
    OP_STORE as STORE, OP_SYSTEM as SYSTEM,
};

// funct3 values the pseudo-instructions need by name.
pub const ADDI: u32 = 0b000;
pub const SLTIU: u32 = 0b011;
pub const XORI: u32 = 0b100;
pub const ANDI: u32 = 0b111;
pub const SLLI: u32 = 0b001;
pub const BEQ: u32 = 0b000;
pub const BNE: u32 = 0b001;
pub const BLT: u32 = 0b100;
pub const BGE: u32 = 0b101;
pub const BLTU: u32 = 0b110;
pub const BGEU: u32 = 0b111;
pub const CSRRW: u32 = 0b001;
pub const CSRRS: u32 = 0b010;
pub const CSRRC: u32 = 0b011;
/// The immediate CSR forms (`csrrwi` ...) are the register forms with bit 2 of funct3 set.
pub const CSR_IMM: u32 = 0b100;
/// `srli`'s funct3, for the RV64 `li` sequences and `zext.w`.
pub const SRLI: u32 = 0b101;
/// The width of an A instruction, in funct3: `.w` (a word) or `.d` (a
/// doubleword, RV64).
pub const AMO_W: u32 = 0b010;
pub const AMO_D: u32 = 0b011;

/// The funct3 values above, by name, for tables generated from these
/// (the C assembler's, `crates/xtask/src/c_tables.rs`).
pub const NAMED_FUNCT3: &[(&str, u32)] = &[
    ("ADDI", ADDI),
    ("SLTIU", SLTIU),
    ("XORI", XORI),
    ("ANDI", ANDI),
    ("SLLI", SLLI),
    ("BEQ", BEQ),
    ("BNE", BNE),
    ("BLT", BLT),
    ("BGE", BGE),
    ("BLTU", BLTU),
    ("BGEU", BGEU),
    ("CSRRW", CSRRW),
    ("CSRRS", CSRRS),
    ("CSRRC", CSRRC),
    ("CSR_IMM", CSR_IMM),
    ("SRLI", SRLI),
    ("AMO_W", AMO_W),
    ("AMO_D", AMO_D),
];

// Registers the pseudo-instructions use.
const ZERO: Reg = 0;
const RA: Reg = 1;
const T1: Reg = 6;

// ===================================================================
// Field packers
//
// Every base instruction is one of six formats. They all keep opcode, rd,
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
/// [`hi_lo64`] is the same split for any 64-bit value; on a 32-bit one they agree.
pub fn hi_lo(value: i32) -> (u32, i32) {
    let lo = (value << 20) >> 20; // the low 12 bits, sign-extended
    let hi = (value.wrapping_sub(lo) as u32) >> 12; // exact: value - lo ends in 12 zero bits
    (hi, lo)
}

/// [`hi_lo`] for a 64-bit value, as LLVM computes `%hi(c)` and `%lo(c)` of a
/// constant (`RISCVAsmBackend`'s `fixup_riscv_hi20` and
/// `fixup_riscv_lo12_i`): `hi = ((c + 0x800) >> 12) & 0xfffff`, `lo` the low
/// 12 bits sign-extended. Bits above 31 are simply dropped, never an error:
/// `%hi(0x123456789)` is `0x23456`, `%lo` of it `0x789`.
pub fn hi_lo64(value: i64) -> (u32, i32) {
    let lo = (value << 52) >> 52; // the low 12 bits, sign-extended
    let hi = (value.wrapping_add(0x800) >> 12) as u32 & 0xf_ffff;
    (hi, lo as i32)
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
            // `%hi(0x12345678)`: no symbol, so split the number right here. Any
            // 64-bit constant, as LLVM takes.
            return match m {
                Modifier::Hi => Ok(Imm::Num(hi_lo64(v).0 as i64)),
                Modifier::Lo => Ok(Imm::Num(hi_lo64(v).1 as i64)),
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
        // A name only RV32 has: the high half of a 64-bit CSR (`cycleh`,
        // `mstatush`) or an odd `pmpcfg`, which RV64 does not have (LLVM:
        // "system register 'cycleh' is RV32 only"). Its number is still
        // accepted, as in LLVM.
        (Expr::Sym(name), None) if csr::parse(name, Xlen::Rv32).is_some() => {
            Err(format!("CSR '{name}' exists only on RV32"))
        }
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
/// (0100000 selects arithmetic). On RV64 the shift amount is 6 bits, imm[5:0],
/// and only imm[11:6] is left for the function (a funct6); the rows hold for
/// both, since every funct7 here has its lowest bit (imm[5]) clear.
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

// -------------------------------------------------------------------
// RV64I's and RV64M's own instructions, in the same shapes as the tables
// above: tables of their own, as the ISA manual gives them a chapter of their
// own. The C assembler (`kernel/user/as`) gets them as tables of their own too
// (`crates/xtask/src/c_tables.rs`).
// -------------------------------------------------------------------

/// OP-32: `rd = sext32(rs1 <op> rs2)` on the low 32 bits of the registers,
/// (mnemonic, funct3, funct7). The rows mirror [`OP_FUNCT`]'s for the
/// operations that have a 32-bit form; the M extension has funct7 = 1.
pub const OP_32_FUNCT: &[(&str, u32, u32)] = &[
    ("addw", 0b000, 0b0000000),
    ("subw", 0b000, 0b0100000),
    ("sllw", 0b001, 0b0000000),
    ("srlw", 0b101, 0b0000000),
    ("sraw", 0b101, 0b0100000),
    ("mulw", 0b000, 0b0000001),
    ("divw", 0b100, 0b0000001),
    ("divuw", 0b101, 0b0000001),
    ("remw", 0b110, 0b0000001),
    ("remuw", 0b111, 0b0000001),
];

/// OP-IMM-32 with a 12-bit immediate, (mnemonic, funct3): only `addiw`
/// (`sext.w` is `addiw rd, rs, 0`).
pub const OP_IMM_32_FUNCT3: &[(&str, u32)] = &[("addiw", ADDI)];

/// OP-IMM-32 shifts, (mnemonic, funct3, funct7): as [`SHIFT_FUNCT`], but the
/// shift amount stays 5 bits, since they shift a 32-bit value.
pub const SHIFT_32_FUNCT: &[(&str, u32, u32)] = &[
    ("slliw", SLLI, 0b0000000),
    ("srliw", 0b101, 0b0000000),
    ("sraiw", 0b101, 0b0100000),
];

/// RV64 loads, (mnemonic, funct3): `lwu`, a word zero-extended (`lw`
/// sign-extends it), and `ld`, a doubleword.
pub const LOAD_RV64_FUNCT3: &[(&str, u32)] = &[("lwu", 0b110), ("ld", 0b011)];

/// RV64 stores, (mnemonic, funct3): `sd`, a doubleword.
pub const STORE_RV64_FUNCT3: &[(&str, u32)] = &[("sd", 0b011)];

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

fn op_32_funct(m: &str) -> Option<(u32, u32)> {
    find2(OP_32_FUNCT, m)
}

fn op_imm_32_funct3(m: &str) -> Option<u32> {
    find(OP_IMM_32_FUNCT3, m)
}

fn shift_32_funct(m: &str) -> Option<(u32, u32)> {
    find2(SHIFT_32_FUNCT, m)
}

fn load_rv64_funct3(m: &str) -> Option<u32> {
    find(LOAD_RV64_FUNCT3, m)
}

fn store_rv64_funct3(m: &str) -> Option<u32> {
    find(STORE_RV64_FUNCT3, m)
}

/// Splits an A mnemonic into its base (`amoadd.w`), its funct5, its width
/// as funct3 ([`AMO_W`] or [`AMO_D`]) and its ordering bits aq|rl, from the
/// suffix: none, `.aq` (10), `.rl` (01) or `.aqrl` (11).
///
/// A `.d` form is its `.w` form with funct3 = 011: the same operation on a
/// doubleword, so [`ATOMIC_FUNCT5`] lists only the `.w` names.
fn atomic(m: &str) -> Option<(&str, u32, u32, u32)> {
    let (base, aqrl) = [(".aqrl", 0b11), (".aq", 0b10), (".rl", 0b01)]
        .into_iter()
        .find_map(|(suffix, bits)| Some((m.strip_suffix(suffix)?, bits)))
        .unwrap_or((m, 0b00));
    if let Some(funct5) = find(ATOMIC_FUNCT5, base) {
        return Some((base, funct5, AMO_W, aqrl));
    }
    let op = base.strip_suffix(".d")?; // `amoadd` of `amoadd.d`
    let funct5 = ATOMIC_FUNCT5
        .iter()
        .find(|e| e.0.strip_suffix(".w") == Some(op))?
        .1;
    Some((base, funct5, AMO_D, aqrl))
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
    // ----- R-type: OP and M, and their 32-bit forms OP-32 -----
    if let Some((opcode, (funct3, funct7))) = op_funct(m)
        .map(|f| (OP, f))
        .or_else(|| op_32_funct(m).map(|f| (OP_32, f)))
    {
        let [rd, rs1, rs2] = ops else {
            return usage("rd, rs1, rs2");
        };
        return one(r_type(opcode, funct3, funct7, reg(rd)?, reg(rs1)?, reg(rs2)?));
    }

    // ----- I-type: OP-IMM, and OP-IMM-32's addiw -----
    if let Some((opcode, funct3)) = op_imm_funct3(m)
        .map(|f| (OP_IMM, f))
        .or_else(|| op_imm_32_funct3(m).map(|f| (OP_IMM_32, f)))
    {
        let [rd, rs1, imm] = ops else {
            return usage("rd, rs1, imm");
        };
        let (rd, rs1, f) = (reg(rd)?, reg(rs1)?, imm12(expr(imm)?, ctx, false)?);
        let word = i_type(opcode, funct3, rd, rs1, f.value);
        return Ok(vec![f.emit(word)]);
    }
    // Shifts by an immediate. The shift amount has as many bits as it takes
    // to count the bits of the value shifted: 6 for the 64-bit registers, 5
    // for the 32-bit shifts of OP-IMM-32 (`slliw`...).
    if let Some((opcode, bits, (funct3, funct7))) = shift_funct(m)
        .map(|f| (OP_IMM, 6, f))
        .or_else(|| shift_32_funct(m).map(|f| (OP_IMM_32, 5, f)))
    {
        let [rd, rs1, shamt] = ops else {
            return usage("rd, rs1, shamt");
        };
        let shamt = uimm(expr(shamt)?, ctx, bits, "shift amount")?;
        // funct7 sits in imm[11:5], the shift amount in imm[5:0], where
        // funct7's bit 0, always clear, leaves room for its bit 5.
        let imm = (funct7 << 5 | shamt) as i32;
        return one(i_type(opcode, funct3, reg(rd)?, reg(rs1)?, imm));
    }

    // ----- loads (I-type) and stores (S-type), RV64's ld, lwu and sd too -----
    if let Some(funct3) = load_funct3(m).or_else(|| load_rv64_funct3(m)) {
        return load(m, funct3, ops, ctx);
    }
    if let Some(funct3) = store_funct3(m).or_else(|| store_rv64_funct3(m)) {
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
    // funct7 is funct5 followed by the aq and rl bits; funct3 is the width,
    // 010 (word) or 011 (doubleword). lr has no rs2 (the field is zero).
    if let Some((base, funct5, width, aqrl)) = atomic(m) {
        let (rd, rs2, addr) = match (base, ops) {
            ("lr.w" | "lr.d", [rd, addr]) => (rd, ZERO, addr),
            ("lr.w" | "lr.d", _) => return usage("rd, (rs1)"),
            (_, [rd, rs2, addr]) => (rd, reg(rs2)?, addr),
            _ => return usage("rd, rs2, (rs1)"),
        };
        let rs1 = atomic_address(m, addr)?;
        return one(r_type(AMO, width, funct5 << 2 | aqrl, reg(rd)?, rs1, rs2));
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
                // The value LLVM's `li` takes: any 64-bit value (`li a0,
                // 0xffffffff` is 2^32 - 1).
                Imm::Num(v) => Ok(li_words(rd, &li_steps(v))),
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
        // The missing comparisons, by swapping the sources, as for `bgt`:
        // sgt rd, a, b == slt rd, b, a (and `sgtu` is `sltu`).
        "sgt" | "sgtu" => {
            let [rd, rs, rt] = ops else {
                return usage("rd, rs, rt");
            };
            let (rd, rs, rt) = (reg(rd)?, reg(rs)?, reg(rt)?);
            let funct3 = if m == "sgt" { 0b010 } else { 0b011 }; // slt's, sltu's
            one(r_type(OP, funct3, 0, rd, rt, rs))
        }
        // The 32-bit forms: sign-extend the low word, or negate it.
        "sext.w" | "negw" => {
            let [rd, rs] = ops else { return usage("rd, rs") };
            let (rd, rs) = (reg(rd)?, reg(rs)?);
            one(match m {
                "sext.w" => i_type(OP_IMM_32, ADDI, rd, rs, 0), // addiw rd, rs, 0
                _ => r_type(OP_32, 0, 0b0100000, rd, ZERO, rs), // negw: subw rd, zero, rs
            })
        }
        // Zero-extend the low word: without Zba's `add.uw`, two shifts, as
        // LLVM expands it. slli rd, rs, 32; srli rd, rd, 32.
        "zext.w" => {
            let [rd, rs] = ops else { return usage("rd, rs") };
            let (rd, rs) = (reg(rd)?, reg(rs)?);
            Ok(vec![
                Emit::word(i_type(OP_IMM, SLLI, rd, rs, 32)),
                Emit::word(i_type(OP_IMM, SRLI, rd, rd, 32)),
            ])
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
            let counter = match m {
                "rdcycle" => csr::CYCLE,
                "rdtime" => csr::TIME,
                "rdinstret" => csr::INSTRET,
                "rdcycleh" => csr::CYCLEH,
                "rdtimeh" => csr::TIMEH,
                _ => csr::INSTRETH,
            };
            // The `h` forms read the high half of a 64-bit counter on RV32.
            // RV64 has no such CSRs: its `rdcycle` reads the whole counter.
            if csr::rv32_only(counter) {
                let full = m.strip_suffix('h').unwrap_or(m);
                return Err(format!(
                    "'{m}' exists only on RV32 (on RV64, {full} reads all 64 bits)"
                ));
            }
            let [rd] = ops else { return usage("rd") };
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

// ===================================================================
// `li`: which instructions build a constant
// ===================================================================

/// One instruction of a `li` sequence. The first one starts from `zero` (or,
/// for `lui`, from nothing); each later one works on `rd`, the value built so
/// far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// `lui rd, imm20`: `imm20 << 12`, sign-extended from bit 31.
    Lui(u32),
    /// `addi rd, rs, imm`, a 12-bit signed immediate.
    Addi(i32),
    /// `addiw rd, rs, imm`: the sum's low 32 bits, sign-extended.
    Addiw(i32),
    /// `slli rd, rd, shamt`.
    Slli(u32),
    /// `srli rd, rd, shamt`.
    Srli(u32),
    /// `xori rd, rd, imm`, with imm always -1: `not rd, rd`.
    Xori(i32),
}

/// True if `v` is a `bits`-bit signed value: sign-extending its low `bits`
/// bits gives it back (LLVM's `isInt<bits>`).
fn fits(v: i64, bits: u32) -> bool {
    (v << (64 - bits)) >> (64 - bits) == v
}

/// The instructions `li` builds `value` with: exactly LLVM's choice,
/// `RISCVMatInt::generateInstSeq` (llvm/lib/Target/RISCV/MCTargetDesc/
/// RISCVMatInt.cpp), for the base ISA. With Zba, Zbb or Zbs LLVM has more
/// instructions to try (`bseti`, `rori`, `sh1add`, ...), and with C it would
/// weigh the compressed ones; merlin-as has none of them, so the cost of a
/// sequence is its length.
///
/// A 32-bit value takes one or two instructions:
///
/// * fits in 12 signed bits: `addi rd, zero, value`;
/// * low 12 bits zero: `lui rd, value >> 12`;
/// * an even value `c << s` with `c` in -32..=31: `addi rd, zero, c` +
///   `slli rd, rd, s` (step 2 below: `li a0, 2048` is `addi a0, zero, 1;
///   slli a0, a0, 11`);
/// * anything else: `lui rd, hi` + `addi rd, rd, lo` (or `addiw`), split by
///   [`hi_lo64`].
///
/// Any 64-bit value takes from 1 to 8 instructions
/// (`li a0, 0x123456789abcdef0` is `lui`, `addi`, then `slli`+`addi` three
/// times). The length depends only on the value, so `li` with a constant
/// keeps the assembler single pass.
///
/// LLVM builds a first sequence (step 1), then tries other shapes and keeps
/// one only if it is shorter:
///
/// 1. [`li_base`]: `lui` + `addi` for a 32-bit value; for a wider one, the
///    low 12 bits are an `addi` at the end and the rest is built shifted
///    right, then shifted back with `slli`.
/// 2. Trailing zeros: build `value >> tz` and add one `slli tz`. Kept when
///    shorter, or when the shifted value fits in 6 bits, where with C both
///    would be 16-bit instructions (LLVM prefers that shape even without C,
///    "to minimize differences in generated code").
///
///    One or two instructions cannot be beaten: every 32-bit value stops here.
/// 3. Low 13 bits from 0x1001 to 0x17ff: build `value + (0x800 - low 12
///    bits)`, whose low bits are then 0x1800 (one `addi -0x800` short of
///    more trailing zeros), and finish with an `addi` that takes it back.
/// 4. A positive value: build it shifted all the way left, the bits shifted
///    in made ones (which can make it a small negative number) or zeros, and
///    finish with `srli` by the leading zeros: [`li_leading_zeros`].
///    `0xffffffff` is `addi -1; srli 32`.
/// 5. A negative value: build its complement as in 4 and finish with
///    `xori -1` (`not`).
pub fn li_steps(value: i64) -> Vec<Step> {
    // 1.
    let mut best = li_base(value);

    // 2. Trailing zeros. (`value & 0xfff != 0`: with the low 12 bits zero,
    // step 1 already shifted them away.)
    if value & 0xfff != 0 && value & 1 == 0 && best.len() >= 2 {
        let tz = value.trailing_zeros();
        let shifted = value >> tz;
        let mut seq = li_base(shifted);
        if seq.len() + 1 < best.len() || fits(shifted, 6) {
            seq.push(Step::Slli(tz));
            best = seq;
        }
    }
    if best.len() <= 2 {
        return best;
    }

    // 3. Low 13 bits 0x1001..=0x17ff: add 0x800 - (value & 0xfff) to make
    // them 0x1800, which the next `addi -0x800` turns into 13 zero bits.
    if value & 0xfff != 0 && value & 0x1800 == 0x1000 {
        let imm12 = -(0x800 - (value & 0xfff)); // -0x7ff..=-1
        let mut seq = li_base(value.wrapping_sub(imm12));
        if seq.len() + 1 < best.len() {
            seq.push(Step::Addi(imm12 as i32));
            best = seq;
        }
    }

    // 4. Leading zeros of a positive value.
    if value > 0 && best.len() > 2 {
        li_leading_zeros(value, &mut best);
    }

    // 5. A negative value as the complement of a positive one. (Only when it
    // could save something: the complement's sequence plus the `xori` must
    // beat what we have.)
    if value < 0 && best.len() > 3 {
        let mut seq = Vec::new();
        li_leading_zeros(!value, &mut seq);
        if !seq.is_empty() && seq.len() + 1 < best.len() {
            seq.push(Step::Xori(-1));
            best = seq;
        }
    }
    best
}

/// LLVM's `generateInstSeqImpl`: the plain sequence for `value`.
///
/// A 32-bit value is `lui hi` + `addi lo` ([`hi_lo64`]), either one left out
/// when it is zero (`addi` stays if both are: `li a0, 0`). `lui`
/// sign-extends from bit 31, so `lui 0x80000` + `addi -1` would give
/// 0xffffffff_7fffffff, not 0x7fffffff: when the 64-bit sum leaves the 32-bit
/// range (only for 0x7ffff800..=0x7fffffff), the second instruction is
/// `addiw`, which sign-extends the 32-bit sum instead.
///
/// A wider value is built from the most significant bits down,
/// though worked out from the least significant up: its low 12 bits become an
/// `addi` at the end (taking them away can carry into bit 12, as in
/// [`hi_lo64`]), and what is left is shifted right past its trailing zeros
/// and built the same way, recursively, followed by the `slli` that shifts it
/// back. At most three rounds of `slli` + `addi` follow the first two
/// instructions. When the shift is more than 12 and the shifted value is too
/// wide for one `addi`, 12 bits less of shift leave its low 12 bits zero, for
/// a `lui` to make in one instruction.
fn li_base(value: i64) -> Vec<Step> {
    let mut seq = Vec::new();
    li_base_into(value, &mut seq);
    seq
}

fn li_base_into(value: i64, seq: &mut Vec<Step>) {
    let (hi20, lo12) = hi_lo64(value);
    if fits(value, 32) {
        if hi20 != 0 {
            seq.push(Step::Lui(hi20));
        }
        if lo12 != 0 || hi20 == 0 {
            let lui = (hi20 << 12) as i32 as i64; // what lui leaves, sign-extended
            let addiw = hi20 != 0 && !fits(lui + lo12 as i64, 32);
            seq.push(if addiw {
                Step::Addiw(lo12)
            } else {
                Step::Addi(lo12)
            });
        }
        return;
    }
    // Wrapping: 0x7fffffff_fffff800 - (-0x800) is 2^63.
    let mut rest = value.wrapping_sub(lo12 as i64);
    let mut shift = 0;
    if !fits(rest, 32) {
        shift = rest.trailing_zeros(); // rest is not 0: 0 fits
        rest >>= shift;
        if shift > 12 && !fits(rest, 12) && fits(rest << 12, 32) {
            shift -= 12;
            rest <<= 12;
        }
    }
    li_base_into(rest, seq);
    if shift != 0 {
        seq.push(Step::Slli(shift));
    }
    if lo12 != 0 {
        seq.push(Step::Addi(lo12));
    }
}

/// LLVM's `generateInstSeqLeadingZeros`, for `value > 0`: shift it left past
/// its leading zeros, build that, and `srli` it back. The bits the shift
/// brings in are free to choose, since `srli` drops them: all ones is tried
/// first (`0x0000ffff_ffffffff` becomes -1, one `addi`), then all zeros. Each
/// replaces `best` if it is shorter, or if `best` is empty (step 5 of
/// [`li_steps`]) and it is a sequence at all (at most 7 + the `srli`).
fn li_leading_zeros(value: i64, best: &mut Vec<Step>) {
    debug_assert!(value > 0);
    let lz = value.leading_zeros(); // 1..=63
    let low = ((1u64 << lz) - 1) as i64; // the lz bits shifted in
    let ones = value << lz | low;
    for shifted in [ones, ones & !low] {
        let mut seq = li_base(shifted);
        if seq.len() + 1 < best.len() || (best.is_empty() && seq.len() < 8) {
            seq.push(Step::Srli(lz));
            *best = seq;
        }
    }
}

/// The words of a `li` sequence into `rd`.
fn li_words(rd: Reg, steps: &[Step]) -> Vec<Emit> {
    let mut src = ZERO; // the first instruction starts from zero, the others from rd
    steps
        .iter()
        .map(|&step| {
            let word = match step {
                Step::Lui(imm20) => u_type(LUI, rd, imm20),
                Step::Addi(imm) => i_type(OP_IMM, ADDI, rd, src, imm),
                Step::Addiw(imm) => i_type(OP_IMM_32, ADDI, rd, src, imm),
                Step::Slli(shamt) => i_type(OP_IMM, SLLI, rd, src, shamt as i32),
                Step::Srli(shamt) => i_type(OP_IMM, SRLI, rd, src, shamt as i32),
                Step::Xori(imm) => i_type(OP_IMM, XORI, rd, src, imm),
            };
            src = rd;
            Emit::word(word)
        })
        .collect()
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
/// (`.insn j` is left out: LLVM, 21 and 23 alike, packs its numeric offsets unscrambled.)
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
    use excalibur::{AluOp, BranchOp, Inst, StoreOp, Xlen};

    fn decode(w: u32) -> Inst {
        excalibur::decode(w, Xlen::Rv64)
    }

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
                    imm: (imm20 << 12) as i32
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
        let words = |v: i32| {
            li_words(10, &li_steps(v as i64))
                .iter()
                .map(|e| e.word)
                .collect::<Vec<_>>()
        };
        assert_eq!(words(0), [0x0000_0513]);
        assert_eq!(words(0x1000_0000), [0x1000_0537]);
        assert_eq!(words(2048), [0x0010_0513, 0x00b5_1513]);
        assert_eq!(words(0x1234_5678), [0x1234_5537, 0x6785_0513]);
        // Odd, so no shift form even though small after the rounding.
        assert_eq!(words(0x801), [0x0000_1537, 0x8015_0513]);
    }

    // ----- RV64 -----

    /// The `.text` words of `src`, or the first error.
    fn words_for(src: &str) -> Result<Vec<u32>, String> {
        let obj = crate::assemble(src).map_err(|e| e[0].msg.clone())?;
        let text = &obj.sections[0].data;
        Ok(text
            .chunks(4)
            .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
            .collect())
    }

    fn words64(src: &str) -> Vec<u32> {
        words_for(src).unwrap_or_else(|e| panic!("{src}: {e}"))
    }

    /// Runs a `li` sequence the way the hart would.
    fn run(steps: &[Step]) -> i64 {
        let word = |v: i64| v;
        steps.iter().fold(0, |rd, &step| match step {
            Step::Lui(imm20) => (imm20 << 12) as i32 as i64,
            Step::Addi(imm) => word(rd.wrapping_add(imm as i64)),
            Step::Addiw(imm) => rd.wrapping_add(imm as i64) as i32 as i64,
            Step::Slli(s) => word(rd << s),
            Step::Srli(s) => word(((rd as u64) >> s) as i64),
            Step::Xori(imm) => word(rd ^ imm as i64),
        })
    }

    /// Words written as llvm-mc prints them: hex, separated by spaces.
    fn hex_words(s: &str) -> Vec<u32> {
        s.split_whitespace()
            .map(|w| u32::from_str_radix(w, 16).unwrap())
            .collect()
    }

    /// `li a0, value` on RV64: llvm-mc 23's words (`llvm-mc --triple=riscv64
    /// -mattr=+m,+a,+zicsr,+zifencei`): a sample with every rule of
    /// [`li_steps`] and its edges, grouped by the rule whose sequence wins.
    const LI64: &[(u64, &str)] = &[
        // Rule 1, a 32-bit value: addi alone, lui alone, or lui + addi.
        (0x0, "00000513"),
        (0x1, "00100513"),
        (0xffff_ffff_ffff_ffff, "fff00513"),
        (0x7ff, "7ff00513"),
        (0xffff_ffff_ffff_f800, "80000513"),
        (0x1000_0000, "10000537"),
        (0x1234_5678, "12345537 67850513"),
        // Odd, so no shift form even though small after the rounding.
        (0x801, "00001537 80150513"),
        // A sign-extended word: lui alone.
        (0xffff_ffff_8000_0000, "80000537"),
        // ... with addiw only where lui + addi would leave the 32-bit range.
        (0x7fff_f7ff, "7ffff537 7ff50513"),
        (0x7fff_f800, "80000537 8005051b"),
        (0x7fff_ffff, "80000537 fff5051b"),
        // Rule 1, wider than 32 bits: built shifted right, then slli (+ addi).
        // Above 2^31 lui alone no longer does, since RV64's lui sign-extends.
        (0x8000_0000, "00100513 01f51513"),
        (0x8000_0001, "00100513 01f51513 00150513"),
        (0x8040_0000, "20100513 01651513"),
        (0xffff_f800, "00100513 02051513 80050513"),
        (0x1_0000_0000, "00100513 02051513"),
        (0x1_0000_0001, "00100513 02051513 00150513"),
        (0x8000_0000_0000_0000, "fff00513 03f51513"),
        // Negative, and three instructions: too short for rule 5 to try.
        (0xffff_fffe_ffff_ffff, "fff00513 02051513 fff50513"),
        // The final addi of a wide value is addi, never addiw.
        (0xffff_ffff_7fff_ffff, "80000537 fff50513"),
        // addiw inside the recursion.
        (0x7fff_ffff_0000_0000, "80000537 fff5051b 02051513"),
        // A shift of more than 12 reduced by 12 for lui (14 to 2, 32 to 20);
        // a shift of exactly 12 is kept.
        (0xdead_beef, "37ab7537 00251513 eef50513"),
        (0x17ff_0000_0001, "017ff537 01451513 00150513"),
        (0x1_ffff_f000, "00200537 fff50513 00c51513"),
        // The longest: lui, addi, then slli + addi three times.
        (
            0x1234_5678_9abc_def0,
            "00247537 8ad50513 00e51513 c4d50513 00c51513 5e750513 00d51513 ef050513",
        ),
        (
            0x5555_5555_5555_5555,
            "05555537 55550513 00c51513 55550513 00c51513 55550513 00c51513 55550513",
        ),
        (
            0x00ff_00ff_00ff_00ff,
            "00ff0537 0ff50513 01051513 0ff50513 01051513 0ff50513",
        ),
        // Rule 2, trailing zeros: the shifted value small ("compressible"),
        // or the sequence shorter.
        (0x800, "00100513 00b51513"),
        (0x8000_0800, "00100537 00150513 00b51513"),
        (0x8000_2b04, "20001537 ac150513 00251513"),
        // Low 13 bits 0x1800, just past rule 3's range.
        (0x1_0000_0000_1800, "00100513 02551513 00350513 00b51513"),
        (
            0xfedc_ba98_7654_3210,
            "fdb97537 53150513 00d51513 d9550513 00e51513 32150513 00451513",
        ),
        // Rule 3, low 13 bits in 0x1001..=0x17ff: one more addi at the end.
        (0x905_acf7_102b, "482d6537 7b950513 00d51513 80050513 82b50513"),
        // In the range too, but rule 1's sequence is no longer.
        (0x1000_0000_17ff, "00100513 02051513 00150513 00c51513 7ff50513"),
        // Rule 4, leading zeros, the bits shifted in made ones: often -1
        // shifted right.
        (0xffff_ffff, "fff00513 02055513"),
        (0x7fff_ffff_ffff_ffff, "fff00513 00155513"),
        (0x0000_ffff_ffff_ffff, "fff00513 01055513"),
        (0x0000_ffff_0000_ffff, "f0001537 01451513 fff50513 01055513"),
        // value - lo12 wraps past 2^63 in rule 1's try.
        (0x7fff_ffff_ffff_f800, "fffff537 00155513"),
        // Rule 4, the bits shifted in made zeros.
        (0x0000_0fff_ffff_f000, "fff00513 02051513 01455513"),
        // Rule 5, negative values built inverted, then xori -1.
        (0x8000_0000_0000_0fff, "ffffe537 00155513 fff54513"),
        (0x8000_0000_0040_6100, "ff7f4537 dff50513 00155513 fff54513"),
        (
            0x8000_4c06_2000_0088,
            "d9fcf537 01251513 eef50513 00155513 fff54513",
        ),
        (
            0xfffa_ccc3_9887_ffff,
            "a6679537 cef50513 02051513 00d55513 fff54513",
        ),
        (0xffff_c000_42e9_8fff, "bd167537 01251513 01255513 fff54513"),
    ];

    #[test]
    fn li_steps_are_llvms() {
        for &(value, words) in LI64 {
            let words = hex_words(words);
            let steps = li_steps(value as i64);
            let got: Vec<u32> = li_words(10, &steps).iter().map(|e| e.word).collect();
            assert_eq!(got, words, "li a0, {value:#x}: {steps:?}");
            assert_eq!(run(&steps), value as i64, "{value:#x}: {steps:?}");
            // And through the assembler.
            assert_eq!(words64(&format!("li a0, {value:#x}")), words, "{value:#x}");
        }
    }

    #[test]
    fn li_takes_the_width_of_the_value() {
        // 0xffffffff is 2^32 - 1, not the word -1.
        assert_eq!(words64("li a0, 0xffffffff"), [0xfff0_0513, 0x0205_5513]);
        // `li sp, 0x80400000`, the stack of examples linked at 0x80000000.
        assert_eq!(words64("li sp, 0x80400000"), [0x2010_0113, 0x0161_1113]);
        assert_eq!(words64("li a0, 0x100000000"), [0x0010_0513, 0x0205_1513]);
        // 64-bit expressions wrap, as in LLVM: -2^63 - 1 is 2^63 - 1.
        assert_eq!(
            words64("li a0, -0x8000000000000000 - 1"),
            [0xfff0_0513, 0x0015_5513]
        );
    }

    /// RV64 instructions and their words from llvm-mc 23 (`--triple=riscv64
    /// -mattr=+m,+a,+zicsr,+zifencei -show-encoding`), with what
    /// `excalibur::decode` reads back from each word on RV64.
    #[test]
    fn the_64_bit_instructions_are_llvms_and_decode_back() {
        use excalibur::{AmoOp, AmoWidth, CsrOp, CsrSrc, LoadOp, MulOp, Ordering};
        // The instructions below use rd = a0 (10), rs1 = a1 (11), rs2 = a2 (12)
        // unless said otherwise; the atomics take their address in a2.
        let op32 = |op, rs1, rs2| Inst::Op32 { op, rd: 10, rs1, rs2 };
        let mul32 = |op| Inst::Mul32 {
            op,
            rd: 10,
            rs1: 11,
            rs2: 12,
        };
        let imm32 = |op, imm| Inst::OpImm32 {
            op,
            rd: 10,
            rs1: 11,
            imm,
        };
        let imm = |op, imm| Inst::OpImm {
            op,
            rd: 10,
            rs1: 11,
            imm,
        };
        let load = |op, rd, imm| Inst::Load { op, rd, rs1: 2, imm };
        let sd = |rs2, imm| Inst::Store {
            op: StoreOp::Sd,
            rs1: 2,
            rs2,
            imm,
        };
        let order = |aq, rl| Ordering { aq, rl };
        let lr_d = |rd, rs1, order| Inst::Lr {
            width: AmoWidth::D,
            rd,
            rs1,
            order,
        };
        let sc_d = |order| Inst::Sc {
            width: AmoWidth::D,
            rd: 10,
            rs1: 12,
            rs2: 11,
            order,
        };
        let amo = |op, width, order| Inst::Amo {
            op,
            width,
            rd: 10,
            rs1: 12,
            rs2: 11,
            order,
        };
        let csrr = |csr| Inst::Csr {
            op: CsrOp::Rs,
            rd: 10,
            src: CsrSrc::Reg(0),
            csr,
        };
        let (none, aq, rl, aqrl) = (
            Ordering::NONE,
            order(true, false),
            order(false, true),
            order(true, true),
        );
        let cases: &[(&str, u32, Inst)] = &[
            ("addw a0, a1, a2", 0x00c5_853b, op32(AluOp::Add, 11, 12)),
            ("subw a0, a1, a2", 0x40c5_853b, op32(AluOp::Sub, 11, 12)),
            ("sllw a0, a1, a2", 0x00c5_953b, op32(AluOp::Sll, 11, 12)),
            ("srlw a0, a1, a2", 0x00c5_d53b, op32(AluOp::Srl, 11, 12)),
            ("sraw a0, a1, a2", 0x40c5_d53b, op32(AluOp::Sra, 11, 12)),
            ("mulw a0, a1, a2", 0x02c5_853b, mul32(MulOp::Mul)),
            ("divw a0, a1, a2", 0x02c5_c53b, mul32(MulOp::Div)),
            ("divuw a0, a1, a2", 0x02c5_d53b, mul32(MulOp::Divu)),
            ("remw a0, a1, a2", 0x02c5_e53b, mul32(MulOp::Rem)),
            ("remuw a0, a1, a2", 0x02c5_f53b, mul32(MulOp::Remu)),
            ("addiw a0, a1, -2048", 0x8005_851b, imm32(AluOp::Add, -2048)),
            ("addiw a0, a1, 2047", 0x7ff5_851b, imm32(AluOp::Add, 2047)),
            ("slliw a0, a1, 31", 0x01f5_951b, imm32(AluOp::Sll, 31)),
            ("srliw a0, a1, 1", 0x0015_d51b, imm32(AluOp::Srl, 1)),
            ("sraiw a0, a1, 31", 0x41f5_d51b, imm32(AluOp::Sra, 31)),
            // 6-bit shift amounts.
            ("slli a0, a1, 63", 0x03f5_9513, imm(AluOp::Sll, 63)),
            ("srli a0, a1, 32", 0x0205_d513, imm(AluOp::Srl, 32)),
            ("srai a0, a1, 63", 0x43f5_d513, imm(AluOp::Sra, 63)),
            ("srai a0, a1, 32", 0x4205_d513, imm(AluOp::Sra, 32)),
            // Doublewords, and the zero-extended word.
            ("ld a0, -8(sp)", 0xff81_3503, load(LoadOp::Ld, 10, -8)),
            ("ld ra, -2048(sp)", 0x8001_3083, load(LoadOp::Ld, 1, -2048)),
            ("lwu a0, 4(sp)", 0x0041_6503, load(LoadOp::Lwu, 10, 4)),
            ("sd a0, 8(sp)", 0x00a1_3423, sd(10, 8)),
            ("sd ra, 2040(sp)", 0x7e11_3c23, sd(1, 2040)),
            // The .d atomics, with every ordering suffix.
            ("lr.d a0, (a1)", 0x1005_b52f, lr_d(10, 11, none)),
            ("lr.d.aq a0, (a1)", 0x1405_b52f, lr_d(10, 11, aq)),
            ("lr.d.rl a0, (a1)", 0x1205_b52f, lr_d(10, 11, rl)),
            ("lr.d.aqrl a0, (a1)", 0x1605_b52f, lr_d(10, 11, aqrl)),
            ("lr.d zero, 0(x31)", 0x100f_b02f, lr_d(0, 31, none)),
            ("sc.d a0, a1, (a2)", 0x18b6_352f, sc_d(none)),
            ("sc.d.aqrl a0, a1, (a2)", 0x1eb6_352f, sc_d(aqrl)),
            (
                "amoswap.d a0, a1, (a2)",
                0x08b6_352f,
                amo(AmoOp::Swap, AmoWidth::D, none),
            ),
            (
                "amoadd.d.aq a0, a1, (a2)",
                0x04b6_352f,
                amo(AmoOp::Add, AmoWidth::D, aq),
            ),
            (
                "amoxor.d.rl a0, a1, (a2)",
                0x22b6_352f,
                amo(AmoOp::Xor, AmoWidth::D, rl),
            ),
            (
                "amoand.d a0, a1, (a2)",
                0x60b6_352f,
                amo(AmoOp::And, AmoWidth::D, none),
            ),
            (
                "amoor.d.aqrl a0, a1, (a2)",
                0x46b6_352f,
                amo(AmoOp::Or, AmoWidth::D, aqrl),
            ),
            (
                "amomin.d a0, a1, (a2)",
                0x80b6_352f,
                amo(AmoOp::Min, AmoWidth::D, none),
            ),
            (
                "amomax.d a0, a1, (a2)",
                0xa0b6_352f,
                amo(AmoOp::Max, AmoWidth::D, none),
            ),
            (
                "amominu.d a0, a1, (a2)",
                0xc0b6_352f,
                amo(AmoOp::Minu, AmoWidth::D, none),
            ),
            (
                "amomaxu.d.rl a0, a1, (a2)",
                0xe2b6_352f,
                amo(AmoOp::Maxu, AmoWidth::D, rl),
            ),
            // The .w forms: the word's AMOs.
            (
                "amoadd.w a0, a1, (a2)",
                0x00b6_252f,
                amo(AmoOp::Add, AmoWidth::W, none),
            ),
            // Pseudo-instructions.
            ("sext.w a0, a1", 0x0005_851b, imm32(AluOp::Add, 0)),
            ("negw a0, a1", 0x40b0_053b, op32(AluOp::Sub, 0, 11)),
            ("mv a0, a1", 0x0005_8513, imm(AluOp::Add, 0)),
            ("not a0, a1", 0xfff5_c513, imm(AluOp::Xor, -1)),
            // CSRs: a number is accepted, RV32-only or not.
            ("csrr a0, 0xc80", 0xc800_2573, csrr(0xc80)),
            ("csrr a0, pmpcfg2", 0x3a20_2573, csrr(0x3a2)),
        ];
        for &(line, word, inst) in cases {
            assert_eq!(words64(line), [word], "{line}");
            assert_eq!(excalibur::decode(word, Xlen::Rv64), inst, "{line}");
            // The disassembler's text assembles back to the same word.
            let text = inst
                .format(None, excalibur::Syntax::plain(Xlen::Rv64))
                .to_string();
            assert_eq!(words64(&text), [word], "{line} -> {text}");
        }
    }

    #[test]
    fn zext_w_is_two_shifts() {
        // LLVM 23 without Zba: slli a0, a1, 32; srli a0, a0, 32.
        assert_eq!(words64("zext.w a0, a1"), [0x0205_9513, 0x0205_5513]);
    }

    #[test]
    fn rv64_loads_and_stores_reach_symbols_pc_relative() {
        // As llvm-mc: PCREL_HI20 on the auipc, PCREL_LO12_I/S on the access.
        for (line, words, lo) in [
            ("ld a0, target", [0x0000_0517, 0x0005_3503], RelocKind::PcrelLo12I),
            (
                "lwu a0, target",
                [0x0000_0517, 0x0005_6503],
                RelocKind::PcrelLo12I,
            ),
            (
                "sd a0, target, t0",
                [0x0000_0297, 0x00a2_b023],
                RelocKind::PcrelLo12S,
            ),
        ] {
            let obj = crate::assemble(line).unwrap();
            assert_eq!(words64(line), words, "{line}");
            let kinds: Vec<_> = obj.sections[0]
                .relocs
                .iter()
                .map(|r| (r.offset, r.kind))
                .collect();
            assert_eq!(kinds, [(0, RelocKind::PcrelHi20), (4, lo)], "{line}");
            assert_eq!(obj.sections[0].relocs[0].symbol, "target");
        }
    }

    #[test]
    fn hi_lo_of_a_constant_takes_64_bits() {
        // LLVM's words: any 64-bit constant.
        assert_eq!(words64("lui a0, %hi(0x123456789)"), [0x2345_6537]);
        assert_eq!(words64("addi a0, a0, %lo(0x123456789)"), [0x7895_0513]);
        assert_eq!(words64("lui a0, %hi(0xffffffff80000000)"), [0x8000_0537]);
        assert_eq!(words64("addi a0, a0, %lo(0xffffffff7ffff800)"), [0x8005_0513]);
        assert_eq!(words64("lui a0, %hi(-1)"), [0x0000_0537]);
        assert_eq!(words64("addi a0, a0, %lo(-1)"), [0xfff5_0513]);
        assert_eq!(words64("lui a0, %hi(0x7fffffffffffffff)"), [0x0000_0537]);
        // hi_lo64 agrees with hi_lo on every 32-bit value.
        for v in [
            0,
            1,
            -1,
            0x7ff,
            0x800,
            -0x801,
            i32::MIN,
            i32::MAX,
            0x7fff_f800,
            0x1234_5fff,
        ] {
            assert_eq!(hi_lo64(v as i64), hi_lo(v), "{v:#x}");
        }
    }

    #[test]
    fn shift_amounts_have_the_width_of_the_register() {
        assert_eq!(words64("slli a0, a0, 63"), [0x03f5_1513]);
        let rv64 = |line: &str| words_for(line);
        assert_eq!(
            rv64("slli a0, a0, 64"),
            Err("shift amount 64 out of range (0..=63)".into())
        );
        assert_eq!(
            rv64("srai a0, a0, -1"),
            Err("shift amount -1 out of range (0..=63)".into())
        );
        // The 32-bit shifts keep 5 bits.
        assert_eq!(
            rv64("slliw a0, a0, 32"),
            Err("shift amount 32 out of range (0..=31)".into())
        );
        assert_eq!(
            rv64("addiw a0, a0, 2048"),
            Err("immediate 2048 out of range (-2048..=2047)".into())
        );
    }
}
