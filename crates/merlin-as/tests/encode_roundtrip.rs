//! Checks the encoder against the shared decoder in `excalibur`, which Grail
//! already runs riscv-tests with. Two independent checks, both independent of
//! llvm-mc:
//!
//! * `expected`: a hand-written table of source lines and the [`Inst`] values
//!   `excalibur::decode` must give back for each word, plus the relocation
//!   each word carries (`expected_relocs`; every other word must carry none).
//!   This covers what the golden file can't (Xcal, CSRs by name, `li` splits,
//!   explicit `%pcrel_hi`/`%pcrel_lo` pairs on user labels) and spells out the
//!   operation and operands of every mnemonic in the golden file.
//! * `disasm`: every word llvm-mc produced for the golden file (plus a few
//!   more from clang), decoded and printed with the disassembler, must
//!   assemble back to exactly that word. Proves the disassembler's syntax
//!   reassembles, and catches bits `decode` ignores, like fence's pred/succ.
//!   Words patched by a relocation are left out: their fields are zero.
//!
//! Same progress rules as `encode_golden`: mnemonics that still return
//! "not implemented yet" are counted, not failed. The `_complete` variants
//! (ignored by default) require everything:
//!
//!     cargo test -p merlin-as --test encode_roundtrip -- --nocapture
//!     cargo test -p merlin-as --test encode_roundtrip -- --ignored

use excalibur::{decode, AluOp, BranchOp, CsrOp, CsrSrc, Inst, LoadOp, MulOp, StoreOp};
use merlin_as::encode::UNIMPLEMENTED;
use merlin_as::reloc::RelocKind;

/// One word of `.text` and the relocation that will patch it, if any.
struct Word {
    word: u32,
    reloc: Option<Reloc>,
}

#[derive(Debug, PartialEq)]
struct Reloc {
    kind: RelocKind,
    to: To,
}

/// What a relocation points at.
#[derive(Debug, PartialEq)]
enum To {
    /// A named symbol plus addend. Numeric labels appear by their internal
    /// name: `1:` is `.L1$0`, the next `1:` is `.L1$1`.
    Sym(String, i64),
    /// A hidden label the assembler put on the auipc at this `.text` offset
    /// (`Target::Auipc`, for `la`, `lw a0, sym`, ...).
    Auipc(u32),
}

enum Assembled {
    Words(Vec<Word>),
    Unimplemented,
    Error(String),
}

fn assemble(src: &str) -> Assembled {
    let obj = match merlin_as::assemble(src) {
        Ok(o) => o,
        Err(errs) if errs.iter().all(|e| e.msg.starts_with(UNIMPLEMENTED)) => {
            return Assembled::Unimplemented
        }
        Err(errs) => return Assembled::Error(errs[0].to_string()),
    };
    let text = &obj.sections[0];
    let reloc_at = |offset: u32| {
        let r = text.relocs.iter().find(|r| r.offset == offset)?;
        let to = if r.symbol.starts_with(".Lpcrel_hi") {
            let sym = obj.symbols.iter().find(|s| s.name == r.symbol);
            To::Auipc(sym.map_or(u32::MAX, |s| s.value))
        } else {
            To::Sym(r.symbol.clone(), r.addend)
        };
        Some(Reloc { kind: r.kind, to })
    };
    let words = text
        .data
        .chunks(4)
        .enumerate()
        .map(|(i, c)| Word {
            word: u32::from_le_bytes([c[0], c[1], c[2], c[3]]),
            reloc: reloc_at(4 * i as u32),
        })
        .collect();
    Assembled::Words(words)
}

/// Collects outcomes and prints the same kind of report as `encode_golden`.
struct Report {
    name: &'static str,
    total: usize,
    pass: usize,
    todo: Vec<String>,
    fails: Vec<String>,
}

impl Report {
    fn new(name: &'static str) -> Self {
        Report {
            name,
            total: 0,
            pass: 0,
            todo: Vec::new(),
            fails: Vec::new(),
        }
    }

