//! Disassembly. Output uses ABI register names and matches the syntax
//! merlin-as accepts.
//!
//! [`Inst::format`] is the general form: it can print pseudo-instructions
//! (`mv`, `ret`, `beqz`, ...) exactly where llvm-objdump does, hex
//! immediates, and absolute branch targets. `Display` is the plain form Grail's
//! trace uses: real instructions only, decimal immediates, branch offsets
//! relative to the instruction.

use crate::reg::abi_name as r;
use crate::*;
use std::fmt;

/// How [`Inst::format`] prints an instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Syntax {
    /// Print the pseudo-instruction llvm-objdump prints for this encoding
    /// (`nop`, `li`, `mv`, `not`, `neg`, `seqz`, `snez`, `sltz`, `sgtz`,
    /// `zext.b`, `beqz`/`bnez`/`blez`/`bgez`/`bltz`/`bgtz`, `j`, `jal off`,
    /// `jr`, `jalr rs`, `ret`, `fence`, `sfence.vma`, `csrr`/`csrw`/`csrs`/
    /// `csrc` and their `i` forms, `rdcycle`/`rdtime`/`rdinstret` and the
    /// `h` variants). `-M no-aliases` in llvm-objdump turns this off.
    pub aliases: bool,
    /// Immediates in hex (`0x7ff`, `-0x8`), llvm-objdump's default. In
    /// decimal mode, upper immediates (`lui`/`auipc`) and unnamed CSR numbers
    /// are still printed in hex because that is how people write them.
    pub hex: bool,
}

impl Syntax {
    /// What llvm-objdump prints by default.
    pub const OBJDUMP: Syntax = Syntax {
        aliases: true,
        hex: true,
    };
    /// What `Display` prints.
    pub const PLAIN: Syntax = Syntax {
        aliases: false,
        hex: false,
    };
}

/// A formatted instruction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disasm {
    pub mnemonic: &'static str,
    /// Comma-separated operands, empty when there are none.
    pub operands: String,
    /// For branches and `jal` formatted with a known address: the absolute
    /// target, already printed in `operands`. Tools add a `<symbol>` after it.
    pub target: Option<u32>,
}

impl fmt::Display for Disasm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.mnemonic)?;
        if !self.operands.is_empty() {
            write!(f, " {}", self.operands)?;
        }
        Ok(())
    }
}

/// `rdcycle` & co: `csrrs rd, <counter>, zero`.
fn counter_alias(csr: u16) -> Option<&'static str> {
    Some(match csr {
        csr::CYCLE => "rdcycle",
        csr::TIME => "rdtime",
        csr::INSTRET => "rdinstret",
        csr::CYCLEH => "rdcycleh",
        csr::TIMEH => "rdtimeh",
        csr::INSTRETH => "rdinstreth",
        _ => return None,
    })
}

/// An A-extension mnemonic with its ordering suffix. Mnemonics are
/// `&'static str`, so each one is spelled out four times: none, `.rl`, `.aq`,
/// `.aqrl` (indexed by the aq and rl bits).
macro_rules! ordered {
    ($m:literal, $order:expr) => {{
        let order: Ordering = $order;
        [$m, concat!($m, ".rl"), concat!($m, ".aq"), concat!($m, ".aqrl")]
            [order.aq as usize * 2 + order.rl as usize]
    }};
}

/// A fence's I/O/R/W set, `0` when empty.
fn fence_set(bits: u8) -> String {
    if bits & 0xf == 0 {
        return "0".to_string();
    }
    "iorw"
        .chars()
        .enumerate()
        .filter(|&(i, _)| bits & (8 >> i) != 0)
        .map(|(_, c)| c)
        .collect()
}

