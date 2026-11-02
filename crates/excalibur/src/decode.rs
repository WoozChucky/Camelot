use crate::*;

// Major opcodes (bits [6:0]).
pub const OP_LUI: u32 = 0b0110111;
pub const OP_AUIPC: u32 = 0b0010111;
pub const OP_JAL: u32 = 0b1101111;
pub const OP_JALR: u32 = 0b1100111;
pub const OP_BRANCH: u32 = 0b1100011;
pub const OP_LOAD: u32 = 0b0000011;
pub const OP_STORE: u32 = 0b0100011;
pub const OP_IMM: u32 = 0b0010011;
pub const OP_OP: u32 = 0b0110011;
pub const OP_MISC_MEM: u32 = 0b0001111;
pub const OP_SYSTEM: u32 = 0b1110011;
pub const OP_CUSTOM0: u32 = 0b0001011;

#[inline]
fn rd(w: u32) -> Reg {
    ((w >> 7) & 31) as Reg
}
#[inline]
fn rs1(w: u32) -> Reg {
    ((w >> 15) & 31) as Reg
}
#[inline]
fn rs2(w: u32) -> Reg {
    ((w >> 20) & 31) as Reg
}
#[inline]
fn funct3(w: u32) -> u32 {
    (w >> 12) & 7
}
#[inline]
fn funct7(w: u32) -> u32 {
    w >> 25
}

/// I-type immediate: bits [31:20], sign-extended.
#[inline]
fn imm_i(w: u32) -> i32 {
    (w as i32) >> 20
}
/// S-type immediate: [31:25] and [11:7].
#[inline]
fn imm_s(w: u32) -> i32 {
    (((w & 0xfe00_0000) as i32) >> 20) | ((w >> 7) & 0x1f) as i32
}
/// B-type immediate: [31]=12, [7]=11, [30:25]=10:5, [11:8]=4:1.
#[inline]
fn imm_b(w: u32) -> i32 {
    (((w & 0x8000_0000) as i32) >> 19)
        | ((w & 0x80) << 4) as i32
        | ((w >> 20) & 0x7e0) as i32
        | ((w >> 7) & 0x1e) as i32
}
/// U-type immediate: upper 20 bits, low 12 zero.
#[inline]
fn imm_u(w: u32) -> u32 {
    w & 0xffff_f000
}
/// J-type immediate: [31]=20, [19:12]=19:12, [20]=11, [30:21]=10:1.
#[inline]
fn imm_j(w: u32) -> i32 {
    (((w & 0x8000_0000) as i32) >> 11)
        | (w & 0x000f_f000) as i32
        | ((w >> 9) & 0x800) as i32
        | ((w >> 20) & 0x7fe) as i32
}