    fn finish(self, require_all: bool) {
        println!(
            "{}: {}/{} pass, {} not implemented yet",
            self.name,
            self.pass,
            self.total,
            self.todo.len()
        );
        if !self.todo.is_empty() {
            let mut m: Vec<&str> = self
                .todo
                .iter()
                .map(|s| s.split_whitespace().next().unwrap_or(""))
                .collect();
            m.sort();
            m.dedup();
            println!("still to do: {}", m.join(" "));
        }
        assert!(
            self.fails.is_empty(),
            "{}: {} failures:\n{}",
            self.name,
            self.fails.len(),
            self.fails.join("\n")
        );
        if require_all {
            assert!(
                self.todo.is_empty(),
                "{} lines not implemented yet",
                self.todo.len()
            );
        }
    }
}

// ----- hand-written expectations -----

fn r(op: AluOp, rd: u8, rs1: u8, rs2: u8) -> Inst {
    Inst::Op { op, rd, rs1, rs2 }
}
fn i(op: AluOp, rd: u8, rs1: u8, imm: i32) -> Inst {
    Inst::OpImm { op, rd, rs1, imm }
}
fn m(op: MulOp, rd: u8, rs1: u8, rs2: u8) -> Inst {
    Inst::Mul { op, rd, rs1, rs2 }
}
fn csr(op: CsrOp, rd: u8, src: CsrSrc, csr: u16) -> Inst {
    Inst::Csr { op, rd, src, csr }
}

// ABI register numbers used below.
const ZERO: u8 = 0;
const RA: u8 = 1;
const SP: u8 = 2;
const T0: u8 = 5;
const T1: u8 = 6;
const T2: u8 = 7;
const A0: u8 = 10;
const A1: u8 = 11;
const A2: u8 = 12;
const A3: u8 = 13;
const A5: u8 = 15;
const S2: u8 = 18;
const S3: u8 = 19;
const T5: u8 = 30;
const T6: u8 = 31;

