//! Disassembly via `Display`. Output uses ABI register names and matches the
//! syntax merlin-as will accept.

use crate::reg::abi_name as r;
use crate::*;
use std::fmt;

impl fmt::Display for Inst {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Inst::Lui { rd, imm } => write!(f, "lui {}, {:#x}", r(rd), imm >> 12),
            Inst::Auipc { rd, imm } => write!(f, "auipc {}, {:#x}", r(rd), imm >> 12),
            Inst::Jal { rd, imm } => write!(f, "jal {}, {}", r(rd), imm),
            Inst::Jalr { rd, rs1, imm } => write!(f, "jalr {}, {}({})", r(rd), imm, r(rs1)),
            Inst::Branch { op, rs1, rs2, imm } => {
                let m = match op {
                    BranchOp::Beq => "beq",
                    BranchOp::Bne => "bne",
                    BranchOp::Blt => "blt",
                    BranchOp::Bge => "bge",
                    BranchOp::Bltu => "bltu",
                    BranchOp::Bgeu => "bgeu",
                };
                write!(f, "{m} {}, {}, {}", r(rs1), r(rs2), imm)
            }
            Inst::Load { op, rd, rs1, imm } => {
                let m = match op {
                    LoadOp::Lb => "lb",
                    LoadOp::Lh => "lh",
                    LoadOp::Lw => "lw",
                    LoadOp::Lbu => "lbu",
                    LoadOp::Lhu => "lhu",
                };
                write!(f, "{m} {}, {}({})", r(rd), imm, r(rs1))
            }
            Inst::Store { op, rs1, rs2, imm } => {
                let m = match op {
                    StoreOp::Sb => "sb",
                    StoreOp::Sh => "sh",
                    StoreOp::Sw => "sw",
                };
                write!(f, "{m} {}, {}({})", r(rs2), imm, r(rs1))
            }
            Inst::OpImm { op, rd, rs1, imm } => {
                let m = match op {
                    AluOp::Add => "addi",
                    AluOp::Slt => "slti",
                    AluOp::Sltu => "sltiu",
                    AluOp::Xor => "xori",
                    AluOp::Or => "ori",
                    AluOp::And => "andi",
                    AluOp::Sll => "slli",
                    AluOp::Srl => "srli",
                    AluOp::Sra => "srai",
                    AluOp::Sub => unreachable!("no subi encoding"),
                };
                write!(f, "{m} {}, {}, {}", r(rd), r(rs1), imm)
            }
            Inst::Op { op, rd, rs1, rs2 } => {
                let m = match op {
                    AluOp::Add => "add",
                    AluOp::Sub => "sub",
                    AluOp::Sll => "sll",
                    AluOp::Slt => "slt",
                    AluOp::Sltu => "sltu",
                    AluOp::Xor => "xor",
                    AluOp::Srl => "srl",
                    AluOp::Sra => "sra",
                    AluOp::Or => "or",
                    AluOp::And => "and",
                };
                write!(f, "{m} {}, {}, {}", r(rd), r(rs1), r(rs2))
            }
            Inst::Mul { op, rd, rs1, rs2 } => {
                let m = match op {
                    MulOp::Mul => "mul",
                    MulOp::Mulh => "mulh",
                    MulOp::Mulhsu => "mulhsu",
                    MulOp::Mulhu => "mulhu",
                    MulOp::Div => "div",
                    MulOp::Divu => "divu",
                    MulOp::Rem => "rem",
                    MulOp::Remu => "remu",
                };
                write!(f, "{m} {}, {}, {}", r(rd), r(rs1), r(rs2))
            }
            Inst::Fence => write!(f, "fence"),
            Inst::FenceI => write!(f, "fence.i"),
            Inst::Ecall => write!(f, "ecall"),
            Inst::Ebreak => write!(f, "ebreak"),
            Inst::Mret => write!(f, "mret"),
            Inst::Sret => write!(f, "sret"),
            Inst::Wfi => write!(f, "wfi"),
            Inst::SfenceVma { rs1, rs2 } => write!(f, "sfence.vma {}, {}", r(rs1), r(rs2)),
            Inst::Csr { op, rd, src, csr } => {
                let m = match (op, src) {
                    (CsrOp::Rw, CsrSrc::Reg(_)) => "csrrw",
                    (CsrOp::Rs, CsrSrc::Reg(_)) => "csrrs",
                    (CsrOp::Rc, CsrSrc::Reg(_)) => "csrrc",
                    (CsrOp::Rw, CsrSrc::Imm(_)) => "csrrwi",
                    (CsrOp::Rs, CsrSrc::Imm(_)) => "csrrsi",
                    (CsrOp::Rc, CsrSrc::Imm(_)) => "csrrci",
                };
                write!(f, "{m} {}, ", r(rd))?;
                match csr::name(csr) {
                    Some(n) => write!(f, "{n}, ")?,
                    None => write!(f, "{csr:#x}, ")?,
                }
                match src {
                    CsrSrc::Reg(s) => write!(f, "{}", r(s)),
                    CsrSrc::Imm(i) => write!(f, "{i}"),
                }
            }
            Inst::XcalDbg { rs1 } => write!(f, "xcal.dbg {}", r(rs1)),
            Inst::XcalHalt { rs1 } => write!(f, "xcal.halt {}", r(rs1)),
            Inst::Illegal(w) => write!(f, ".word {w:#010x}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::decode;

    #[test]
    fn formats() {
        assert_eq!(decode(0xfff50513).to_string(), "addi a0, a0, -1");
        assert_eq!(decode(0xfeb12e23).to_string(), "sw a1, -4(sp)");
        assert_eq!(decode(0x34051573).to_string(), "csrrw a0, mscratch, a0");
        assert_eq!(decode(0x800002b7).to_string(), "lui t0, 0x80000");
    }
}