impl Inst {
    /// Formats the instruction. `pc` is its address: when given, branch and
    /// `jal` targets print as absolute addresses (`0x80000044`), otherwise as
    /// offsets.
    pub fn format(&self, pc: Option<u32>, syntax: Syntax) -> Disasm {
        let a = syntax.aliases;
        let imm = |v: i32| {
            if !syntax.hex {
                v.to_string()
            } else if v < 0 {
                format!("-{:#x}", v.unsigned_abs())
            } else {
                format!("{v:#x}")
            }
        };
        let mut target = None;
        let mut offset = |off: i32| match pc {
            Some(pc) => {
                let t = pc.wrapping_add(off as u32);
                target = Some(t);
                format!("{t:#x}")
            }
            None => imm(off),
        };
        let csr_name = |c: u16| match csr::name(c) {
            Some(n) => n.to_string(),
            None => format!("{c:#x}"),
        };

        let (mnemonic, operands): (&'static str, String) = match *self {
            Inst::Lui { rd, imm } => ("lui", format!("{}, {:#x}", r(rd), imm >> 12)),
            Inst::Auipc { rd, imm } => ("auipc", format!("{}, {:#x}", r(rd), imm >> 12)),
            Inst::Jal { rd, imm } => match rd {
                0 if a => ("j", offset(imm)),
                1 if a => ("jal", offset(imm)),
                _ => ("jal", format!("{}, {}", r(rd), offset(imm))),
            },
            Inst::Jalr { rd, rs1, imm: i } => match (rd, rs1, i) {
                (0, 1, 0) if a => ("ret", String::new()),
                (0, _, 0) if a => ("jr", r(rs1).to_string()),
                (1, _, 0) if a => ("jalr", r(rs1).to_string()),
                (0, _, _) if a => ("jr", format!("{}({})", imm(i), r(rs1))),
                (1, _, _) if a => ("jalr", format!("{}({})", imm(i), r(rs1))),
                (_, _, 0) if a => ("jalr", format!("{}, {}", r(rd), r(rs1))),
                _ => ("jalr", format!("{}, {}({})", r(rd), imm(i), r(rs1))),
            },
            Inst::Branch { op, rs1, rs2, imm } => {
                let t = offset(imm);
                // The order matters when both registers are zero: llvm-objdump
                // prints `bltz zero` and `blez zero`.
                let alias = match op {
                    BranchOp::Beq if rs2 == 0 => Some(("beqz", rs1)),
                    BranchOp::Bne if rs2 == 0 => Some(("bnez", rs1)),
                    BranchOp::Blt if rs2 == 0 => Some(("bltz", rs1)),
                    BranchOp::Blt if rs1 == 0 => Some(("bgtz", rs2)),
                    BranchOp::Bge if rs1 == 0 => Some(("blez", rs2)),
                    BranchOp::Bge if rs2 == 0 => Some(("bgez", rs1)),
                    _ => None,
                };
                match alias {
                    Some((m, rs)) if a => (m, format!("{}, {t}", r(rs))),
                    _ => {
                        let m = match op {
                            BranchOp::Beq => "beq",
                            BranchOp::Bne => "bne",
                            BranchOp::Blt => "blt",
                            BranchOp::Bge => "bge",
                            BranchOp::Bltu => "bltu",
                            BranchOp::Bgeu => "bgeu",
                        };
                        (m, format!("{}, {}, {t}", r(rs1), r(rs2)))
                    }
                }
            }
            Inst::Load { op, rd, rs1, imm: i } => {
                let m = match op {
                    LoadOp::Lb => "lb",
                    LoadOp::Lh => "lh",
                    LoadOp::Lw => "lw",
                    LoadOp::Lbu => "lbu",
                    LoadOp::Lhu => "lhu",
                };
                (m, format!("{}, {}({})", r(rd), imm(i), r(rs1)))
            }
            Inst::Store { op, rs1, rs2, imm: i } => {
                let m = match op {
                    StoreOp::Sb => "sb",
                    StoreOp::Sh => "sh",
                    StoreOp::Sw => "sw",
                };
                (m, format!("{}, {}({})", r(rs2), imm(i), r(rs1)))
            }
            Inst::OpImm { op, rd, rs1, imm: i } => {
                let alias = match (op, rd, rs1, i) {
                    (AluOp::Add, 0, 0, 0) => Some(("nop", String::new())),
                    (AluOp::Add, _, 0, _) => Some(("li", format!("{}, {}", r(rd), imm(i)))),
                    (AluOp::Add, _, _, 0) => Some(("mv", format!("{}, {}", r(rd), r(rs1)))),
                    (AluOp::Sltu, _, _, 1) => Some(("seqz", format!("{}, {}", r(rd), r(rs1)))),
                    (AluOp::Xor, _, _, -1) => Some(("not", format!("{}, {}", r(rd), r(rs1)))),
                    (AluOp::And, _, _, 255) => Some(("zext.b", format!("{}, {}", r(rd), r(rs1)))),
                    _ => None,
                };
                match alias {
                    Some(x) if a => x,
                    _ => {
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
                        (m, format!("{}, {}, {}", r(rd), r(rs1), imm(i)))
                    }
                }
            }
            Inst::Op { op, rd, rs1, rs2 } => {
                let alias = match (op, rs1, rs2) {
                    (AluOp::Sub, 0, _) => Some(("neg", rs2)),
                    (AluOp::Sltu, 0, _) => Some(("snez", rs2)),
                    (AluOp::Slt, _, 0) => Some(("sltz", rs1)),
                    (AluOp::Slt, 0, _) => Some(("sgtz", rs2)),
                    _ => None,
                };
                match alias {
                    Some((m, rs)) if a => (m, format!("{}, {}", r(rd), r(rs))),
                    _ => {
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
                        (m, format!("{}, {}, {}", r(rd), r(rs1), r(rs2)))
                    }
                }
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
                (m, format!("{}, {}, {}", r(rd), r(rs1), r(rs2)))
            }
            Inst::Lr { rd, rs1, order } => (ordered!("lr.w", order), format!("{}, ({})", r(rd), r(rs1))),
            Inst::Sc { rd, rs1, rs2, order } => (
                ordered!("sc.w", order),
                format!("{}, {}, ({})", r(rd), r(rs2), r(rs1)),
            ),
            Inst::Amo {
                op,
                rd,
                rs1,
                rs2,
                order,
            } => {
                let m = match op {
                    AmoOp::Swap => ordered!("amoswap.w", order),
                    AmoOp::Add => ordered!("amoadd.w", order),
                    AmoOp::Xor => ordered!("amoxor.w", order),
                    AmoOp::And => ordered!("amoand.w", order),
                    AmoOp::Or => ordered!("amoor.w", order),
                    AmoOp::Min => ordered!("amomin.w", order),
                    AmoOp::Max => ordered!("amomax.w", order),
                    AmoOp::Minu => ordered!("amominu.w", order),
                    AmoOp::Maxu => ordered!("amomaxu.w", order),
                };
                (m, format!("{}, {}, ({})", r(rd), r(rs2), r(rs1)))
            }
            Inst::Fence { pred: 0xf, succ: 0xf } if a => ("fence", String::new()),
            Inst::Fence { pred, succ } => ("fence", format!("{}, {}", fence_set(pred), fence_set(succ))),
            Inst::FenceTso => ("fence.tso", String::new()),
            Inst::FenceI => ("fence.i", String::new()),
            Inst::Ecall => ("ecall", String::new()),
            Inst::Ebreak => ("ebreak", String::new()),
            Inst::Mret => ("mret", String::new()),
            Inst::Sret => ("sret", String::new()),
            Inst::Wfi => ("wfi", String::new()),
            Inst::SfenceVma { rs1: 0, rs2: 0 } if a => ("sfence.vma", String::new()),
            Inst::SfenceVma { rs1, rs2: 0 } if a => ("sfence.vma", r(rs1).to_string()),
            Inst::SfenceVma { rs1, rs2 } => ("sfence.vma", format!("{}, {}", r(rs1), r(rs2))),
            // `unimp` is the canonical name of this encoding (llvm-objdump
            // prints it even with no-aliases).
            Inst::Csr {
                op: CsrOp::Rw,
                rd: 0,
                src: CsrSrc::Reg(0),
                csr: csr::CYCLE,
            } => ("unimp", String::new()),
            Inst::Csr { op, rd, src, csr } => {
                let c = csr_name(csr);
                let alias = match (op, rd, src) {
                    (CsrOp::Rs, _, CsrSrc::Reg(0)) => Some(match counter_alias(csr) {
                        Some(m) => (m, r(rd).to_string()),
                        None => ("csrr", format!("{}, {c}", r(rd))),
                    }),
                    (CsrOp::Rw, 0, CsrSrc::Reg(s)) => Some(("csrw", format!("{c}, {}", r(s)))),
                    (CsrOp::Rs, 0, CsrSrc::Reg(s)) => Some(("csrs", format!("{c}, {}", r(s)))),
                    (CsrOp::Rc, 0, CsrSrc::Reg(s)) => Some(("csrc", format!("{c}, {}", r(s)))),
                    (CsrOp::Rw, 0, CsrSrc::Imm(i)) => Some(("csrwi", format!("{c}, {}", imm(i as i32)))),
                    (CsrOp::Rs, 0, CsrSrc::Imm(i)) => Some(("csrsi", format!("{c}, {}", imm(i as i32)))),
                    (CsrOp::Rc, 0, CsrSrc::Imm(i)) => Some(("csrci", format!("{c}, {}", imm(i as i32)))),
                    _ => None,
                };
                match alias {
                    Some(x) if a => x,
                    _ => {
                        let m = match (op, src) {
                            (CsrOp::Rw, CsrSrc::Reg(_)) => "csrrw",
                            (CsrOp::Rs, CsrSrc::Reg(_)) => "csrrs",
                            (CsrOp::Rc, CsrSrc::Reg(_)) => "csrrc",
                            (CsrOp::Rw, CsrSrc::Imm(_)) => "csrrwi",
                            (CsrOp::Rs, CsrSrc::Imm(_)) => "csrrsi",
                            (CsrOp::Rc, CsrSrc::Imm(_)) => "csrrci",
                        };
                        let s = match src {
                            CsrSrc::Reg(s) => r(s).to_string(),
                            CsrSrc::Imm(i) => imm(i as i32),
                        };
                        (m, format!("{}, {c}, {s}", r(rd)))
                    }
                }
            }
            Inst::XcalDbg { rs1 } => ("xcal.dbg", r(rs1).to_string()),
            Inst::XcalHalt { rs1 } => ("xcal.halt", r(rs1).to_string()),
            Inst::Illegal(w) => (".word", format!("{w:#010x}")),
        };
        Disasm {
            mnemonic,
            operands,
            target,
        }
    }
}