/// Source and the instructions `decode` must return, one per emitted word.
/// Words carrying a relocation have zero immediates, so their expected
/// immediates are 0.
fn expected() -> Vec<(&'static str, Vec<Inst>)> {
    use Inst::*;
    vec![
        // U and J
        (
            "lui a0, 0x12345",
            vec![Lui {
                rd: A0,
                imm: 0x1234_5000,
            }],
        ),
        (
            "lui t0, 0xfffff",
            vec![Lui {
                rd: T0,
                imm: 0xffff_f000,
            }],
        ),
        ("auipc sp, 1", vec![Auipc { rd: SP, imm: 0x1000 }]),
        ("jal ra, -4", vec![Jal { rd: RA, imm: -4 }]),
        (
            "jal zero, 1048574",
            vec![Jal {
                rd: ZERO,
                imm: 1_048_574,
            }],
        ),
        (
            "jal zero, -1048576",
            vec![Jal {
                rd: ZERO,
                imm: -1_048_576,
            }],
        ),
        // I: jalr, loads, ALU immediates, shifts
        (
            "jalr zero, 8(a5)",
            vec![Jalr {
                rd: ZERO,
                rs1: A5,
                imm: 8,
            }],
        ),
        (
            "jalr ra, -2048(t0)",
            vec![Jalr {
                rd: RA,
                rs1: T0,
                imm: -2048,
            }],
        ),
        (
            "lw ra, 2047(sp)",
            vec![Load {
                op: LoadOp::Lw,
                rd: RA,
                rs1: SP,
                imm: 2047,
            }],
        ),
        (
            "lhu s2, 12(s3)",
            vec![Load {
                op: LoadOp::Lhu,
                rd: S2,
                rs1: S3,
                imm: 12,
            }],
        ),
        (
            "lb a0, (a1)",
            vec![Load {
                op: LoadOp::Lb,
                rd: A0,
                rs1: A1,
                imm: 0,
            }],
        ),
        (
            "lh a0, -1(sp)",
            vec![Load {
                op: LoadOp::Lh,
                rd: A0,
                rs1: SP,
                imm: -1,
            }],
        ),
        (
            "lbu t0, -2048(t1)",
            vec![Load {
                op: LoadOp::Lbu,
                rd: T0,
                rs1: T1,
                imm: -2048,
            }],
        ),
        ("addi sp, sp, -2048", vec![i(AluOp::Add, SP, SP, -2048)]),
        ("slti a0, a1, -5", vec![i(AluOp::Slt, A0, A1, -5)]),
        ("sltiu t0, t1, -1", vec![i(AluOp::Sltu, T0, T1, -1)]),
        ("xori a0, a1, 0x555", vec![i(AluOp::Xor, A0, A1, 0x555)]),
        ("ori a0, a1, -2048", vec![i(AluOp::Or, A0, A1, -2048)]),
        ("andi a0, a1, 0x7ff", vec![i(AluOp::And, A0, A1, 0x7ff)]),
        ("slli a0, a1, 31", vec![i(AluOp::Sll, A0, A1, 31)]),
        ("srli a0, a1, 0", vec![i(AluOp::Srl, A0, A1, 0)]),
        ("srai a0, a1, 17", vec![i(AluOp::Sra, A0, A1, 17)]),
        // S and B (scrambled immediates: the classic place for bugs)
        (
            "sw ra, 2044(sp)",
            vec![Store {
                op: StoreOp::Sw,
                rs1: SP,
                rs2: RA,
                imm: 2044,
            }],
        ),
        (
            "sh t5, -2(t6)",
            vec![Store {
                op: StoreOp::Sh,
                rs1: T6,
                rs2: T5,
                imm: -2,
            }],
        ),
        (
            "sb a0, -2048(a1)",
            vec![Store {
                op: StoreOp::Sb,
                rs1: A1,
                rs2: A0,
                imm: -2048,
            }],
        ),
        (
            "beq a0, a1, 8",
            vec![Branch {
                op: BranchOp::Beq,
                rs1: A0,
                rs2: A1,
                imm: 8,
            }],
        ),
        (
            "bne t0, t1, -8",
            vec![Branch {
                op: BranchOp::Bne,
                rs1: T0,
                rs2: T1,
                imm: -8,
            }],
        ),
        (
            "blt a0, a1, 16",
            vec![Branch {
                op: BranchOp::Blt,
                rs1: A0,
                rs2: A1,
                imm: 16,
            }],
        ),
        (
            "bge a2, a3, -4096",
            vec![Branch {
                op: BranchOp::Bge,
                rs1: A2,
                rs2: A3,
                imm: -4096,
            }],
        ),
        (
            "bltu t5, t6, 4094",
            vec![Branch {
                op: BranchOp::Bltu,
                rs1: T5,
                rs2: T6,
                imm: 4094,
            }],
        ),
        (
            "bgeu a0, a1, 2048",
            vec![Branch {
                op: BranchOp::Bgeu,
                rs1: A0,
                rs2: A1,
                imm: 2048,
            }],
        ),
        // R and M
        ("add a0, a1, a2", vec![r(AluOp::Add, A0, A1, A2)]),
        ("sub a0, a1, a2", vec![r(AluOp::Sub, A0, A1, A2)]),
        ("sll t0, t1, t2", vec![r(AluOp::Sll, T0, T1, T2)]),
        ("slt a0, a1, a2", vec![r(AluOp::Slt, A0, A1, A2)]),
        ("sltu t0, t1, t2", vec![r(AluOp::Sltu, T0, T1, T2)]),
        ("xor a0, a1, a2", vec![r(AluOp::Xor, A0, A1, A2)]),
        ("srl a1, a2, a3", vec![r(AluOp::Srl, A1, A2, A3)]),
        ("sra a1, a2, a3", vec![r(AluOp::Sra, A1, A2, A3)]),
        ("or t5, t6, a0", vec![r(AluOp::Or, T5, T6, A0)]),
        ("and a0, a1, a2", vec![r(AluOp::And, A0, A1, A2)]),
        ("mul a0, a1, a2", vec![m(MulOp::Mul, A0, A1, A2)]),
        ("mulh a0, a1, a2", vec![m(MulOp::Mulh, A0, A1, A2)]),
        ("mulhsu a0, a1, a2", vec![m(MulOp::Mulhsu, A0, A1, A2)]),
        ("mulhu t0, t1, t2", vec![m(MulOp::Mulhu, T0, T1, T2)]),
        ("div a0, a1, a2", vec![m(MulOp::Div, A0, A1, A2)]),
        ("divu a0, a1, a2", vec![m(MulOp::Divu, A0, A1, A2)]),
        ("rem s2, s3, a0", vec![m(MulOp::Rem, S2, S3, A0)]),
        ("remu t0, t1, t2", vec![m(MulOp::Remu, T0, T1, T2)]),
        // System
        ("fence", vec![Fence]),
        ("fence.i", vec![FenceI]),
        ("ecall", vec![Ecall]),
        ("ebreak", vec![Ebreak]),
        ("mret", vec![Mret]),
        ("sret", vec![Sret]),
        ("wfi", vec![Wfi]),
        ("sfence.vma", vec![SfenceVma { rs1: ZERO, rs2: ZERO }]),
        ("sfence.vma a0, a1", vec![SfenceVma { rs1: A0, rs2: A1 }]),
        // CSRs, by name and by number
        (
            "csrrw a0, mstatus, a1",
            vec![csr(CsrOp::Rw, A0, CsrSrc::Reg(A1), 0x300)],
        ),
        (
            "csrrs t0, mepc, zero",
            vec![csr(CsrOp::Rs, T0, CsrSrc::Reg(ZERO), 0x341)],
        ),
        (
            "csrrc a0, 0x7c0, a1",
            vec![csr(CsrOp::Rc, A0, CsrSrc::Reg(A1), 0x7c0)],
        ),
        (
            "csrrwi zero, satp, 5",
            vec![csr(CsrOp::Rw, ZERO, CsrSrc::Imm(5), 0x180)],
        ),
        (
            "csrrsi a0, sie, 31",
            vec![csr(CsrOp::Rs, A0, CsrSrc::Imm(31), 0x104)],
        ),
        (
            "csrrci a0, 4095, 0",
            vec![csr(CsrOp::Rc, A0, CsrSrc::Imm(0), 0xfff)],
        ),
        (
            "csrr a0, mcause",
            vec![csr(CsrOp::Rs, A0, CsrSrc::Reg(ZERO), 0x342)],
        ),
        (
            "csrw mscratch, sp",
            vec![csr(CsrOp::Rw, ZERO, CsrSrc::Reg(SP), 0x340)],
        ),
        ("csrs mie, t0", vec![csr(CsrOp::Rs, ZERO, CsrSrc::Reg(T0), 0x304)]),
        (
            "csrc mstatus, t1",
            vec![csr(CsrOp::Rc, ZERO, CsrSrc::Reg(T1), 0x300)],
        ),
        (
            "csrwi mstatus, 8",
            vec![csr(CsrOp::Rw, ZERO, CsrSrc::Imm(8), 0x300)],
        ),
        (
            "csrsi sstatus, 2",
            vec![csr(CsrOp::Rs, ZERO, CsrSrc::Imm(2), 0x100)],
        ),
        (
            "csrci sstatus, 2",
            vec![csr(CsrOp::Rc, ZERO, CsrSrc::Imm(2), 0x100)],
        ),
        // Xcal (spec section 9), not in the llvm-mc golden file
        ("xcal.dbg a0", vec![XcalDbg { rs1: A0 }]),
        ("xcal.dbg zero", vec![XcalDbg { rs1: ZERO }]),
        ("xcal.halt zero", vec![XcalHalt { rs1: ZERO }]),
        ("xcal.halt t6", vec![XcalHalt { rs1: T6 }]),
        // Simple pseudo-instructions
        ("nop", vec![i(AluOp::Add, ZERO, ZERO, 0)]),
        ("mv a0, a1", vec![i(AluOp::Add, A0, A1, 0)]),
        ("not a0, a1", vec![i(AluOp::Xor, A0, A1, -1)]),
        ("neg a0, a1", vec![r(AluOp::Sub, A0, ZERO, A1)]),
        ("seqz a0, a1", vec![i(AluOp::Sltu, A0, A1, 1)]),
        ("snez a0, a1", vec![r(AluOp::Sltu, A0, ZERO, A1)]),
        ("sltz a0, a1", vec![r(AluOp::Slt, A0, A1, ZERO)]),
        ("sgtz a0, a1", vec![r(AluOp::Slt, A0, ZERO, A1)]),
        ("j -8", vec![Jal { rd: ZERO, imm: -8 }]),
        (
            "jr t0",
            vec![Jalr {
                rd: ZERO,
                rs1: T0,
                imm: 0,
            }],
        ),
        (
            "ret",
            vec![Jalr {
                rd: ZERO,
                rs1: RA,
                imm: 0,
            }],
        ),
        (
            "beqz a0, 8",
            vec![Branch {
                op: BranchOp::Beq,
                rs1: A0,
                rs2: ZERO,
                imm: 8,
            }],
        ),
        (
            "bnez a0, -8",
            vec![Branch {
                op: BranchOp::Bne,
                rs1: A0,
                rs2: ZERO,
                imm: -8,
            }],
        ),
        (
            "bltz a0, 4",
            vec![Branch {
                op: BranchOp::Blt,
                rs1: A0,
                rs2: ZERO,
                imm: 4,
            }],
        ),
        (
            "bgez a0, 4",
            vec![Branch {
                op: BranchOp::Bge,
                rs1: A0,
                rs2: ZERO,
                imm: 4,
            }],
        ),
        (
            "blez a0, 4",
            vec![Branch {
                op: BranchOp::Bge,
                rs1: ZERO,
                rs2: A0,
                imm: 4,
            }],
        ),
        (
            "bgtz a0, 4",
            vec![Branch {
                op: BranchOp::Blt,
                rs1: ZERO,
                rs2: A0,
                imm: 4,
            }],
        ),
        // Swapped-operand branches: bgt a, b == blt b, a
        (
            "bgt a0, a1, 8",
            vec![Branch {
                op: BranchOp::Blt,
                rs1: A1,
                rs2: A0,
                imm: 8,
            }],
        ),
        (
            "ble a0, a1, 8",
            vec![Branch {
                op: BranchOp::Bge,
                rs1: A1,
                rs2: A0,
                imm: 8,
            }],
        ),
        (
            "bgtu a0, a1, 8",
            vec![Branch {
                op: BranchOp::Bltu,
                rs1: A1,
                rs2: A0,
                imm: 8,
            }],
        ),
        (
            "bleu a0, a1, 8",
            vec![Branch {
                op: BranchOp::Bgeu,
                rs1: A1,
                rs2: A0,
                imm: 8,
            }],
        ),
        // li, in LLVM's order of preference (encode.golden follows LLVM):
        //   fits in 12 signed bits          -> addi
        //   low 12 bits are zero            -> lui
        //   c << s with c in -32..=31       -> addi c + slli s (both compress to 16 bits with C)
        //   anything else                   -> lui + addi
        ("li a0, -2048", vec![i(AluOp::Add, A0, ZERO, -2048)]),
        (
            "li a0, 2048",
            vec![i(AluOp::Add, A0, ZERO, 1), i(AluOp::Sll, A0, A0, 11)],
        ),
        (
            "li a0, 0x1800",
            vec![i(AluOp::Add, A0, ZERO, 3), i(AluOp::Sll, A0, A0, 11)],
        ),
        (
            "li a0, -6144",
            vec![i(AluOp::Add, A0, ZERO, -3), i(AluOp::Sll, A0, A0, 11)],
        ),
        // 0x7ff << 11: c doesn't fit in 6 bits, so lui + addi.
        (
            "li a0, 0x3ff800",
            vec![
                Lui {
                    rd: A0,
                    imm: 0x0040_0000,
                },
                i(AluOp::Add, A0, A0, -2048),
            ],
        ),
        (
            "li a0, 0x80000000",
            vec![Lui {
                rd: A0,
                imm: 0x8000_0000,
            }],
        ),
        (
            "li a0, 0x12345678",
            vec![
                Lui {
                    rd: A0,
                    imm: 0x1234_5000,
                },
                i(AluOp::Add, A0, A0, 0x678),
            ],
        ),
        // Low 12 bits 0xfff sign-extend to -1, so the upper part rounds up: the +0x800 fix.
        (
            "li a0, 0x12345fff",
            vec![
                Lui {
                    rd: A0,
                    imm: 0x1234_6000,
                },
                i(AluOp::Add, A0, A0, -1),
            ],
        ),
        (
            "li a0, -2049",
            vec![
                Lui {
                    rd: A0,
                    imm: 0xffff_f000,
                },
                i(AluOp::Add, A0, A0, 2047),
            ],
        ),
        (
            "li a0, 0x7ffff800",
            vec![
                Lui {
                    rd: A0,
                    imm: 0x8000_0000,
                },
                i(AluOp::Add, A0, A0, -2048),
            ],
        ),
        // .equ constants are plain numbers to the encoder.
        (
            ".equ UART_BASE, 0x10000000\nli t0, UART_BASE",
            vec![Lui {
                rd: T0,
                imm: 0x1000_0000,
            }],
        ),
        // Label operands: relocated fields are zero, the rest must be right.
        (
            "call target",
            vec![
                Auipc { rd: RA, imm: 0 },
                Jalr {
                    rd: RA,
                    rs1: RA,
                    imm: 0,
                },
            ],
        ),
        (
            "tail target",
            vec![
                Auipc { rd: T1, imm: 0 },
                Jalr {
                    rd: ZERO,
                    rs1: T1,
                    imm: 0,
                },
            ],
        ),
        (
            "la a0, target",
            vec![Auipc { rd: A0, imm: 0 }, i(AluOp::Add, A0, A0, 0)],
        ),
        (
            "lla a0, target",
            vec![Auipc { rd: A0, imm: 0 }, i(AluOp::Add, A0, A0, 0)],
        ),
        (
            "lw a0, target",
            vec![
                Auipc { rd: A0, imm: 0 },
                Load {
                    op: LoadOp::Lw,
                    rd: A0,
                    rs1: A0,
                    imm: 0,
                },
            ],
        ),
        (
            "sw a0, target, t0",
            vec![
                Auipc { rd: T0, imm: 0 },
                Store {
                    op: StoreOp::Sw,
                    rs1: T0,
                    rs2: A0,
                    imm: 0,
                },
            ],
        ),
        (
            "beq a0, a1, target",
            vec![Branch {
                op: BranchOp::Beq,
                rs1: A0,
                rs2: A1,
                imm: 0,
            }],
        ),
        ("jal target", vec![Jal { rd: RA, imm: 0 }]),
        ("lui a0, %hi(target)", vec![Lui { rd: A0, imm: 0 }]),
        ("addi a0, a0, %lo(target)", vec![i(AluOp::Add, A0, A0, 0)]),
        (
            "sw a1, %lo(target)(a0)",
            vec![Store {
                op: StoreOp::Sw,
                rs1: A0,
                rs2: A1,
                imm: 0,
            }],
        ),
        // Explicit pc-relative pairs: %pcrel_lo names the label on the auipc.
        (
            "1: auipc a0, %pcrel_hi(target)\naddi a0, a0, %pcrel_lo(1b)",
            vec![Auipc { rd: A0, imm: 0 }, i(AluOp::Add, A0, A0, 0)],
        ),
        (
            "1: auipc a0, %pcrel_hi(target)\nlw a1, %pcrel_lo(1b)(a0)",
            vec![
                Auipc { rd: A0, imm: 0 },
                Load {
                    op: LoadOp::Lw,
                    rd: A1,
                    rs1: A0,
                    imm: 0,
                },
            ],
        ),
        (
            "2: auipc t0, %pcrel_hi(target+8)\nsw a1, %pcrel_lo(2b)(t0)",
            vec![
                Auipc { rd: T0, imm: 0 },
                Store {
                    op: StoreOp::Sw,
                    rs1: T0,
                    rs2: A1,
                    imm: 0,
                },
            ],
        ),
    ]
}

