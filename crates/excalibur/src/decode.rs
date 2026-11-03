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
/// OP-IMM-32 (RV64): `addiw`, `slliw`, `srliw`, `sraiw`.
pub const OP_IMM_32: u32 = 0b0011011;
pub const OP_OP: u32 = 0b0110011;
/// OP-32 (RV64): `addw`, `subw`, the 32-bit shifts, and M's `mulw` .. `remuw`.
pub const OP_32: u32 = 0b0111011;
pub const OP_MISC_MEM: u32 = 0b0001111;
pub const OP_SYSTEM: u32 = 0b1110011;
pub const OP_CUSTOM0: u32 = 0b0001011;
pub const OP_AMO: u32 = 0b0101111;

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
/// U-type immediate: the upper 20 bits, low 12 zero, as a signed value
/// (RV64 sign-extends it from bit 31).
#[inline]
fn imm_u(w: u32) -> i32 {
    (w & 0xffff_f000) as i32
}
/// J-type immediate: [31]=20, [19:12]=19:12, [20]=11, [30:21]=10:1.
#[inline]
fn imm_j(w: u32) -> i32 {
    (((w & 0x8000_0000) as i32) >> 11)
        | (w & 0x000f_f000) as i32
        | ((w >> 9) & 0x800) as i32
        | ((w >> 20) & 0x7fe) as i32
}