impl fmt::Display for Inst {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.format(None, Syntax::PLAIN).fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::Syntax;
    use crate::decode;

    fn objdump(w: u32, pc: u32) -> String {
        decode(w).format(Some(pc), Syntax::OBJDUMP).to_string()
    }

    fn no_aliases(w: u32) -> String {
        let s = Syntax {
            aliases: false,
            hex: true,
        };
        decode(w).format(Some(0), s).to_string()
    }

    #[test]
    fn formats() {
        assert_eq!(decode(0xfff50513).to_string(), "addi a0, a0, -1");
        assert_eq!(decode(0xfeb12e23).to_string(), "sw a1, -4(sp)");
        assert_eq!(decode(0x34051573).to_string(), "csrrw a0, mscratch, a0");
        assert_eq!(decode(0x800002b7).to_string(), "lui t0, 0x80000");
        assert_eq!(decode(0x00c000ef).to_string(), "jal ra, 12");
        assert_eq!(decode(0x0ff0000f).to_string(), "fence iorw, iorw");
        assert_eq!(decode(0x7c002573).to_string(), "csrrs a0, 0x7c0, zero");
        assert_eq!(decode(0).to_string(), ".word 0x00000000");
    }

    #[test]
    fn objdump_style() {
        assert_eq!(objdump(0x00010117, 0x8000_0000), "auipc sp, 0x10");
        assert_eq!(objdump(0x07010113, 0x8000_0004), "addi sp, sp, 0x70");
        assert_eq!(objdump(0x00c000ef, 0x8000_0010), "jal 0x8000001c");
        assert_eq!(objdump(0x00050313, 0), "mv t1, a0");
        assert_eq!(objdump(0x00038e63, 0x8000_0028), "beqz t2, 0x80000044");
        assert_eq!(objdump(0xfe5ff06f, 0x8000_0040), "j 0x80000024");
        assert_eq!(objdump(0x00008067, 0), "ret");
        assert_eq!(objdump(0x80044503, 0), "lbu a0, -0x800(s0)");
        assert_eq!(objdump(0x0005000b, 0), "xcal.dbg a0");
        assert_eq!(objdump(0xc0001073, 0), "unimp");
        assert_eq!(no_aliases(0xc0001073), "unimp");
        assert_eq!(objdump(0x0330000f, 0), "fence rw, rw");
        assert_eq!(objdump(0x0100000f, 0), "fence w, 0");
        assert_eq!(objdump(0x8330000f, 0), "fence.tso");
        assert_eq!(objdump(0xffffffff, 0), ".word 0xffffffff");
    }