/// (word index, kind, target)
type WordReloc = (usize, RelocKind, To);

/// The relocation on each word of the rows above that refer to a label.
/// Every word not listed must carry no relocation. Matches clang/llvm-mc with
/// relaxation off.
fn expected_relocs() -> Vec<(&'static str, Vec<WordReloc>)> {
    use RelocKind::*;
    let target = || To::Sym("target".into(), 0);
    let pcrel = |lo| vec![(0, PcrelHi20, target()), (1, lo, To::Auipc(0))];
    vec![
        ("call target", vec![(0, CallPlt, target())]),
        ("tail target", vec![(0, CallPlt, target())]),
        ("la a0, target", pcrel(PcrelLo12I)),
        ("lla a0, target", pcrel(PcrelLo12I)),
        ("lw a0, target", pcrel(PcrelLo12I)),
        ("sw a0, target, t0", pcrel(PcrelLo12S)),
        ("beq a0, a1, target", vec![(0, Branch, target())]),
        ("jal target", vec![(0, Jal, target())]),
        ("lui a0, %hi(target)", vec![(0, Hi20, target())]),
        ("addi a0, a0, %lo(target)", vec![(0, Lo12I, target())]),
        ("sw a1, %lo(target)(a0)", vec![(0, Lo12S, target())]),
        // The low half points at the user's label (`1:` is `.L1$0`), not the symbol.
        (
            "1: auipc a0, %pcrel_hi(target)\naddi a0, a0, %pcrel_lo(1b)",
            vec![
                (0, PcrelHi20, target()),
                (1, PcrelLo12I, To::Sym(".L1$0".into(), 0)),
            ],
        ),
        (
            "1: auipc a0, %pcrel_hi(target)\nlw a1, %pcrel_lo(1b)(a0)",
            vec![
                (0, PcrelHi20, target()),
                (1, PcrelLo12I, To::Sym(".L1$0".into(), 0)),
            ],
        ),
        (
            "2: auipc t0, %pcrel_hi(target+8)\nsw a1, %pcrel_lo(2b)(t0)",
            vec![
                (0, PcrelHi20, To::Sym("target".into(), 8)),
                (1, PcrelLo12S, To::Sym(".L2$0".into(), 0)),
            ],
        ),
    ]
}