/// Decodes one 32-bit instruction word.
pub fn decode(w: u32) -> Inst {
    let illegal = Inst::Illegal(w);
    // Only 32-bit instructions exist (no C extension): low two bits must be 11.
    if w & 3 != 3 {
        return illegal;
    }
    match w & 0x7f {
        OP_LUI => Inst::Lui {
            rd: rd(w),
            imm: imm_u(w),
        },
        OP_AUIPC => Inst::Auipc {
            rd: rd(w),
            imm: imm_u(w),
        },
        OP_JAL => Inst::Jal {
            rd: rd(w),
            imm: imm_j(w),
        },
        OP_JALR if funct3(w) == 0 => Inst::Jalr {
            rd: rd(w),
            rs1: rs1(w),
            imm: imm_i(w),
        },
        OP_BRANCH => {
            let op = match funct3(w) {
                0 => BranchOp::Beq,
                1 => BranchOp::Bne,
                4 => BranchOp::Blt,
                5 => BranchOp::Bge,
                6 => BranchOp::Bltu,
                7 => BranchOp::Bgeu,
                _ => return illegal,
            };
            Inst::Branch {
                op,
                rs1: rs1(w),
                rs2: rs2(w),
                imm: imm_b(w),
            }
        }
        OP_LOAD => {
            let op = match funct3(w) {
                0 => LoadOp::Lb,
                1 => LoadOp::Lh,
                2 => LoadOp::Lw,
                4 => LoadOp::Lbu,
                5 => LoadOp::Lhu,
                _ => return illegal,
            };
            Inst::Load {
                op,
                rd: rd(w),
                rs1: rs1(w),
                imm: imm_i(w),
            }
        }
        OP_STORE => {
            let op = match funct3(w) {
                0 => StoreOp::Sb,
                1 => StoreOp::Sh,
                2 => StoreOp::Sw,
                _ => return illegal,
            };
            Inst::Store {
                op,
                rs1: rs1(w),
                rs2: rs2(w),
                imm: imm_s(w),
            }
        }
        OP_IMM => {
            let shamt = rs2(w) as i32;
            let (op, imm) = match (funct3(w), funct7(w)) {
                (0, _) => (AluOp::Add, imm_i(w)),
                (2, _) => (AluOp::Slt, imm_i(w)),
                (3, _) => (AluOp::Sltu, imm_i(w)),
                (4, _) => (AluOp::Xor, imm_i(w)),
                (6, _) => (AluOp::Or, imm_i(w)),
                (7, _) => (AluOp::And, imm_i(w)),
                (1, 0b0000000) => (AluOp::Sll, shamt),
                (5, 0b0000000) => (AluOp::Srl, shamt),
                (5, 0b0100000) => (AluOp::Sra, shamt),
                _ => return illegal,
            };
            Inst::OpImm {
                op,
                rd: rd(w),
                rs1: rs1(w),
                imm,
            }
        }
        OP_OP => {
            let (rd, rs1, rs2) = (rd(w), rs1(w), rs2(w));
            if funct7(w) == 0b0000001 {
                let op = match funct3(w) {
                    0 => MulOp::Mul,
                    1 => MulOp::Mulh,
                    2 => MulOp::Mulhsu,
                    3 => MulOp::Mulhu,
                    4 => MulOp::Div,
                    5 => MulOp::Divu,
                    6 => MulOp::Rem,
                    _ => MulOp::Remu,
                };
                return Inst::Mul { op, rd, rs1, rs2 };
            }
            let op = match (funct3(w), funct7(w)) {
                (0, 0b0000000) => AluOp::Add,
                (0, 0b0100000) => AluOp::Sub,
                (1, 0b0000000) => AluOp::Sll,
                (2, 0b0000000) => AluOp::Slt,
                (3, 0b0000000) => AluOp::Sltu,
                (4, 0b0000000) => AluOp::Xor,
                (5, 0b0000000) => AluOp::Srl,
                (5, 0b0100000) => AluOp::Sra,
                (6, 0b0000000) => AluOp::Or,
                (7, 0b0000000) => AluOp::And,
                _ => return illegal,
            };
            Inst::Op { op, rd, rs1, rs2 }
        }
        OP_MISC_MEM => match funct3(w) {
            0 => Inst::Fence,
            1 => Inst::FenceI,
            _ => illegal,
        },
        OP_SYSTEM => {
            let csr = (w >> 20) as u16;
            let op = match funct3(w) & 3 {
                1 => CsrOp::Rw,
                2 => CsrOp::Rs,
                3 => CsrOp::Rc,
                _ => {
                    // funct3 == 0: privileged instructions. funct3 == 4 is reserved.
                    if funct3(w) != 0 {
                        return illegal;
                    }
                    if funct7(w) == 0b0001001 && rd(w) == 0 {
                        return Inst::SfenceVma {
                            rs1: rs1(w),
                            rs2: rs2(w),
                        };
                    }
                    if rd(w) != 0 || rs1(w) != 0 {
                        return illegal;
                    }
                    return match w >> 20 {
                        0x000 => Inst::Ecall,
                        0x001 => Inst::Ebreak,
                        0x102 => Inst::Sret,
                        0x302 => Inst::Mret,
                        0x105 => Inst::Wfi,
                        _ => illegal,
                    };
                }
            };
            let src = if funct3(w) & 4 != 0 {
                CsrSrc::Imm(rs1(w))
            } else {
                CsrSrc::Reg(rs1(w))
            };
            Inst::Csr {
                op,
                rd: rd(w),
                src,
                csr,
            }
        }
        // Xcal extensions: I-type with rd = 0 and imm = 0 (spec section 9).
        OP_CUSTOM0 if rd(w) == 0 && imm_i(w) == 0 => match funct3(w) {
            0 => Inst::XcalDbg { rs1: rs1(w) },
            1 => Inst::XcalHalt { rs1: rs1(w) },
            _ => illegal,
        },
        _ => illegal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immediates() {
        // addi a0, a0, -1
        assert_eq!(
            decode(0xfff50513),
            Inst::OpImm {
                op: AluOp::Add,
                rd: 10,
                rs1: 10,
                imm: -1
            }
        );
        // sw a1, -4(sp)
        assert_eq!(
            decode(0xfeb12e23),
            Inst::Store {
                op: StoreOp::Sw,
                rs1: 2,
                rs2: 11,
                imm: -4
            }
        );
        // beq zero, zero, -8
        assert_eq!(
            decode(0xfe000ce3),
            Inst::Branch {
                op: BranchOp::Beq,
                rs1: 0,
                rs2: 0,
                imm: -8
            }
        );
        // jal ra, 2048
        assert_eq!(decode(0x001000ef), Inst::Jal { rd: 1, imm: 2048 });
        // jal zero, -2
        assert_eq!(decode(0xfffff06f), Inst::Jal { rd: 0, imm: -2 });
        // lui t0, 0x80000
        assert_eq!(
            decode(0x800002b7),
            Inst::Lui {
                rd: 5,
                imm: 0x8000_0000
            }
        );
    }

    #[test]
    fn system() {
        assert_eq!(decode(0x00000073), Inst::Ecall);
        assert_eq!(decode(0x30200073), Inst::Mret);
        assert_eq!(decode(0x10500073), Inst::Wfi);
        // csrrw a0, mscratch, a0
        assert_eq!(
            decode(0x34051573),
            Inst::Csr {
                op: CsrOp::Rw,
                rd: 10,
                src: CsrSrc::Reg(10),
                csr: 0x340
            }
        );
        assert!(matches!(decode(0), Inst::Illegal(0)));
    }

    #[test]
    fn xcal() {
        // xcal.dbg a0 / xcal.halt a0
        assert_eq!(decode(0x0005000b), Inst::XcalDbg { rs1: 10 });
        assert_eq!(decode(0x0005100b), Inst::XcalHalt { rs1: 10 });
        // Non-zero rd or imm, or an unknown funct3, stays illegal.
        assert!(matches!(decode(0x0005008b), Inst::Illegal(_)));
        assert!(matches!(decode(0x0015000b), Inst::Illegal(_)));
        assert!(matches!(decode(0x0005200b), Inst::Illegal(_)));
    }
}
