//! Disassembly. Output uses ABI register names and matches the syntax
//! merlin-as accepts.
//!
//! [`Inst::format`] is the general form: it can print pseudo-instructions
//! (`mv`, `ret`, `beqz`, `sext.w`, ...) exactly where llvm-objdump does, hex
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
    /// (`nop`, `li`, `mv`, `not`, `neg`, `negw`, `sext.w`, `seqz`, `snez`,
    /// `sltz`, `sgtz`, `zext.b`, `beqz`/`bnez`/`blez`/`bgez`/`bltz`/`bgtz`,
    /// `j`, `jal off`, `jr`, `jalr rs`, `ret`, `fence`, `sfence.vma`,
    /// `csrr`/`csrw`/`csrs`/`csrc` and their `i` forms, `rdcycle`/`rdtime`/
    /// `rdinstret`, and on RV32 the `h` variants; and the hints LLVM 23
    /// names, `pause`, `ntl.p1`/`ntl.pall`/`ntl.s1`/`ntl.all` and `lpad`).
    /// `-M no-aliases` in llvm-objdump turns this off.
    pub aliases: bool,
    /// Immediates in hex (`0x7ff`, `-0x8`), llvm-objdump's default. In
    /// decimal mode, upper immediates (`lui`/`auipc`) and unnamed CSR numbers
    /// are still printed in hex because that is how people write them.
    pub hex: bool,
    /// The width the code is for: branch and jump targets wrap at it, and
    /// the RV32-only CSRs have names only on RV32 (as in llvm-objdump).
    pub xlen: Xlen,
    /// Name Zicbop's prefetch hints (`ori zero, rs1, imm` with `imm[4:0]`
    /// 0, 1 or 3) `prefetch.i`, `prefetch.r` and `prefetch.w`, as LLVM 23
    /// does with aliases or without. The plain form leaves them `ori`: its
    /// output is what merlin-as assembles, and merlin-as, like clang for an
    /// ISA without Zicbop, takes no `prefetch.*`.
    pub zicbop: bool,
}

impl Syntax {
    /// What llvm-objdump prints by default, for code of width `xlen`.
    pub const fn objdump(xlen: Xlen) -> Syntax {
        Syntax {
            aliases: true,
            hex: true,
            xlen,
            zicbop: true,
        }
    }

    /// What `Display` prints (with `xlen` RV32): real instructions only,
    /// decimal immediates.
    pub const fn plain(xlen: Xlen) -> Syntax {
        Syntax {
            aliases: false,
            hex: false,
            xlen,
            zicbop: false,
        }
    }
}

/// A formatted instruction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disasm {
    pub mnemonic: &'static str,
    /// Comma-separated operands, empty when there are none.
    pub operands: String,
    /// For branches and `jal` formatted with a known address: the absolute
    /// target, already printed in `operands`. Tools add a `<symbol>` after it.
    pub target: Option<u64>,
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

/// `rdcycle` & co: `csrrs rd, <counter>, zero`. The `h` forms exist only on
/// RV32, as their CSRs do.
fn counter_alias(csr: u16, xlen: Xlen) -> Option<&'static str> {
    Some(match csr {
        csr::CYCLE => "rdcycle",
        csr::TIME => "rdtime",
        csr::INSTRET => "rdinstret",
        csr::CYCLEH if xlen == Xlen::Rv32 => "rdcycleh",
        csr::TIMEH if xlen == Xlen::Rv32 => "rdtimeh",
        csr::INSTRETH if xlen == Xlen::Rv32 => "rdinstreth",
        _ => return None,
    })
}