    /// What llvm-objdump 21 prints for clang's `-march=rv32ima` encodings.
    #[test]
    fn atomics() {
        let cases = [
            (0x1005a52f, "lr.w a0, (a1)"),
            (0x1405a52f, "lr.w.aq a0, (a1)"),
            (0x1205a52f, "lr.w.rl a0, (a1)"),
            (0x1605a52f, "lr.w.aqrl a0, (a1)"),
            (0x18b6252f, "sc.w a0, a1, (a2)"),
            (0x1e63a2af, "sc.w.aqrl t0, t1, (t2)"),
            (0x08b6252f, "amoswap.w a0, a1, (a2)"),
            (0x04b6252f, "amoadd.w.aq a0, a1, (a2)"),
            (0x62b6252f, "amoand.w.rl a0, a1, (a2)"),
            (0x40b6252f, "amoor.w a0, a1, (a2)"),
            (0x20b6252f, "amoxor.w a0, a1, (a2)"),
            (0x80b6252f, "amomin.w a0, a1, (a2)"),
            (0xa0b6252f, "amomax.w a0, a1, (a2)"),
            (0xc0b6252f, "amominu.w a0, a1, (a2)"),
            (0xe0b6202f, "amomaxu.w zero, a1, (a2)"),
            (0x0805a5af, "amoswap.w a1, zero, (a1)"),
            (0x1805a52f, "sc.w a0, zero, (a1)"),
            (0x1015a52f, ".word 0x1015a52f"),
        ];
        for (w, want) in cases {
            assert_eq!(objdump(w, 0), want);
            assert_eq!(no_aliases(w), want);
            assert_eq!(decode(w).to_string(), want);
        }
        // Every operation and ordering: the suffix is Ordering::suffix.
        for op in crate::AmoOp::ALL {
            for bits in 0..4 {
                let order = crate::Ordering {
                    aq: bits & 2 != 0,
                    rl: bits & 1 != 0,
                };
                let amo = |order| crate::Inst::Amo {
                    op,
                    rd: 1,
                    rs1: 2,
                    rs2: 3,
                    order,
                };
                let base = amo(crate::Ordering::NONE).format(None, Syntax::PLAIN).mnemonic;
                assert_eq!(
                    amo(order).format(None, Syntax::PLAIN).mnemonic,
                    format!("{base}{}", order.suffix())
                );
            }
        }
    }

