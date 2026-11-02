//! Excalibur ISA definitions.
//!
//! This crate is the single source of truth for instruction encodings. Grail
//! (the emulator) uses [`decode`] to execute, disassemblers use [`Inst::format`]
//! (or the plain [`std::fmt::Display`] impl on [`Inst`]), and merlin-as will use the same
//! tables to encode.

pub mod csr;
mod decode;
mod disasm;
pub mod reg;

pub use decode::decode;
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