/// Decodes one 32-bit instruction word for a hart of width `xlen`. An
/// encoding that the width does not have (`ld` on RV32, `slli` with a 6-bit
/// shift amount on RV32, ...) is [`Inst::Illegal`].
pub fn decode(w: u32, xlen: Xlen) -> Inst {
    let illegal = Inst::Illegal(w);
    let rv64 = xlen == Xlen::Rv64;
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
                3 if rv64 => LoadOp::Ld,
                4 => LoadOp::Lbu,
                5 => LoadOp::Lhu,
                6 if rv64 => LoadOp::Lwu,
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
                3 if rv64 => StoreOp::Sd,
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
            // Shifts: on RV64 a 6-bit amount in [25:20] under funct6 [31:26];
            // on RV32 a 5-bit one under funct7, so bit 25 must be 0 there.
            let shift = if rv64 {
                let (funct6, shamt) = (w >> 26, ((w >> 20) & 63) as i32);
                match (funct3(w), funct6) {
                    (1, 0b000000) => Some((AluOp::Sll, shamt)),
                    (5, 0b000000) => Some((AluOp::Srl, shamt)),
                    (5, 0b010000) => Some((AluOp::Sra, shamt)),
                    _ => None,
                }
            } else {
                let shamt = rs2(w) as i32;
                match (funct3(w), funct7(w)) {
                    (1, 0b0000000) => Some((AluOp::Sll, shamt)),
                    (5, 0b0000000) => Some((AluOp::Srl, shamt)),
                    (5, 0b0100000) => Some((AluOp::Sra, shamt)),
                    _ => None,
                }
            };
            let (op, imm) = match funct3(w) {
                0 => (AluOp::Add, imm_i(w)),
                2 => (AluOp::Slt, imm_i(w)),
                3 => (AluOp::Sltu, imm_i(w)),
                4 => (AluOp::Xor, imm_i(w)),
                6 => (AluOp::Or, imm_i(w)),
                7 => (AluOp::And, imm_i(w)),
                _ => match shift {
                    Some(s) => s,
                    None => return illegal,
                },
            };
            Inst::OpImm {
                op,
                rd: rd(w),
                rs1: rs1(w),
                imm,
            }
        }
        // RV64's 32-bit immediate forms: the shifts keep a 5-bit amount, so a
        // set bit 25 is reserved, as is every funct3 but these three.
        OP_IMM_32 if rv64 => {
            let shamt = rs2(w) as i32;
            let (op, imm) = match (funct3(w), funct7(w)) {
                (0, _) => (AluOp::Add, imm_i(w)),
                (1, 0b0000000) => (AluOp::Sll, shamt),
                (5, 0b0000000) => (AluOp::Srl, shamt),
                (5, 0b0100000) => (AluOp::Sra, shamt),
                _ => return illegal,
            };
            Inst::OpImm32 {
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
        // RV64's 32-bit register forms, and M's: `mulw` and the divides (there
        // is no 32-bit `mulh*`: funct3 1-3 with M are reserved).
        OP_32 if rv64 => {
            let (rd, rs1, rs2) = (rd(w), rs1(w), rs2(w));
            if funct7(w) == 0b0000001 {
                let op = match funct3(w) {
                    0 => MulOp::Mul,
                    4 => MulOp::Div,
                    5 => MulOp::Divu,
                    6 => MulOp::Rem,
                    7 => MulOp::Remu,
                    _ => return illegal,
                };
                return Inst::Mul32 { op, rd, rs1, rs2 };
            }
            let op = match (funct3(w), funct7(w)) {
                (0, 0b0000000) => AluOp::Add,
                (0, 0b0100000) => AluOp::Sub,
                (1, 0b0000000) => AluOp::Sll,
                (5, 0b0000000) => AluOp::Srl,
                (5, 0b0100000) => AluOp::Sra,
                _ => return illegal,
            };
            Inst::Op32 { op, rd, rs1, rs2 }
        }
        // A extension: funct5 in [31:27], aq in [26], rl in [25], and funct3
        // = 010 (word) or 011 (doubleword, RV64). Unknown funct5 values
        // (amocas, ...) and an lr whose rs2 field is not zero are reserved.
        OP_AMO if funct3(w) == 2 || (funct3(w) == 3 && rv64) => {
            let (rd, rs1, rs2) = (rd(w), rs1(w), rs2(w));
            let width = if funct3(w) == 2 { AmoWidth::W } else { AmoWidth::D };
            let order = Ordering {
                aq: (w >> 26) & 1 != 0,
                rl: (w >> 25) & 1 != 0,
            };
            let op = match w >> 27 {
                0b00010 if rs2 == 0 => {
                    return Inst::Lr {
                        width,
                        rd,
                        rs1,
                        order,
                    }
                }
                0b00011 => {
                    return Inst::Sc {
                        width,
                        rd,
                        rs1,
                        rs2,
                        order,
                    }
                }
                0b00001 => AmoOp::Swap,
                0b00000 => AmoOp::Add,
                0b00100 => AmoOp::Xor,
                0b01100 => AmoOp::And,
                0b01000 => AmoOp::Or,
                0b10000 => AmoOp::Min,
                0b10100 => AmoOp::Max,
                0b11000 => AmoOp::Minu,
                0b11100 => AmoOp::Maxu,
                _ => return illegal,
            };
            Inst::Amo {
                op,
                width,
                rd,
                rs1,
                rs2,
                order,
            }
        }
        OP_MISC_MEM => match funct3(w) {
            0 if w >> 20 == 0x833 => Inst::FenceTso,
            0 => Inst::Fence {
                pred: ((w >> 24) & 0xf) as u8,
                succ: ((w >> 20) & 0xf) as u8,
            },
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

    const RV32: Xlen = Xlen::Rv32;
    const RV64: Xlen = Xlen::Rv64;

    /// The same on both widths.
    fn both(w: u32) -> Inst {
        let d = decode(w, RV32);
        assert_eq!(decode(w, RV64), d, "{w:#010x}");
        d
    }

    #[test]
    fn immediates() {
        // addi a0, a0, -1
        assert_eq!(
            both(0xfff50513),
            Inst::OpImm {
                op: AluOp::Add,
                rd: 10,
                rs1: 10,
                imm: -1
            }
        );
        // sw a1, -4(sp)
        assert_eq!(
            both(0xfeb12e23),
            Inst::Store {
                op: StoreOp::Sw,
                rs1: 2,
                rs2: 11,
                imm: -4
            }
        );
        // beq zero, zero, -8
        assert_eq!(
            both(0xfe000ce3),
            Inst::Branch {
                op: BranchOp::Beq,
                rs1: 0,
                rs2: 0,
                imm: -8
            }
        );
        // jal ra, 2048
        assert_eq!(both(0x001000ef), Inst::Jal { rd: 1, imm: 2048 });
        // jal zero, -2
        assert_eq!(both(0xfffff06f), Inst::Jal { rd: 0, imm: -2 });
        // lui t0, 0x80000: the immediate is negative as a 32-bit value, so
        // RV64 sign-extends it.
        assert_eq!(
            both(0x800002b7),
            Inst::Lui {
                rd: 5,
                imm: 0x8000_0000u32 as i32
            }
        );
    }

    #[test]
    fn system() {
        assert_eq!(both(0x00000073), Inst::Ecall);
        assert_eq!(both(0x30200073), Inst::Mret);
        assert_eq!(both(0x10500073), Inst::Wfi);
        // csrrw a0, mscratch, a0
        assert_eq!(
            both(0x34051573),
            Inst::Csr {
                op: CsrOp::Rw,
                rd: 10,
                src: CsrSrc::Reg(10),
                csr: 0x340
            }
        );
        assert!(matches!(both(0), Inst::Illegal(0)));
    }

    #[test]
    fn fences() {
        assert_eq!(both(0x0ff0000f), Inst::Fence { pred: 0xf, succ: 0xf });
        assert_eq!(both(0x0210000f), Inst::Fence { pred: 0x2, succ: 0x1 });
        assert_eq!(both(0x8330000f), Inst::FenceTso);
        assert_eq!(both(0x0000100f), Inst::FenceI);
        // Reserved fm / rd / rs1 values execute as a normal fence.
        assert_eq!(both(0x8ff0000f), Inst::Fence { pred: 0xf, succ: 0xf });
        assert_eq!(both(0x0ff5008f), Inst::Fence { pred: 0xf, succ: 0xf });
    }

    #[test]
    fn atomics() {
        // Encodings from clang -march=rv32ima.
        let none = Ordering::NONE;
        let aqrl = Ordering { aq: true, rl: true };
        let w = AmoWidth::W;
        // lr.w a0, (a1) / lr.w.aqrl a0, (a1)
        assert_eq!(
            both(0x1005a52f),
            Inst::Lr {
                width: w,
                rd: 10,
                rs1: 11,
                order: none
            }
        );
        assert_eq!(
            both(0x1605a52f),
            Inst::Lr {
                width: w,
                rd: 10,
                rs1: 11,
                order: aqrl
            }
        );
        // sc.w.aqrl t0, t1, (t2)
        assert_eq!(
            both(0x1e63a2af),
            Inst::Sc {
                width: w,
                rd: 5,
                rs1: 7,
                rs2: 6,
                order: aqrl
            }
        );
        // amoadd.w.aq a0, a1, (a2), amoand.w.rl a0, a1, (a2)
        assert_eq!(
            both(0x04b6252f),
            Inst::Amo {
                op: AmoOp::Add,
                width: w,
                rd: 10,
                rs1: 12,
                rs2: 11,
                order: Ordering { aq: true, rl: false }
            }
        );
        assert_eq!(
            both(0x62b6252f),
            Inst::Amo {
                op: AmoOp::And,
                width: w,
                rd: 10,
                rs1: 12,
                rs2: 11,
                order: Ordering { aq: false, rl: true }
            }
        );
        let ops = [
            (0x08b6252f, AmoOp::Swap),
            (0x20b6252f, AmoOp::Xor),
            (0x40b6252f, AmoOp::Or),
            (0x80b6252f, AmoOp::Min),
            (0xa0b6252f, AmoOp::Max),
            (0xc0b6252f, AmoOp::Minu),
            (0xe0b6252f, AmoOp::Maxu),
        ];
        for (word, op) in ops {
            let want = Inst::Amo {
                op,
                width: w,
                rd: 10,
                rs1: 12,
                rs2: 11,
                order: none,
            };
            assert_eq!(both(word), want, "{word:#010x}");
        }
        // Reserved: lr.w with rs2 != 0, unknown funct5 (amocas.w is Zacas),
        // and funct3 other than .w and .d.
        for word in [0x1015a52f, 0x2805a52f, 0xf805a52f, 0x0005c52f] {
            assert_eq!(both(word), Inst::Illegal(word), "{word:#010x}");
        }
    }

    #[test]
    fn rv64_loads_and_stores() {
        // From clang -march=rv64ima: ld a0, -8(sp); lwu a1, 4(a0); sd ra, 8(sp).
        assert_eq!(
            decode(0xff813503, RV64),
            Inst::Load {
                op: LoadOp::Ld,
                rd: 10,
                rs1: 2,
                imm: -8
            }
        );
        assert_eq!(
            decode(0x00456583, RV64),
            Inst::Load {
                op: LoadOp::Lwu,
                rd: 11,
                rs1: 10,
                imm: 4
            }
        );
        assert_eq!(
            decode(0x00113423, RV64),
            Inst::Store {
                op: StoreOp::Sd,
                rs1: 2,
                rs2: 1,
                imm: 8
            }
        );
        // None of them exists on RV32.
        for word in [0xff813503, 0x00456583, 0x00113423] {
            assert_eq!(decode(word, RV32), Inst::Illegal(word), "{word:#010x}");
        }
    }

    #[test]
    fn shift_amounts_by_width() {
        // slli a0, a0, 63 (RV64) has bit 25 set: on RV32 that is reserved,
        // which riscv-tests' rv32mi-p-shamt checks.
        let slli63 = 0x03f51513;
        assert_eq!(
            decode(slli63, RV64),
            Inst::OpImm {
                op: AluOp::Sll,
                rd: 10,
                rs1: 10,
                imm: 63
            }
        );
        assert_eq!(decode(slli63, RV32), Inst::Illegal(slli63));
        // srai a0, a0, 40 and srli a0, a0, 33.
        assert_eq!(
            decode(0x42855513, RV64),
            Inst::OpImm {
                op: AluOp::Sra,
                rd: 10,
                rs1: 10,
                imm: 40
            }
        );
        assert_eq!(
            decode(0x02155513, RV64),
            Inst::OpImm {
                op: AluOp::Srl,
                rd: 10,
                rs1: 10,
                imm: 33
            }
        );
        // A funct6 that is not a shift's is reserved on RV64.
        assert_eq!(decode(0x08051513, RV64), Inst::Illegal(0x08051513));
        // Shift amounts below 32 mean the same on both.
        assert_eq!(
            both(0x01f51513),
            Inst::OpImm {
                op: AluOp::Sll,
                rd: 10,
                rs1: 10,
                imm: 31
            }
        );
    }

    #[test]
    fn rv64_word_instructions() {
        let cases = [
            // addiw a0, a1, -1 / slliw a0, a1, 31 / srliw / sraiw a0, a1, 1
            (
                0xfff5851b,
                Inst::OpImm32 {
                    op: AluOp::Add,
                    rd: 10,
                    rs1: 11,
                    imm: -1,
                },
            ),
            (
                0x01f5951b,
                Inst::OpImm32 {
                    op: AluOp::Sll,
                    rd: 10,
                    rs1: 11,
                    imm: 31,
                },
            ),
            (
                0x0015d51b,
                Inst::OpImm32 {
                    op: AluOp::Srl,
                    rd: 10,
                    rs1: 11,
                    imm: 1,
                },
            ),
            (
                0x4015d51b,
                Inst::OpImm32 {
                    op: AluOp::Sra,
                    rd: 10,
                    rs1: 11,
                    imm: 1,
                },
            ),
            // addw a0, a1, a2 / subw / sllw / srlw / sraw
            (
                0x00c5853b,
                Inst::Op32 {
                    op: AluOp::Add,
                    rd: 10,
                    rs1: 11,
                    rs2: 12,
                },
            ),
            (
                0x40c5853b,
                Inst::Op32 {
                    op: AluOp::Sub,
                    rd: 10,
                    rs1: 11,
                    rs2: 12,
                },
            ),
            (
                0x00c5953b,
                Inst::Op32 {
                    op: AluOp::Sll,
                    rd: 10,
                    rs1: 11,
                    rs2: 12,
                },
            ),
            (
                0x00c5d53b,
                Inst::Op32 {
                    op: AluOp::Srl,
                    rd: 10,
                    rs1: 11,
                    rs2: 12,
                },
            ),
            (
                0x40c5d53b,
                Inst::Op32 {
                    op: AluOp::Sra,
                    rd: 10,
                    rs1: 11,
                    rs2: 12,
                },
            ),
            // mulw / divw / divuw / remw / remuw a0, a1, a2
            (
                0x02c5853b,
                Inst::Mul32 {
                    op: MulOp::Mul,
                    rd: 10,
                    rs1: 11,
                    rs2: 12,
                },
            ),
            (
                0x02c5c53b,
                Inst::Mul32 {
                    op: MulOp::Div,
                    rd: 10,
                    rs1: 11,
                    rs2: 12,
                },
            ),
            (
                0x02c5d53b,
                Inst::Mul32 {
                    op: MulOp::Divu,
                    rd: 10,
                    rs1: 11,
                    rs2: 12,
                },
            ),
            (
                0x02c5e53b,
                Inst::Mul32 {
                    op: MulOp::Rem,
                    rd: 10,
                    rs1: 11,
                    rs2: 12,
                },
            ),
            (
                0x02c5f53b,
                Inst::Mul32 {
                    op: MulOp::Remu,
                    rd: 10,
                    rs1: 11,
                    rs2: 12,
                },
            ),
        ];
        for (word, want) in cases {
            assert_eq!(decode(word, RV64), want, "{word:#010x}");
            // OP-IMM-32 and OP-32 do not exist on RV32.
            assert_eq!(decode(word, RV32), Inst::Illegal(word), "{word:#010x}");
        }
        // Reserved: slliw with bit 25 set, an OP-IMM-32 funct3 that is not
        // 0, 1 or 5, OP-32 M with funct3 1-3 (no 32-bit mulh), and an OP-32
        // funct7 that is not 0, 0x20 or 1.
        for word in [
            0x0205951b, 0x0005a51b, 0x02c5953b, 0x02c5a53b, 0x02c5b53b, 0x80c5853b, 0x00c5a53b,
        ] {
            assert_eq!(decode(word, RV64), Inst::Illegal(word), "{word:#010x}");
        }
    }

    #[test]
    fn rv64_doubleword_atomics() {
        // lr.d a0, (a1) / sc.d.aqrl t0, t1, (t2) / amoadd.d.aq a0, a1, (a2)
        let d = AmoWidth::D;
        assert_eq!(
            decode(0x1005b52f, RV64),
            Inst::Lr {
                width: d,
                rd: 10,
                rs1: 11,
                order: Ordering::NONE
            }
        );
        assert_eq!(
            decode(0x1e63b2af, RV64),
            Inst::Sc {
                width: d,
                rd: 5,
                rs1: 7,
                rs2: 6,
                order: Ordering { aq: true, rl: true }
            }
        );
        assert_eq!(
            decode(0x04b6352f, RV64),
            Inst::Amo {
                op: AmoOp::Add,
                width: d,
                rd: 10,
                rs1: 12,
                rs2: 11,
                order: Ordering { aq: true, rl: false }
            }
        );
        // The .d forms do not exist on RV32.
        for word in [0x1005b52f, 0x1e63b2af, 0x04b6352f] {
            assert_eq!(decode(word, RV32), Inst::Illegal(word), "{word:#010x}");
        }
    }

    #[test]
    fn xcal() {
        // xcal.dbg a0 / xcal.halt a0
        assert_eq!(both(0x0005000b), Inst::XcalDbg { rs1: 10 });
        assert_eq!(both(0x0005100b), Inst::XcalHalt { rs1: 10 });
        // Non-zero rd or imm, or an unknown funct3, stays illegal.
        assert!(matches!(both(0x0005008b), Inst::Illegal(_)));
        assert!(matches!(both(0x0015000b), Inst::Illegal(_)));
        assert!(matches!(both(0x0005200b), Inst::Illegal(_)));
    }
}