/// A typo in `expected_relocs` would silently drop its checks.
#[test]
fn every_expected_reloc_row_is_in_the_table() {
    let rows: Vec<&str> = expected().iter().map(|(src, _)| *src).collect();
    for (src, relocs) in expected_relocs() {
        assert!(rows.contains(&src), "'{src}' is not in expected()");
        let words = expected().into_iter().find(|(s, _)| *s == src).unwrap().1.len();
        assert!(
            relocs.iter().all(|r| r.0 < words),
            "'{src}': word index out of range"
        );
    }
}

fn run_expected(require_all: bool) {
    let mut rep = Report::new("decode(encode(x)) == x");
    let relocs = expected_relocs();
    for (src, want) in expected() {
        rep.total += 1;
        let shown = src.replace('\n', "; ");
        let words = match assemble(src) {
            Assembled::Words(w) => w,
            Assembled::Unimplemented => {
                // Report the mnemonic of the last line (the one under test).
                rep.todo.push(src.lines().last().unwrap_or(src).to_string());
                continue;
            }
            Assembled::Error(e) => {
                rep.fails.push(format!("  {shown:<34} error: {e}"));
                continue;
            }
        };
        let got: Vec<Inst> = words.iter().map(|w| decode(w.word)).collect();
        if got != want {
            rep.fails
                .push(format!("  {shown:<34} got {got:?}\n  {:<34} want {want:?}", ""));
            continue;
        }
        let want_relocs = relocs
            .iter()
            .find(|(s, _)| *s == src)
            .map_or(&[][..], |r| &r.1[..]);
        let mismatch = words.iter().enumerate().find_map(|(n, w)| {
            let want = want_relocs
                .iter()
                .find(|r| r.0 == n)
                .map(|(_, kind, to)| (*kind, to));
            let got = w.reloc.as_ref().map(|r| (r.kind, &r.to));
            (got != want).then(|| format!("word {n}: relocation {got:?}, want {want:?}"))
        });
        match mismatch {
            None => rep.pass += 1,
            Some(why) => rep.fails.push(format!("  {shown:<34} {why}")),
        }
    }
    rep.finish(require_all);
}

