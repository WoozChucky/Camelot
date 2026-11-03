//! Excalibur ISA definitions.
//!
//! This crate is the single source of truth for instruction encodings. Grail
//! (the emulator) uses [`decode`] to execute, disassemblers use [`Inst::format`]
//! (or the plain [`std::fmt::Display`] impl on [`Inst`]), and merlin-as encodes
//! with the same major opcodes ([`opcode`]), CSR numbers ([`csr`]) and register
//! names ([`reg`]). Its funct3/funct7 tables are its own, kept honest by its
//! round-trip tests through [`decode`].
//!
//! Both widths of the ISA are here (spec sections 1-4): RV64, what Excalibur
//! is, and RV32, what it was, which the rest of the system keeps using until
//! it moves (#141). An instruction word means different things on the two:
//! `slli` takes a 6-bit shift amount on RV64 and a 5-bit one on RV32, and
//! `ld`, the W instructions and the `.d` atomics exist only on RV64. So
//! [`decode`] takes the width ([`Xlen`]), and so does [`Syntax`], since a
//! branch target wraps at the width and some CSR names exist only on RV32.

pub mod csr;
mod decode;
mod disasm;
pub mod reg;

pub use decode::decode;

/// Major opcodes, bits [6:0] of every instruction (spec section 4).
pub mod opcode {
    pub use crate::decode::{
        OP_32, OP_AMO, OP_AUIPC, OP_BRANCH, OP_CUSTOM0, OP_IMM, OP_IMM_32, OP_JAL, OP_JALR, OP_LOAD, OP_LUI,
        OP_MISC_MEM, OP_OP, OP_STORE, OP_SYSTEM,
    };
}
pub use disasm::{Disasm, Syntax};

/// Register index, 0..=31.
pub type Reg = u8;

/// The width of the integer registers, `XLEN`: what an instruction word means
/// depends on it (spec section 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Xlen {
    /// RV32: 32-bit registers (Excalibur before #141).
    Rv32,
    /// RV64: 64-bit registers.
    Rv64,
}