/// An A-extension mnemonic with its width and ordering suffixes. Mnemonics
/// are `&'static str`, so each one is spelled out for both widths and four
/// orderings: none, `.rl`, `.aq`, `.aqrl` (indexed by the aq and rl bits).
macro_rules! ordered {
    ($m:literal, $width:expr, $order:expr) => {{
        let order: Ordering = $order;
        let i = order.aq as usize * 2 + order.rl as usize;
        match $width {
            AmoWidth::W => [
                concat!($m, ".w"),
                concat!($m, ".w.rl"),
                concat!($m, ".w.aq"),
                concat!($m, ".w.aqrl"),
            ][i],
            AmoWidth::D => [
                concat!($m, ".d"),
                concat!($m, ".d.rl"),
                concat!($m, ".d.aq"),
                concat!($m, ".d.aqrl"),
            ][i],
        }
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

/// The mnemonic of an OP-IMM operation (`word` false) or an OP-IMM-32 one
/// (`word` true, RV64).
fn alu_imm_name(op: AluOp, word: bool) -> &'static str {
    match (op, word) {
        (AluOp::Add, false) => "addi",
        (AluOp::Sll, false) => "slli",
        (AluOp::Slt, false) => "slti",
        (AluOp::Sltu, false) => "sltiu",
        (AluOp::Xor, false) => "xori",
        (AluOp::Srl, false) => "srli",
        (AluOp::Sra, false) => "srai",
        (AluOp::Or, false) => "ori",
        (AluOp::And, false) => "andi",
        (AluOp::Add, true) => "addiw",
        (AluOp::Sll, true) => "slliw",
        (AluOp::Srl, true) => "srliw",
        (AluOp::Sra, true) => "sraiw",
        // There is no subtract-immediate, and OP-IMM-32 has only addiw and
        // the shifts: the decoder never makes these (see `Inst`).
        (AluOp::Sub, _) | (AluOp::Slt | AluOp::Sltu | AluOp::Xor | AluOp::Or | AluOp::And, true) => {
            unreachable!("not an instruction: {op:?} with an immediate (word {word})")
        }
    }
}

/// The mnemonic of an OP operation (`word` false) or an OP-32 one (`word`
/// true, RV64).
fn alu_reg_name(op: AluOp, word: bool) -> &'static str {
    match (op, word) {
        (AluOp::Add, false) => "add",
        (AluOp::Sub, false) => "sub",
        (AluOp::Sll, false) => "sll",
        (AluOp::Slt, false) => "slt",
        (AluOp::Sltu, false) => "sltu",
        (AluOp::Xor, false) => "xor",
        (AluOp::Srl, false) => "srl",
        (AluOp::Sra, false) => "sra",
        (AluOp::Or, false) => "or",
        (AluOp::And, false) => "and",
        (AluOp::Add, true) => "addw",
        (AluOp::Sub, true) => "subw",
        (AluOp::Sll, true) => "sllw",
        (AluOp::Srl, true) => "srlw",
        (AluOp::Sra, true) => "sraw",
        // OP-32 has only these five: the decoder never makes the others.
        (AluOp::Slt | AluOp::Sltu | AluOp::Xor | AluOp::Or | AluOp::And, true) => {
            unreachable!("not an instruction: OP-32 {op:?}")
        }
    }
}

/// The mnemonic of an M operation (`word` false) or a 32-bit one (`word`
/// true, RV64).
fn mul_name(op: MulOp, word: bool) -> &'static str {
    match (op, word) {
        (MulOp::Mul, false) => "mul",
        (MulOp::Mulh, false) => "mulh",
        (MulOp::Mulhsu, false) => "mulhsu",
        (MulOp::Mulhu, false) => "mulhu",
        (MulOp::Div, false) => "div",
        (MulOp::Divu, false) => "divu",
        (MulOp::Rem, false) => "rem",
        (MulOp::Remu, false) => "remu",
        (MulOp::Mul, true) => "mulw",
        (MulOp::Div, true) => "divw",
        (MulOp::Divu, true) => "divuw",
        (MulOp::Rem, true) => "remw",
        (MulOp::Remu, true) => "remuw",
        // There are no 32-bit high multiplies: the decoder never makes these.
        (MulOp::Mulh | MulOp::Mulhsu | MulOp::Mulhu, true) => {
            unreachable!("not an instruction: 32-bit {op:?}")
        }
    }
}