    #[test]
    fn branch_targets() {
        let d = decode(0xfe0e0ce3).format(Some(0x8000_0034), Syntax::OBJDUMP);
        assert_eq!(d.target, Some(0x8000_002c));
        assert_eq!(d.operands, "t3, 0x8000002c");
        // Without an address the offset is printed instead.
        let d = decode(0xfe0e0ce3).format(None, Syntax::OBJDUMP);
        assert_eq!((d.target, d.operands.as_str()), (None, "t3, -0x8"));
        // Targets wrap around the 32-bit address space.
        let d = decode(0xffdff06f).format(Some(0), Syntax::OBJDUMP);
        assert_eq!(d.operands, "0xfffffffc");
        assert_eq!(decode(0x00000013).format(Some(0), Syntax::OBJDUMP).target, None);
    }

    /// Both registers zero: llvm-objdump picks a particular alias.
    #[test]
    fn alias_priorities() {
        assert_eq!(objdump(0x00000013, 0), "nop");
        assert_eq!(objdump(0x00000513, 0), "li a0, 0x0");
        assert_eq!(objdump(0x00500013, 0), "li zero, 0x5");
        assert_eq!(objdump(0x00050013, 0), "mv zero, a0");
        assert_eq!(objdump(0x00150013, 0), "addi zero, a0, 0x1");
        assert_eq!(objdump(0x00002533, 0), "sltz a0, zero");
        assert_eq!(objdump(0x00003533, 0), "snez a0, zero");
        assert_eq!(objdump(0xfe0044e3, 0x20), "bltz zero, 0x8");
        assert_eq!(objdump(0xfe0052e3, 0x24), "blez zero, 0x8");
        assert_eq!(objdump(0xc0002073, 0), "rdcycle zero");
        assert_eq!(objdump(0x30002073, 0), "csrr zero, mstatus");
        assert_eq!(objdump(0x30001073, 0), "csrw mstatus, zero");
        assert_eq!(objdump(0x00060367, 0), "jalr t1, a2");
        assert_eq!(objdump(0x004500e7, 0), "jalr 0x4(a0)");
        assert_eq!(objdump(0xff858067, 0), "jr -0x8(a1)");
    }

    #[test]
    fn decimal() {
        let s = Syntax {
            aliases: true,
            hex: false,
        };
        assert_eq!(decode(0x80000513).format(None, s).to_string(), "li a0, -2048");
        assert_eq!(decode(0x12345537).format(None, s).to_string(), "lui a0, 0x12345");
        assert_eq!(
            decode(0x3002d573).format(None, s).to_string(),
            "csrrwi a0, mstatus, 5"
        );
    }
}