impl Xlen {
    /// The register width in bits.
    pub fn bits(self) -> u32 {
        match self {
            Xlen::Rv32 => 32,
            Xlen::Rv64 => 64,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchOp {
    Beq,
    Bne,
    Blt,
    Bge,
    Bltu,
    Bgeu,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadOp {
    Lb,
    Lh,
    /// On RV64, the word is sign-extended to 64 bits.
    Lw,
    Lbu,
    Lhu,
    /// RV64: the word, zero-extended.
    Lwu,
    /// RV64: a doubleword.
    Ld,
}

impl LoadOp {
    /// Access width in bytes.
    pub fn size(self) -> u32 {
        match self {
            LoadOp::Lb | LoadOp::Lbu => 1,
            LoadOp::Lh | LoadOp::Lhu => 2,
            LoadOp::Lw | LoadOp::Lwu => 4,
            LoadOp::Ld => 8,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreOp {
    Sb,
    Sh,
    Sw,
    /// RV64: a doubleword.
    Sd,
}

impl StoreOp {
    pub fn size(self) -> u32 {
        match self {
            StoreOp::Sb => 1,
            StoreOp::Sh => 2,
            StoreOp::Sw => 4,
            StoreOp::Sd => 8,
        }
    }
}

/// Integer ALU operations shared by OP and OP-IMM, and by their 32-bit forms
/// OP-32 and OP-IMM-32 (RV64), which use only `Add`, `Sub`, `Sll`, `Srl` and
/// `Sra`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AluOp {
    Add,
    Sub,
    Sll,
    Slt,
    Sltu,
    Xor,
    Srl,
    Sra,
    Or,
    And,
}

/// M extension operations. The 32-bit forms ([`Inst::Mul32`], RV64) use
/// only `Mul`, `Div`, `Divu`, `Rem` and `Remu`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MulOp {
    Mul,
    Mulh,
    Mulhsu,
    Mulhu,
    Div,
    Divu,
    Rem,
    Remu,
}

/// A extension read-modify-write operations (`amo<op>.w`, `amo<op>.d`):
/// memory gets `op(old, rs2)` and rd gets the old value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmoOp {
    Swap,
    Add,
    Xor,
    And,
    Or,
    /// Signed minimum.
    Min,
    /// Signed maximum.
    Max,
    Minu,
    Maxu,
}

impl AmoOp {
    /// Every operation.
    pub const ALL: [AmoOp; 9] = [
        AmoOp::Swap,
        AmoOp::Add,
        AmoOp::Xor,
        AmoOp::And,
        AmoOp::Or,
        AmoOp::Min,
        AmoOp::Max,
        AmoOp::Minu,
        AmoOp::Maxu,
    ];
}

/// The width of an A instruction: funct3 010 (`.w`) or 011 (`.d`, RV64).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmoWidth {
    /// A 32-bit word. On RV64 the loaded value is sign-extended into rd.
    W,
    /// A 64-bit doubleword (RV64).
    D,
}

impl AmoWidth {
    /// Access width in bytes.
    pub fn size(self) -> u32 {
        match self {
            AmoWidth::W => 4,
            AmoWidth::D => 8,
        }
    }
}

/// Memory-ordering bits of an A instruction (bits 26 and 25). Grail is a
/// single in-order hart, so they do not change what an instruction does; they
/// only change its encoding and its `.aq`/`.rl`/`.aqrl` mnemonic suffix.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ordering {
    /// Acquire: no later memory access is seen before this one.
    pub aq: bool,
    /// Release: no earlier memory access is seen after this one.
    pub rl: bool,
}

impl Ordering {
    /// Neither bit set.
    pub const NONE: Ordering = Ordering { aq: false, rl: false };

    /// The mnemonic suffix: `""`, `".aq"`, `".rl"` or `".aqrl"`.
    pub fn suffix(self) -> &'static str {
        match (self.aq, self.rl) {
            (false, false) => "",
            (true, false) => ".aq",
            (false, true) => ".rl",
            (true, true) => ".aqrl",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CsrOp {
    /// Read and write.
    Rw,
    /// Read and set bits.
    Rs,
    /// Read and clear bits.
    Rc,
}

/// Source operand of a CSR instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CsrSrc {
    Reg(Reg),
    /// 5-bit zero-extended immediate (the `csrr*i` forms).
    Imm(u8),
}

/// A decoded instruction. Immediates are already sign-extended and, for
/// branches and jumps, already byte offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inst {
    /// `imm` is the U-type immediate, its low 12 bits zero, as a signed
    /// value: RV64 sign-extends it from bit 31 (`lui t0, 0x80000` gives
    /// 0xFFFF_FFFF_8000_0000), and on RV32 its bits are the result.
    Lui {
        rd: Reg,
        imm: i32,
    },
    /// `imm` as for [`Inst::Lui`], added to the pc.
    Auipc {
        rd: Reg,
        imm: i32,
    },
    Jal {
        rd: Reg,
        imm: i32,
    },
    Jalr {
        rd: Reg,
        rs1: Reg,
        imm: i32,
    },
    Branch {
        op: BranchOp,
        rs1: Reg,
        rs2: Reg,
        imm: i32,
    },
    Load {
        op: LoadOp,
        rd: Reg,
        rs1: Reg,
        imm: i32,
    },
    Store {
        op: StoreOp,
        rs1: Reg,
        rs2: Reg,
        imm: i32,
    },
    /// OP-IMM. For shifts, `imm` holds the shift amount (up to 63 on
    /// RV64, 31 on RV32).
    OpImm {
        op: AluOp,
        rd: Reg,
        rs1: Reg,
        imm: i32,
    },
    /// OP-IMM-32 (RV64): `addiw`, `slliw`, `srliw`, `sraiw`. They compute on
    /// the low 32 bits of rs1 and sign-extend the result to 64. For shifts,
    /// `imm` holds the shift amount, 0 to 31.
    OpImm32 {
        op: AluOp,
        rd: Reg,
        rs1: Reg,
        imm: i32,
    },
    Op {
        op: AluOp,
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
    },
    /// OP-32 (RV64): `addw`, `subw`, `sllw`, `srlw`, `sraw`, on the low 32
    /// bits, the result sign-extended.
    Op32 {
        op: AluOp,
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
    },
    Mul {
        op: MulOp,
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
    },
    /// OP-32 with M (RV64): `mulw`, `divw`, `divuw`, `remw`, `remuw`.
    Mul32 {
        op: MulOp,
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
    },
    /// `lr.w rd, (rs1)` / `lr.d`: load-reserved. The encoding's rs2 field
    /// must be 0; anything else decodes as [`Inst::Illegal`].
    Lr {
        width: AmoWidth,
        rd: Reg,
        rs1: Reg,
        order: Ordering,
    },
    /// `sc.w rd, rs2, (rs1)` / `sc.d`: store-conditional. rd = 0 on
    /// success, 1 on failure.
    Sc {
        width: AmoWidth,
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        order: Ordering,
    },
    /// `amo<op>.w rd, rs2, (rs1)` / `amo<op>.d`: atomic memory operation.
    Amo {
        op: AmoOp,
        width: AmoWidth,
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        order: Ordering,
    },
    /// `pred`/`succ` are the 4-bit I/O/R/W sets (bit 3 = I ... bit 0 = W).
    /// Reserved encodings (non-zero rd/rs1, unknown `fm`) decode as a plain
    /// fence, as the base ISA requires.
    Fence {
        pred: u8,
        succ: u8,
    },
    /// `fence.tso` (fm = 1000, pred = succ = RW).
    FenceTso,
    FenceI,
    Ecall,
    Ebreak,
    Mret,
    Sret,
    Wfi,
    SfenceVma {
        rs1: Reg,
        rs2: Reg,
    },
    Csr {
        op: CsrOp,
        rd: Reg,
        src: CsrSrc,
        csr: u16,
    },
    /// Xcaldbg: print a register to the emulator console.
    XcalDbg {
        rs1: Reg,
    },
    /// Xcalhalt: stop the machine with exit code rs1 (M-mode only).
    XcalHalt {
        rs1: Reg,
    },
    /// Anything we do not recognise. Executing it raises illegal-instruction.
    Illegal(u32),
}
