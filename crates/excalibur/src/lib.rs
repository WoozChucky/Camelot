//! Excalibur ISA definitions.
//!
//! This crate is the single source of truth for instruction encodings. Grail
//! (the emulator) uses [`decode`] to execute, disassemblers use [`Inst::format`]
//! (or the plain [`std::fmt::Display`] impl on [`Inst`]), and merlin-as encodes
//! with the same major opcodes ([`opcode`]), CSR numbers ([`csr`]) and register
//! names ([`reg`]). Its funct3/funct7 tables are its own, kept honest by its
//! round-trip tests through [`decode`].

pub mod csr;
mod decode;
mod disasm;
pub mod reg;

pub use decode::decode;

/// Major opcodes, bits [6:0] of every instruction (spec section 4).
pub mod opcode {
    pub use crate::decode::{
        OP_AMO, OP_AUIPC, OP_BRANCH, OP_CUSTOM0, OP_IMM, OP_JAL, OP_JALR, OP_LOAD, OP_LUI, OP_MISC_MEM,
        OP_OP, OP_STORE, OP_SYSTEM,
    };
}
pub use disasm::{Disasm, Syntax};

/// Register index, 0..=31.
pub type Reg = u8;

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
    Lw,
    Lbu,
    Lhu,
}

impl LoadOp {
    /// Access width in bytes.
    pub fn size(self) -> u32 {
        match self {
            LoadOp::Lb | LoadOp::Lbu => 1,
            LoadOp::Lh | LoadOp::Lhu => 2,
            LoadOp::Lw => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreOp {
    Sb,
    Sh,
    Sw,
}

impl StoreOp {
    pub fn size(self) -> u32 {
        match self {
            StoreOp::Sb => 1,
            StoreOp::Sh => 2,
            StoreOp::Sw => 4,
        }
    }
}

/// Integer ALU operations shared by OP and OP-IMM.
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

/// M extension operations.
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

/// A extension read-modify-write operations (`amo<op>.w`): memory gets
/// `op(old, rs2)` and rd gets the old value.
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
    Lui {
        rd: Reg,
        imm: u32,
    },
    Auipc {
        rd: Reg,
        imm: u32,
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
    /// OP-IMM. For shifts, `imm` holds the shift amount.
    OpImm {
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
    Mul {
        op: MulOp,
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
    },
    /// `lr.w rd, (rs1)`: load-reserved word. The encoding's rs2 field must
    /// be 0; anything else decodes as [`Inst::Illegal`].
    Lr {
        rd: Reg,
        rs1: Reg,
        order: Ordering,
    },
    /// `sc.w rd, rs2, (rs1)`: store-conditional word. rd = 0 on success,
    /// 1 on failure.
    Sc {
        rd: Reg,
        rs1: Reg,
        rs2: Reg,
        order: Ordering,
    },
    /// `amo<op>.w rd, rs2, (rs1)`: atomic memory operation on a word.
    Amo {
        op: AmoOp,
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