impl Inst {
    /// Formats the instruction. `pc` is its address: when given, branch and
    /// `jal` targets print as absolute addresses (`0x80000044`), wrapping at
    /// the width, otherwise as offsets.
    pub fn format(&self, pc: Option<u64>, syntax: Syntax) -> Disasm {
        let a = syntax.aliases;
        let xlen = syntax.xlen;
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
                let t = pc.wrapping_add(off as i64 as u64);
                let t = if xlen == Xlen::Rv32 { t & 0xffff_ffff } else { t };
                target = Some(t);
                format!("{t:#x}")
            }
            None => imm(off),
        };
        let csr_name = |c: u16| match csr::name(c, xlen) {
            Some(n) => n.to_string(),
            None => format!("{c:#x}"),
        };

        let (mnemonic, operands): (&'static str, String) = match *self {
            Inst::Lui { rd, imm } => ("lui", format!("{}, {:#x}", r(rd), (imm as u32) >> 12)),
            // `auipc zero, imm` is a hint: Zicfilp's landing pad, as LLVM 23 names it.
            Inst::Auipc { rd: 0, imm } if a => ("lpad", format!("{:#x}", (imm as u32) >> 12)),
            Inst::Auipc { rd, imm } => ("auipc", format!("{}, {:#x}", r(rd), (imm as u32) >> 12)),
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
                    LoadOp::Lwu => "lwu",
                    LoadOp::Ld => "ld",
                };
                (m, format!("{}, {}({})", r(rd), imm(i), r(rs1)))
            }
            Inst::Store { op, rs1, rs2, imm: i } => {
                let m = match op {
                    StoreOp::Sb => "sb",
                    StoreOp::Sh => "sh",
                    StoreOp::Sw => "sw",
                    StoreOp::Sd => "sd",
                };
                (m, format!("{}, {}({})", r(rs2), imm(i), r(rs1)))
            }
            // `ori zero, rs1, imm` with imm[4:0] 0, 1 or 3 is a hint: Zicbop's
            // prefetches, which LLVM 23 decodes as instructions of their own
            // (named without aliases too), unless `Syntax::zicbop` is off.
            // The offset is the rest of imm.
            Inst::OpImm {
                op: AluOp::Or,
                rd: 0,
                rs1,
                imm: i,
            } if syntax.zicbop && matches!(i & 0x1f, 0 | 1 | 3) => {
                let m = match i & 0x1f {
                    0 => "prefetch.i",
                    1 => "prefetch.r",
                    _ => "prefetch.w",
                };
                (m, format!("{}({})", imm(i & !0x1f), r(rs1)))
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
                    _ => (
                        alu_imm_name(op, false),
                        format!("{}, {}, {}", r(rd), r(rs1), imm(i)),
                    ),
                }
            }
            // `addiw rd, rs, 0` sign-extends the low word: llvm-objdump's `sext.w`.
            Inst::OpImm32 {
                op: AluOp::Add,
                rd,
                rs1,
                imm: 0,
            } if a => ("sext.w", format!("{}, {}", r(rd), r(rs1))),
            Inst::OpImm32 { op, rd, rs1, imm: i } => (
                alu_imm_name(op, true),
                format!("{}, {}, {}", r(rd), r(rs1), imm(i)),
            ),
            // `add zero, zero, rs2` for rs2 = x2..x5 is a hint: Zihintntl's
            // non-temporal locality hints, aliases in LLVM 23.
            Inst::Op {
                op: AluOp::Add,
                rd: 0,
                rs1: 0,
                rs2: rs2 @ 2..=5,
            } if a => {
                let m = ["ntl.p1", "ntl.pall", "ntl.s1", "ntl.all"][rs2 as usize - 2];
                (m, String::new())
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
                    _ => (
                        alu_reg_name(op, false),
                        format!("{}, {}, {}", r(rd), r(rs1), r(rs2)),
                    ),
                }
            }
            // `subw rd, zero, rs`: llvm-objdump's `negw`.
            Inst::Op32 {
                op: AluOp::Sub,
                rd,
                rs1: 0,
                rs2,
            } if a => ("negw", format!("{}, {}", r(rd), r(rs2))),
            Inst::Op32 { op, rd, rs1, rs2 } => (
                alu_reg_name(op, true),
                format!("{}, {}, {}", r(rd), r(rs1), r(rs2)),
            ),
            Inst::Mul { op, rd, rs1, rs2 } => {
                (mul_name(op, false), format!("{}, {}, {}", r(rd), r(rs1), r(rs2)))
            }
            Inst::Mul32 { op, rd, rs1, rs2 } => {
                (mul_name(op, true), format!("{}, {}, {}", r(rd), r(rs1), r(rs2)))
            }
            Inst::Lr {
                width,
                rd,
                rs1,
                order,
            } => (ordered!("lr", width, order), format!("{}, ({})", r(rd), r(rs1))),
            Inst::Sc {
                width,
                rd,
                rs1,
                rs2,
                order,
            } => (
                ordered!("sc", width, order),
                format!("{}, {}, ({})", r(rd), r(rs2), r(rs1)),
            ),
            Inst::Amo {
                op,
                width,
                rd,
                rs1,
                rs2,
                order,
            } => {
                let m = match op {
                    AmoOp::Swap => ordered!("amoswap", width, order),
                    AmoOp::Add => ordered!("amoadd", width, order),
                    AmoOp::Xor => ordered!("amoxor", width, order),
                    AmoOp::And => ordered!("amoand", width, order),
                    AmoOp::Or => ordered!("amoor", width, order),
                    AmoOp::Min => ordered!("amomin", width, order),
                    AmoOp::Max => ordered!("amomax", width, order),
                    AmoOp::Minu => ordered!("amominu", width, order),
                    AmoOp::Maxu => ordered!("amomaxu", width, order),
                };
                (m, format!("{}, {}, ({})", r(rd), r(rs2), r(rs1)))
            }
            Inst::Fence { pred: 0xf, succ: 0xf } if a => ("fence", String::new()),
            // `fence w, 0` is a hint: Zihintpause's `pause`, an alias in LLVM 23.
            Inst::Fence { pred: 0x1, succ: 0 } if a => ("pause", String::new()),
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
                    (CsrOp::Rs, _, CsrSrc::Reg(0)) => Some(match counter_alias(csr, xlen) {
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

/// The plain form, with RV32's CSR names. `Inst` does not know the width it
/// was decoded for, so this is right for RV32 only: on RV64 the CSRs that
/// exist only on RV32 (`cycleh`, `mstatush`...) are numbers, and an RV64
/// caller formats with [`Inst::format`] and `Syntax::plain(Xlen::Rv64)`
/// instead. Every other name, and every mnemonic and operand, is the same
/// for both widths.
impl fmt::Display for Inst {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.format(None, Syntax::plain(Xlen::Rv32)).fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::Syntax;
    use crate::{decode, Xlen};

    const RV32: Xlen = Xlen::Rv32;
    const RV64: Xlen = Xlen::Rv64;

    fn objdump(w: u32, pc: u64) -> String {
        decode(w, RV32)
            .format(Some(pc), Syntax::objdump(RV32))
            .to_string()
    }

    fn objdump64(w: u32, pc: u64) -> String {
        decode(w, RV64)
            .format(Some(pc), Syntax::objdump(RV64))
            .to_string()
    }

    fn no_aliases(w: u32) -> String {
        let s = Syntax {
            aliases: false,
            hex: true,
            xlen: RV32,
            zicbop: true,
        };
        decode(w, RV32).format(Some(0), s).to_string()
    }

    #[test]
    fn formats() {
        let plain = |w| decode(w, RV32).to_string();
        assert_eq!(plain(0xfff50513), "addi a0, a0, -1");
        assert_eq!(plain(0xfeb12e23), "sw a1, -4(sp)");
        assert_eq!(plain(0x34051573), "csrrw a0, mscratch, a0");
        assert_eq!(plain(0x800002b7), "lui t0, 0x80000");
        assert_eq!(plain(0x00c000ef), "jal ra, 12");
        assert_eq!(plain(0x0ff0000f), "fence iorw, iorw");
        assert_eq!(plain(0x7c002573), "csrrs a0, 0x7c0, zero");
        assert_eq!(plain(0), ".word 0x00000000");
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
        assert_eq!(objdump(0x0100000f, 0), "pause");
        assert_eq!(no_aliases(0x0100000f), "fence w, 0");
        assert_eq!(objdump(0x8330000f, 0), "fence.tso");
        assert_eq!(objdump(0xffffffff, 0), ".word 0xffffffff");
    }

    /// What llvm-objdump 23 prints for clang's `-march=rv32ima` encodings.
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
            assert_eq!(decode(w, RV32).to_string(), want);
        }
        // Every operation, width and ordering: the suffixes are the width's
        // and Ordering::suffix.
        for op in crate::AmoOp::ALL {
            for width in [crate::AmoWidth::W, crate::AmoWidth::D] {
                for bits in 0..4 {
                    let order = crate::Ordering {
                        aq: bits & 2 != 0,
                        rl: bits & 1 != 0,
                    };
                    let amo = |order| crate::Inst::Amo {
                        op,
                        width,
                        rd: 1,
                        rs1: 2,
                        rs2: 3,
                        order,
                    };
                    let base = amo(crate::Ordering::NONE)
                        .format(None, Syntax::plain(RV64))
                        .mnemonic;
                    let w = if width == crate::AmoWidth::W { ".w" } else { ".d" };
                    assert!(base.ends_with(w), "{base}");
                    assert_eq!(
                        amo(order).format(None, Syntax::plain(RV64)).mnemonic,
                        format!("{base}{}", order.suffix())
                    );
                }
            }
        }
    }

    #[test]
    fn branch_targets() {
        let d = decode(0xfe0e0ce3, RV32).format(Some(0x8000_0034), Syntax::objdump(RV32));
        assert_eq!(d.target, Some(0x8000_002c));
        assert_eq!(d.operands, "t3, 0x8000002c");
        // Without an address the offset is printed instead.
        let d = decode(0xfe0e0ce3, RV32).format(None, Syntax::objdump(RV32));
        assert_eq!((d.target, d.operands.as_str()), (None, "t3, -0x8"));
        // Targets wrap at the width: around 2^32 on RV32, 2^64 on RV64.
        let d = decode(0xffdff06f, RV32).format(Some(0), Syntax::objdump(RV32));
        assert_eq!(d.operands, "0xfffffffc");
        let d = decode(0xffdff06f, RV64).format(Some(0), Syntax::objdump(RV64));
        assert_eq!(d.operands, "0xfffffffffffffffc");
        let d = decode(0x00c000ef, RV64).format(Some(0x1_0000_0000), Syntax::objdump(RV64));
        assert_eq!(
            (d.target, d.operands.as_str()),
            (Some(0x1_0000_000c), "0x10000000c")
        );
        assert_eq!(
            decode(0x00000013, RV32)
                .format(Some(0), Syntax::objdump(RV32))
                .target,
            None
        );
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

    /// The hints LLVM 23 names: Zihintpause, Zihintntl and Zicfilp's as
    /// aliases, Zicbop's as instructions (so without aliases too).
    #[test]
    fn hints() {
        let cases = [
            (0x0100000f, "pause", "fence w, 0"),
            (0x00200033, "ntl.p1", "add zero, zero, sp"),
            (0x00300033, "ntl.pall", "add zero, zero, gp"),
            (0x00400033, "ntl.s1", "add zero, zero, tp"),
            (0x00500033, "ntl.all", "add zero, zero, t0"),
            (0x00100033, "add zero, zero, ra", "add zero, zero, ra"),
            (0x00600033, "add zero, zero, t1", "add zero, zero, t1"),
            (0x00200133, "add sp, zero, sp", "add sp, zero, sp"),
            (0x00001017, "lpad 0x1", "auipc zero, 0x1"),
            (0xfffff017, "lpad 0xfffff", "auipc zero, 0xfffff"),
            (0x02056013, "prefetch.i 0x20(a0)", "prefetch.i 0x20(a0)"),
            (0x02156013, "prefetch.r 0x20(a0)", "prefetch.r 0x20(a0)"),
            (0x02356013, "prefetch.w 0x20(a0)", "prefetch.w 0x20(a0)"),
            (0x00006013, "prefetch.i 0x0(zero)", "prefetch.i 0x0(zero)"),
            (0x8005e013, "prefetch.i -0x800(a1)", "prefetch.i -0x800(a1)"),
            (0x02256013, "ori zero, a0, 0x22", "ori zero, a0, 0x22"),
            (0x02056093, "ori ra, a0, 0x20", "ori ra, a0, 0x20"),
        ];
        for (w, aliases, plain) in cases {
            assert_eq!(objdump(w, 0), aliases, "{w:#010x}");
            assert_eq!(no_aliases(w), plain, "{w:#010x}");
            assert_eq!(objdump64(w, 0), aliases, "{w:#010x}");
        }
        // The plain form (Grail's trace) is what merlin-as assembles: no
        // prefetch.i, which needs Zicbop.
        assert_eq!(decode(0xfe056013, RV32).to_string(), "ori zero, a0, -32");
        assert_eq!(decode(0x0100000f, RV32).to_string(), "fence w, 0");
    }

    #[test]
    fn rv32_only_csrs_by_width() {
        // csrrs a0, cycleh, zero: rdcycleh on RV32; on RV64 the CSR has no
        // name, so llvm-objdump prints its number.
        assert_eq!(objdump(0xc8002573, 0), "rdcycleh a0");
        assert_eq!(objdump64(0xc8002573, 0), "csrr a0, 0xc80");
        assert_eq!(objdump64(0xc0002573, 0), "rdcycle a0");
    }

    #[test]
    fn decimal() {
        let s = Syntax {
            aliases: true,
            hex: false,
            xlen: RV32,
            zicbop: true,
        };
        let f = |w| decode(w, RV32).format(None, s).to_string();
        assert_eq!(f(0x80000513), "li a0, -2048");
        assert_eq!(f(0x12345537), "lui a0, 0x12345");
        assert_eq!(f(0x3002d573), "csrrwi a0, mstatus, 5");
    }
}