// ----- decode → disassemble → assemble again -----

/// Each golden line with llvm-mc's words for it, leaving out the words a
/// relocation patches (their fields are zero, so they don't disassemble back
/// to the source). Lines left with no words are dropped.
fn golden_words() -> Vec<(&'static str, Vec<u32>)> {
    let hex = |s: &str| u32::from_str_radix(s, 16).unwrap();
    include_str!("golden/encode.golden")
        .lines()
        .filter_map(|l| {
            let mut parts = l.split(" | ");
            let src = parts.next().unwrap().trim();
            let words = parts.next().unwrap().split_whitespace().map(hex);
            let relocated: Vec<u32> = parts
                .next()
                .unwrap_or("")
                .split_whitespace()
                .map(|r| hex(r.split(':').next().unwrap()))
                .collect();
            let words: Vec<u32> = words
                .enumerate()
                .filter(|(i, _)| !relocated.contains(&(4 * *i as u32)))
                .map(|(_, w)| w)
                .collect();
            (!words.is_empty()).then_some((src, words))
        })
        .collect()
}

fn run_disasm(require_all: bool) {
    let mut rep = Report::new("encode(disasm(decode(w))) == w");
    // Not in the golden file. Words from clang (Xcal written as `.insn i 0x0b, ...`).
    let extra = [
        ("xcal.dbg a0", vec![0x0005_000b]),
        ("xcal.halt zero", vec![0x0000_100b]),
        ("csrrw a0, satp, a0", vec![0x1805_1573]),
        ("csrrs a0, 0x7c0, zero", vec![0x7c00_2573]),
    ];
    for (src, words) in golden_words().into_iter().chain(extra) {
        rep.total += 1;
        let mut outcome = Ok(());
        for w in words {
            // `{inst}` is the disassembler's syntax, which merlin-as accepts.
            // Words are compared, not decoded `Inst`s: `decode` ignores some
            // bits (fence's pred/succ, for one) that must still come out right.
            let text = decode(w).to_string();
            match assemble(&text) {
                Assembled::Words(again) if again.len() == 1 && again[0].word == w => {}
                Assembled::Words(again) => {
                    let again: Vec<String> = again.iter().map(|w| format!("{:08x}", w.word)).collect();
                    outcome = Err(Some(format!(
                        "{w:08x} disassembles to '{text}', which assembles to [{}]",
                        again.join(" ")
                    )));
                    break;
                }
                // The disassembly uses a mnemonic not written yet.
                Assembled::Unimplemented => {
                    outcome = Err(None);
                    break;
                }
                Assembled::Error(e) => {
                    outcome = Err(Some(format!("{w:08x} disassembles to '{text}': {e}")));
                    break;
                }
            }
        }
        match outcome {
            Ok(()) => rep.pass += 1,
            Err(None) => rep.todo.push(src.to_string()),
            Err(Some(why)) => rep.fails.push(format!("  {src:<28} {why}")),
        }
    }
    rep.finish(require_all);
}

#[test]
fn roundtrip_expected_progress() {
    run_expected(false);
}

#[test]
#[ignore = "passes once the encoder is complete"]
fn roundtrip_expected_complete() {
    run_expected(true);
}

#[test]
fn roundtrip_disasm_progress() {
    run_disasm(false);
}

#[test]
#[ignore = "passes once the encoder is complete"]
fn roundtrip_disasm_complete() {
    run_disasm(true);
}
