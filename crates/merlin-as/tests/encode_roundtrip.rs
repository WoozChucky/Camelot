//! Checks the encoder against the shared decoder in `excalibur`, which Grail
//! already runs riscv-tests with. Two independent checks, both independent of
//! LLVM:
//!
//! * `expected`: a hand-written table of source lines and the [`Inst`] values
//!   `excalibur::decode` must give back for each word, plus the relocation
//!   each word carries (`expected_relocs`; every other word must carry none).
//!   This covers what the golden file can't (Xcal, CSRs by name, `li` splits,
//!   explicit `%pcrel_hi`/`%pcrel_lo` pairs on user labels) and spells out the
//!   operation and operands of every mnemonic in the golden file.
//! * `CLANG_WORDS`: lines the golden file doesn't have (every `.insn` form,
//!   counter reads, short forms, `li` edges) must give clang's exact words.
//! * `disasm`: every word LLVM produced for the golden file (plus a few
//!   more from clang), decoded and printed with the disassembler, must
//!   assemble back to exactly that word. Proves the disassembler's syntax
//!   reassembles, and catches bits `decode` ignores, like fence's pred/succ.
//!   Words patched by a relocation are left out: their fields are zero.
//!
//! Each check runs for both widths. On RV64 the decoder and the disassembler
//! are given `Xlen::Rv64`, the RV64-only instructions have tables of their
//! own (`expected64`, `CLANG_WORDS64`, the `.d` atomics, rv64/encode.golden),
//! and the RV32 rows the width changes are left out (`WIDTH_DEPENDENT`).

use excalibur::{
    AluOp, AmoOp, AmoWidth, BranchOp, CsrOp, CsrSrc, Inst, LoadOp, MulOp, Ordering, StoreOp, Syntax, Xlen,
};
use merlin_as::reloc::RelocKind;
use merlin_as::Options;

const RV32: Xlen = Xlen::Rv32;
const RV64: Xlen = Xlen::Rv64;

fn decode(w: u32, xlen: Xlen) -> Inst {
    excalibur::decode(w, xlen)
}

/// The disassembler's plain text for `inst` (decimal immediates, no
/// pseudo-instructions), which merlin-as must accept. The width matters for
/// CSR names only: `cycleh` is a name on RV32 and a number on RV64.
fn disasm(inst: Inst, xlen: Xlen) -> String {
    inst.format(None, Syntax::plain(xlen)).to_string()
}

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

/// The words of `.text` for `xlen`, or the first error.
fn assemble(src: &str, xlen: Xlen) -> Result<Vec<Word>, String> {
    let obj = merlin_as::assemble_with(src, &Options { xlen }).map_err(|errs| errs[0].to_string())?;
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
    Ok(words)
}

/// Collects failures and reports them all at once.
struct Report {
    name: &'static str,
    total: usize,
    fails: Vec<String>,
}

impl Report {
    fn new(name: &'static str) -> Self {
        Report {
            name,
            total: 0,
            fails: Vec::new(),
        }
    }

    fn finish(self) {
        assert!(
            self.fails.is_empty(),
            "{}: {} of {} failed:\n{}",
            self.name,
            self.fails.len(),
            self.total,
            self.fails.join("\n")
        );
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

/// Source lines, each with the instructions `decode` must return for it.
type Rows = Vec<(&'static str, Vec<Inst>)>;

/// Source and the instructions `decode` must return, one per emitted word.
/// Words carrying a relocation have zero immediates, so their expected
/// immediates are 0.
fn expected() -> Rows {
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
                imm: 0xffff_f000u32 as i32,
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
        // A: the address register is rs1, the source (sc.w and AMOs) rs2.
        // Every mnemonic and suffix is in `roundtrip_atomics`.
        (
            "lr.w a0, (a1)",
            vec![Lr {
                width: AmoWidth::W,
                rd: A0,
                rs1: A1,
                order: Ordering::NONE,
            }],
        ),
        (
            "sc.w.rl t0, t1, 0(t2)",
            vec![Sc {
                width: AmoWidth::W,
                rd: T0,
                rs1: T2,
                rs2: T1,
                order: Ordering { aq: false, rl: true },
            }],
        ),
        (
            "amomaxu.w.aq zero, a5, (sp)",
            vec![Amo {
                op: AmoOp::Maxu,
                width: AmoWidth::W,
                rd: ZERO,
                rs1: SP,
                rs2: A5,
                order: Ordering { aq: true, rl: false },
            }],
        ),
        // System
        ("fence", vec![Fence { pred: 0xf, succ: 0xf }]),
        ("fence.i", vec![FenceI]),
        ("ecall", vec![Ecall]),
        ("ebreak", vec![Ebreak]),
        ("mret", vec![Mret]),
        ("sret", vec![Sret]),
        ("wfi", vec![Wfi]),
        ("sfence.vma", vec![SfenceVma { rs1: ZERO, rs2: ZERO }]),
        ("sfence.vma a0, a1", vec![SfenceVma { rs1: A0, rs2: A1 }]),
        ("sfence.vma a0", vec![SfenceVma { rs1: A0, rs2: ZERO }]),
        (
            "jalr ra, t0, 8",
            vec![Jalr {
                rd: RA,
                rs1: T0,
                imm: 8,
            }],
        ),
        // Counter reads: csrrs rd, counter, zero.
        ("rdcycle a0", vec![csr(CsrOp::Rs, A0, CsrSrc::Reg(ZERO), 0xc00)]),
        ("rdtime a0", vec![csr(CsrOp::Rs, A0, CsrSrc::Reg(ZERO), 0xc01)]),
        ("rdinstret a0", vec![csr(CsrOp::Rs, A0, CsrSrc::Reg(ZERO), 0xc02)]),
        ("rdcycleh a0", vec![csr(CsrOp::Rs, A0, CsrSrc::Reg(ZERO), 0xc80)]),
        ("rdtimeh a0", vec![csr(CsrOp::Rs, A0, CsrSrc::Reg(ZERO), 0xc81)]),
        (
            "rdinstreth a0",
            vec![csr(CsrOp::Rs, A0, CsrSrc::Reg(ZERO), 0xc82)],
        ),
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
        // A number where the register goes: the immediate form, as in LLVM.
        (
            "csrw mstatus, 8",
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
        // Xcal (spec section 9), not in the LLVM golden file
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
        // sgt a, b is slt b, a: the sources swapped.
        ("sgt a0, a1, a2", vec![r(AluOp::Slt, A0, A2, A1)]),
        ("sgtu t0, t1, t2", vec![r(AluOp::Sltu, T0, T2, T1)]),
        ("zext.b a0, a1", vec![i(AluOp::And, A0, A1, 0xff)]),
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
        // 33 << 11: c is just outside -32..=31, so lui + addi.
        (
            "li a0, 0x10800",
            vec![
                Lui {
                    rd: A0,
                    imm: 0x0001_1000,
                },
                i(AluOp::Add, A0, A0, -2048),
            ],
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
                imm: 0x8000_0000u32 as i32,
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
                    imm: 0xffff_f000u32 as i32,
                },
                i(AluOp::Add, A0, A0, 2047),
            ],
        ),
        (
            "li a0, 0x7ffff800",
            vec![
                Lui {
                    rd: A0,
                    imm: 0x8000_0000u32 as i32,
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

/// Rows of relocations: a source line, and the relocation on each of its words that has one.
type RelocRows = Vec<(&'static str, Vec<WordReloc>)>;

/// The relocation on each word of the rows above that refer to a label.
/// Every word not listed must carry no relocation. Matches clang/llvm-mc with
/// relaxation off.
fn expected_relocs() -> RelocRows {
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

/// The rows of `expected()` that encode differently on RV64, or not at all,
/// and why. `roundtrip_expected_on_rv64` leaves them out.
const WIDTH_DEPENDENT: &[(&str, &str)] = &[
    ("rdcycleh a0", "RV32 only"),
    ("rdtimeh a0", "RV32 only"),
    ("rdinstreth a0", "RV32 only"),
    ("li a0, 0x80000000", "positive on RV64: addi 1; slli 31"),
    ("li a0, 0x7ffff800", "lui alone would sign-extend it: lui; addiw"),
];

/// RV64-only source lines and the instructions `decode` must return for them.
fn expected64() -> Rows {
    use Inst::*;
    let w32 = |op, rd, rs1, imm| OpImm32 { op, rd, rs1, imm };
    let r32 = |op, rd, rs1, rs2| Op32 { op, rd, rs1, rs2 };
    let m32 = |op, rd, rs1, rs2| Mul32 { op, rd, rs1, rs2 };
    let load = |op, rd, rs1, imm| Load { op, rd, rs1, imm };
    let store = |op, rs1, rs2, imm| Store { op, rs1, rs2, imm };
    vec![
        // OP-IMM-32 and OP-32
        ("addiw a0, a1, -1", vec![w32(AluOp::Add, A0, A1, -1)]),
        ("addiw sp, sp, 2047", vec![w32(AluOp::Add, SP, SP, 2047)]),
        ("slliw a0, a1, 31", vec![w32(AluOp::Sll, A0, A1, 31)]),
        ("srliw t0, t1, 1", vec![w32(AluOp::Srl, T0, T1, 1)]),
        ("sraiw a0, a1, 0", vec![w32(AluOp::Sra, A0, A1, 0)]),
        ("addw a0, a1, a2", vec![r32(AluOp::Add, A0, A1, A2)]),
        ("subw t0, t1, t2", vec![r32(AluOp::Sub, T0, T1, T2)]),
        ("sllw a0, a1, a2", vec![r32(AluOp::Sll, A0, A1, A2)]),
        ("srlw t5, t6, a0", vec![r32(AluOp::Srl, T5, T6, A0)]),
        ("sraw a1, a2, a3", vec![r32(AluOp::Sra, A1, A2, A3)]),
        ("mulw a0, a1, a2", vec![m32(MulOp::Mul, A0, A1, A2)]),
        ("divw a0, a1, a2", vec![m32(MulOp::Div, A0, A1, A2)]),
        ("divuw s2, s3, a0", vec![m32(MulOp::Divu, S2, S3, A0)]),
        ("remw a0, a1, a2", vec![m32(MulOp::Rem, A0, A1, A2)]),
        ("remuw t0, t1, t2", vec![m32(MulOp::Remu, T0, T1, T2)]),
        // 6-bit shift amounts
        ("slli a0, a1, 63", vec![i(AluOp::Sll, A0, A1, 63)]),
        ("srli a0, a1, 32", vec![i(AluOp::Srl, A0, A1, 32)]),
        ("srai a0, a1, 45", vec![i(AluOp::Sra, A0, A1, 45)]),
        // Doublewords and unsigned words
        ("ld ra, 2040(sp)", vec![load(LoadOp::Ld, RA, SP, 2040)]),
        ("ld a0, (a1)", vec![load(LoadOp::Ld, A0, A1, 0)]),
        ("lwu t0, -2048(t1)", vec![load(LoadOp::Lwu, T0, T1, -2048)]),
        ("sd ra, 2040(sp)", vec![store(StoreOp::Sd, SP, RA, 2040)]),
        ("sd a5, -8(s2)", vec![store(StoreOp::Sd, S2, A5, -8)]),
        (
            "lr.d.aqrl a0, (a1)",
            vec![Lr {
                width: AmoWidth::D,
                rd: A0,
                rs1: A1,
                order: Ordering { aq: true, rl: true },
            }],
        ),
        // Pseudo-instructions
        ("sext.w a0, a1", vec![w32(AluOp::Add, A0, A1, 0)]),
        ("negw a0, a1", vec![r32(AluOp::Sub, A0, ZERO, A1)]),
        (
            "zext.w a0, a1",
            vec![i(AluOp::Sll, A0, A1, 32), i(AluOp::Srl, A0, A0, 32)],
        ),
        // A CSR that has a name on RV32 only is still reachable by number.
        (
            "csrr a0, 0xc80",
            vec![csr(CsrOp::Rs, A0, CsrSrc::Reg(ZERO), 0xc80)],
        ),
        // li, as LLVM's RISCVMatInt builds 64-bit values (rv64/li.golden has
        // many more, word for word): addiw only where addi would be wrong,
        // then shifts, a final srli for leading zeros, xori for leading ones.
        (
            "li a0, 0x7fffffff",
            vec![
                Lui {
                    rd: A0,
                    imm: 0x8000_0000u32 as i32,
                },
                w32(AluOp::Add, A0, A0, -1),
            ],
        ),
        (
            "li a0, 0x80000000",
            vec![i(AluOp::Add, A0, ZERO, 1), i(AluOp::Sll, A0, A0, 31)],
        ),
        (
            "li a0, 0xffffffff",
            vec![i(AluOp::Add, A0, ZERO, -1), i(AluOp::Srl, A0, A0, 32)],
        ),
        (
            "li a0, 0x100000001",
            vec![
                i(AluOp::Add, A0, ZERO, 1),
                i(AluOp::Sll, A0, A0, 32),
                i(AluOp::Add, A0, A0, 1),
            ],
        ),
        (
            "li a0, 0x8000000000000fff",
            vec![
                Lui {
                    rd: A0,
                    imm: 0xffff_e000u32 as i32,
                },
                i(AluOp::Srl, A0, A0, 1),
                i(AluOp::Xor, A0, A0, -1),
            ],
        ),
        // Label operands: relocated fields are zero, the rest must be right.
        (
            "ld a0, target",
            vec![Auipc { rd: A0, imm: 0 }, load(LoadOp::Ld, A0, A0, 0)],
        ),
        (
            "lwu a0, target",
            vec![Auipc { rd: A0, imm: 0 }, load(LoadOp::Lwu, A0, A0, 0)],
        ),
        (
            "sd a0, target, t0",
            vec![Auipc { rd: T0, imm: 0 }, store(StoreOp::Sd, T0, A0, 0)],
        ),
        ("ld a1, %lo(target)(a0)", vec![load(LoadOp::Ld, A1, A0, 0)]),
        ("addiw a0, a0, %lo(target)", vec![w32(AluOp::Add, A0, A0, 0)]),
    ]
}

/// The relocations on `expected64()`'s rows, as `expected_relocs` for
/// `expected()`'s.
fn expected_relocs64() -> RelocRows {
    use RelocKind::*;
    let target = || To::Sym("target".into(), 0);
    let pcrel = |lo| vec![(0, PcrelHi20, target()), (1, lo, To::Auipc(0))];
    vec![
        ("ld a0, target", pcrel(PcrelLo12I)),
        ("lwu a0, target", pcrel(PcrelLo12I)),
        ("sd a0, target, t0", pcrel(PcrelLo12S)),
        ("ld a1, %lo(target)(a0)", vec![(0, Lo12I, target())]),
        ("addiw a0, a0, %lo(target)", vec![(0, Lo12I, target())]),
    ]
}

/// The rows and relocations `roundtrip_expected` checks on `xlen`.
fn rows(xlen: Xlen) -> (Rows, RelocRows) {
    match xlen {
        Xlen::Rv32 => (expected(), expected_relocs()),
        Xlen::Rv64 => {
            let same = |src: &str| !WIDTH_DEPENDENT.iter().any(|(s, _)| *s == src);
            let mut rows: Vec<_> = expected().into_iter().filter(|(src, _)| same(src)).collect();
            rows.extend(expected64());
            let mut relocs: Vec<_> = expected_relocs()
                .into_iter()
                .filter(|(src, _)| same(src))
                .collect();
            relocs.extend(expected_relocs64());
            (rows, relocs)
        }
    }
}

/// A typo in `expected_relocs` or `WIDTH_DEPENDENT` would silently drop
/// their checks.
#[test]
fn every_expected_reloc_row_is_in_the_table() {
    for (table, relocs) in [
        (expected(), expected_relocs()),
        (expected64(), expected_relocs64()),
    ] {
        let rows: Vec<&str> = table.iter().map(|(src, _)| *src).collect();
        for (src, relocs) in relocs {
            assert!(rows.contains(&src), "'{src}' is not in its table");
            let words = table.iter().find(|(s, _)| *s == src).unwrap().1.len();
            assert!(
                relocs.iter().all(|r| r.0 < words),
                "'{src}': word index out of range"
            );
        }
    }
    let rows: Vec<&str> = expected().iter().map(|(src, _)| *src).collect();
    for (src, _) in WIDTH_DEPENDENT {
        assert!(rows.contains(src), "'{src}' is not in expected()");
    }
}

#[test]
fn roundtrip_expected() {
    check_expected(RV32);
}

#[test]
fn roundtrip_expected_on_rv64() {
    check_expected(RV64);
}

fn check_expected(xlen: Xlen) {
    let mut rep = Report::new("decode(encode(x)) == x");
    let (rows, relocs) = rows(xlen);
    for (src, want) in rows {
        rep.total += 1;
        let shown = src.replace('\n', "; ");
        let words = match assemble(src, xlen) {
            Ok(w) => w,
            Err(e) => {
                rep.fails.push(format!("  {shown:<34} error: {e}"));
                continue;
            }
        };
        let got: Vec<Inst> = words.iter().map(|w| decode(w.word, xlen)).collect();
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
        if let Some(why) = mismatch {
            rep.fails.push(format!("  {shown:<34} {why}"));
        }
    }
    rep.finish();
}

/// Every A mnemonic with every ordering suffix, with registers that vary per
/// line, decodes to the instruction it names: the `.w` forms on both widths,
/// the `.d` forms on RV64. The mnemonic-to-operation table here is the
/// test's own.
#[test]
fn roundtrip_atomics() {
    check_atomics(RV32, AmoWidth::W);
}

#[test]
fn roundtrip_atomics_on_rv64() {
    check_atomics(RV64, AmoWidth::W);
    check_atomics(RV64, AmoWidth::D);
}

fn check_atomics(xlen: Xlen, width: AmoWidth) {
    let mut rep = Report::new("A extension: decode(encode(x)) == x");
    let w = match width {
        AmoWidth::W => "w",
        AmoWidth::D => "d",
    };
    let amos = [
        ("amoswap", AmoOp::Swap),
        ("amoadd", AmoOp::Add),
        ("amoxor", AmoOp::Xor),
        ("amoand", AmoOp::And),
        ("amoor", AmoOp::Or),
        ("amomin", AmoOp::Min),
        ("amomax", AmoOp::Max),
        ("amominu", AmoOp::Minu),
        ("amomaxu", AmoOp::Maxu),
    ];
    let orders = [
        ("", false, false),
        (".aq", true, false),
        (".rl", false, true),
        (".aqrl", true, true),
    ];
    let name = |r: u8| excalibur::reg::abi_name(r);
    let mut n = 0u8;
    for base in ["lr", "sc"].into_iter().chain(amos.iter().map(|a| a.0)) {
        for (suffix, aq, rl) in orders {
            // Spread the registers over all 32 (rd, rs1 and rs2 differ).
            let (rd, rs1, rs2) = (n % 32, (n + 11) % 32, (n + 23) % 32);
            n += 5;
            let order = Ordering { aq, rl };
            let (src, want) = match base {
                "lr" => (
                    format!("lr.{w}{suffix} {}, ({})", name(rd), name(rs1)),
                    Inst::Lr {
                        width,
                        rd,
                        rs1,
                        order,
                    },
                ),
                "sc" => (
                    format!("sc.{w}{suffix} {}, {}, ({})", name(rd), name(rs2), name(rs1)),
                    Inst::Sc {
                        width,
                        rd,
                        rs1,
                        rs2,
                        order,
                    },
                ),
                _ => {
                    let op = amos.iter().find(|a| a.0 == base).unwrap().1;
                    (
                        format!("{base}.{w}{suffix} {}, {}, ({})", name(rd), name(rs2), name(rs1)),
                        Inst::Amo {
                            op,
                            width,
                            rd,
                            rs1,
                            rs2,
                            order,
                        },
                    )
                }
            };
            rep.total += 1;
            match assemble(&src, xlen) {
                Ok(words) if words.len() == 1 && decode(words[0].word, xlen) == want => {
                    // And the disassembly is the source back.
                    let text = disasm(want, xlen);
                    if text != src {
                        rep.fails.push(format!("  {src:<36} disassembles to '{text}'"));
                    }
                }
                Ok(words) => {
                    let got: Vec<Inst> = words.iter().map(|w| decode(w.word, xlen)).collect();
                    rep.fails.push(format!("  {src:<36} got {got:?}, want {want:?}"));
                }
                Err(e) => rep.fails.push(format!("  {src:<36} error: {e}")),
            }
        }
    }
    assert_eq!(rep.total, 44);
    rep.finish();
}

// ----- exact words from clang -----

/// Lines not in the golden file, with the words clang 21.1.5 gives for each
/// (`clang --target=riscv32 -march=rv32im_zicsr_zifencei -mno-relax -c`,
/// one line per file, read back with `llvm-objdump -d`). Pins the encodings
/// that `expected()` can't tell apart through `decode` or doesn't list: every
/// `.insn` form, the counter reads, `zext.b`, the `jalr`/`csrw`/`sfence.vma`
/// short forms and the edges of `li`'s shift form.
const CLANG_WORDS: &[(&str, &[u32])] = &[
    (".insn r 0x33, 0, 0x20, a0, a1, a2", &[0x40c5_8533]),
    (".insn r 0x0b, 7, 0x7f, t6, s0, a5", &[0xfef4_7f8b]),
    (".insn i 0x13, 0, a0, a1, -5", &[0xffb5_8513]),
    (".insn i 0x03, 2, a0, 8(a1)", &[0x0085_a503]),
    (".insn i 0x67, 0, ra, -16(t0)", &[0xff02_80e7]),
    (".insn s 0x23, 2, a1, 12(a0)", &[0x00b5_2623]),
    (".insn s 0x23, 0, t1, -2048(sp)", &[0x8061_0023]),
    (".insn b 0x63, 1, a0, a1, 16", &[0x00b5_1863]),
    (".insn b 0x63, 4, t0, t1, -4096", &[0x8062_c063]),
    (".insn u 0x37, a0, 0x12345", &[0x1234_5537]),
    (".insn u 0x17, t0, 0xfffff", &[0xffff_f297]),
    (".insn 0x00b50533", &[0x00b5_0533]),
    (".insn 4, 0x40b50533", &[0x40b5_0533]),
    ("zext.b a0, a1", &[0x0ff5_f513]),
    ("rdcycle a0", &[0xc000_2573]),
    ("rdtime a1", &[0xc010_25f3]),
    ("rdinstret a2", &[0xc020_2673]),
    ("rdcycleh a3", &[0xc800_26f3]),
    ("rdtimeh a4", &[0xc810_2773]),
    ("rdinstreth a5", &[0xc820_27f3]),
    ("jalr ra, t0, 8", &[0x0082_80e7]),
    ("jalr a0, a1, -2048", &[0x8005_8567]),
    ("jalr t0", &[0x0002_80e7]),
    // A number where a csrw/csrs/csrc wants a register: the immediate form.
    ("csrw mstatus, 8", &[0x3004_5073]),
    ("csrs sie, 31", &[0x104f_e073]),
    ("csrc sstatus, 2", &[0x1001_7073]),
    ("sfence.vma a0", &[0x1205_0073]),
    // li's shift form takes c in -32..=31 only: 33 << 11 and -33 << 11 are
    // lui + addi, 31 << 11 and -31 << 11 are addi + slli.
    ("li a0, 0x10800", &[0x0001_1537, 0x8005_0513]),
    ("li a0, -0x10800", &[0xffff_0537, 0x8005_0513]),
    ("li a0, 0xf800", &[0x01f0_0513, 0x00b5_1513]),
    ("li a0, -0xf800", &[0xfe10_0513, 0x00b5_1513]),
    ("li a0, -3968", &[0xfe10_0513, 0x0075_1513]), // -31 << 7
    ("li a0, -4224", &[0xffff_f537, 0xf805_0513]), // -33 << 7
    ("li a0, 0x7ff800", &[0x0080_0537, 0x8005_0513]),
];

/// The same for RV64 (`clang --target=riscv64 -march=rv64ima_zicsr_zifencei
/// -mabi=lp64 -mno-relax -c`, LLVM 23.1.4): `CLANG_WORDS` without the RV32-only
/// counter reads, the `.insn` forms of the RV64 opcodes and of `ld`/`lwu`/`sd`,
/// RV32-only CSRs by number, `%hi`/`%lo` of constants wider than 32 bits, and
/// `li` of values written in ways that wrap or shift in 64 bits.
const CLANG_WORDS64: &[(&str, &[u32])] = &[
    (".insn r 0x33, 0, 0x20, a0, a1, a2", &[0x40c5_8533]),
    (".insn r 0x0b, 7, 0x7f, t6, s0, a5", &[0xfef4_7f8b]),
    (".insn i 0x13, 0, a0, a1, -5", &[0xffb5_8513]),
    (".insn i 0x03, 2, a0, 8(a1)", &[0x0085_a503]),
    (".insn i 0x67, 0, ra, -16(t0)", &[0xff02_80e7]),
    (".insn s 0x23, 2, a1, 12(a0)", &[0x00b5_2623]),
    (".insn s 0x23, 0, t1, -2048(sp)", &[0x8061_0023]),
    (".insn b 0x63, 1, a0, a1, 16", &[0x00b5_1863]),
    (".insn b 0x63, 4, t0, t1, -4096", &[0x8062_c063]),
    (".insn u 0x37, a0, 0x12345", &[0x1234_5537]),
    (".insn u 0x17, t0, 0xfffff", &[0xffff_f297]),
    (".insn 0x00b50533", &[0x00b5_0533]),
    (".insn 4, 0x40b50533", &[0x40b5_0533]),
    // OP-32 (addw, mulw, sraw), OP-IMM-32 (addiw), ld, lwu and sd.
    (".insn r 0x3b, 0, 0, a0, a1, a2", &[0x00c5_853b]),
    (".insn r 0x3b, 0, 1, a0, a1, a2", &[0x02c5_853b]),
    (".insn r 0x3b, 5, 0x20, a0, a1, a2", &[0x40c5_d53b]),
    (".insn i 0x1b, 0, a0, a1, -1", &[0xfff5_851b]),
    (".insn i 0x03, 3, a0, 8(a1)", &[0x0085_b503]),
    (".insn i 0x03, 6, a0, 8(a1)", &[0x0085_e503]),
    (".insn s 0x23, 3, a1, 12(a0)", &[0x00b5_3623]),
    ("zext.b a0, a1", &[0x0ff5_f513]),
    ("rdcycle a0", &[0xc000_2573]),
    ("rdtime a1", &[0xc010_25f3]),
    ("rdinstret a2", &[0xc020_2673]),
    ("jalr ra, t0, 8", &[0x0082_80e7]),
    ("jalr a0, a1, -2048", &[0x8005_8567]),
    ("jalr t0", &[0x0002_80e7]),
    ("csrw mstatus, 8", &[0x3004_5073]),
    ("csrs sie, 31", &[0x104f_e073]),
    ("csrc sstatus, 2", &[0x1001_7073]),
    ("sfence.vma a0", &[0x1205_0073]),
    // cycleh and mstatush have no names on RV64, but their numbers assemble.
    ("csrr a0, 0xc80", &[0xc800_2573]),
    ("csrrs a0, 0x310, zero", &[0x3100_2573]),
    ("csrr a0, pmpcfg2", &[0x3a20_2573]),
    ("csrr a1, cycle", &[0xc000_25f3]),
    // The shift form's edges, as on RV32.
    ("li a0, 0x10800", &[0x0001_1537, 0x8005_0513]),
    ("li a0, -0x10800", &[0xffff_0537, 0x8005_0513]),
    ("li a0, 0xf800", &[0x01f0_0513, 0x00b5_1513]),
    ("li a0, -0xf800", &[0xfe10_0513, 0x00b5_1513]),
    ("li a0, -3968", &[0xfe10_0513, 0x0075_1513]),
    ("li a0, -4224", &[0xffff_f537, 0xf805_0513]),
    ("li a0, 0x7ff800", &[0x0080_0537, 0x8005_0513]),
    // Leading zeros: build the value shifted up, then srli.
    ("li a0, 0xfffffffff", &[0xfff0_0513, 0x01c5_5513]),
    (
        "li a0, 0x0000ffff0000ffff",
        &[0xf000_1537, 0x0145_1513, 0xfff5_0513, 0x0105_5513],
    ),
    // Values that wrap in 64 bits: -(2^63) is 2^63, and -(2^63 + 1) is
    // 2^63 - 1; a u64 literal is its two's complement.
    ("li a0, -0x8000000000000000", &[0xfff0_0513, 0x03f5_1513]),
    ("li a0, -0x8000000000000001", &[0xfff0_0513, 0x0015_5513]),
    ("li a0, 18446744073709551615", &[0xfff0_0513]),
    // Expressions are 64-bit: >> is logical, << reaches bit 32.
    ("li a0, -1 >> 1", &[0xfff0_0513, 0x0015_5513]),
    ("li a0, 1 << 32", &[0x0010_0513, 0x0205_1513]),
    // %hi/%lo of a constant take its low 32 bits, whatever its width.
    ("lui a0, %hi(0x123456789)", &[0x2345_6537]),
    ("addi a0, a0, %lo(0x123456789)", &[0x7895_0513]),
    ("lui a0, %hi(0xffffffff7ffff800)", &[0x8000_0537]),
    ("addi a0, a0, %lo(0xffffffff7ffff800)", &[0x8005_0513]),
    ("addiw a0, a0, 2047", &[0x7ff5_051b]),
    ("addiw a0, a0, -2048", &[0x8005_051b]),
    ("sraiw a0, a1, 0", &[0x4005_d51b]),
    ("slli zero, zero, 63", &[0x03f0_1013]),
];

#[test]
fn matches_clang_words() {
    check_clang_words(CLANG_WORDS, RV32);
}

#[test]
fn matches_clang_words_on_rv64() {
    check_clang_words(CLANG_WORDS64, RV64);
}

fn check_clang_words(lines: &[(&str, &[u32])], xlen: Xlen) {
    let mut rep = Report::new("words == clang's");
    for &(src, want) in lines {
        rep.total += 1;
        match assemble(src, xlen) {
            Ok(words) => {
                let got: Vec<u32> = words.iter().map(|w| w.word).collect();
                if got != want || words.iter().any(|w| w.reloc.is_some()) {
                    rep.fails
                        .push(format!("  {src:<36} got {got:08x?}, want {want:08x?}"));
                }
            }
            Err(e) => rep.fails.push(format!("  {src:<36} error: {e}")),
        }
    }
    rep.finish();
}

// ----- decode → disassemble → assemble again -----

/// Each line of a golden file with LLVM's words for it, leaving out the
/// words a relocation patches (their fields are zero, so they don't
/// disassemble back to the source). Lines left with no words are dropped.
fn golden_words(golden: &'static str) -> Vec<(&'static str, Vec<u32>)> {
    let hex = |s: &str| u32::from_str_radix(s, 16).unwrap();
    golden
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

/// Re-assembles the plain disassembly of `w` for `xlen`. Err: why it doesn't
/// round-trip.
fn reassemble(w: u32, xlen: Xlen) -> Result<u32, String> {
    // The disassembler's syntax, which merlin-as accepts.
    let text = disasm(decode(w, xlen), xlen);
    match assemble(&text, xlen) {
        Ok(again) if again.len() == 1 => Ok(again[0].word),
        Ok(again) => {
            let again: Vec<String> = again.iter().map(|w| format!("{:08x}", w.word)).collect();
            Err(format!(
                "{w:08x} disassembles to '{text}', which assembles to [{}]",
                again.join(" ")
            ))
        }
        Err(e) => Err(format!("{w:08x} disassembles to '{text}': {e}")),
    }
}

#[test]
fn roundtrip_disasm() {
    check_disasm(include_str!("golden/encode.golden"), CLANG_WORDS, RV32);
}

#[test]
fn roundtrip_disasm_on_rv64() {
    check_disasm(include_str!("golden/rv64/encode.golden"), CLANG_WORDS64, RV64);
}

fn check_disasm(golden: &'static str, clang: &[(&'static str, &[u32])], xlen: Xlen) {
    let mut rep = Report::new("encode(disasm(decode(w))) == w");
    // Not in the golden files, and the same on both widths. Words from clang
    // (Xcal written as `.insn i 0x0b, ...`).
    let extra = [
        ("xcal.dbg a0", vec![0x0005_000b]),
        ("xcal.halt zero", vec![0x0000_100b]),
        ("csrrw a0, satp, a0", vec![0x1805_1573]),
        ("csrrs a0, 0x7c0, zero", vec![0x7c00_2573]),
        ("fence rw, w", vec![0x0310_000f]),
        ("fence.tso", vec![0x8330_000f]),
        ("unimp", vec![0xc000_1073]),
    ];
    let clang = clang.iter().map(|&(src, words)| (src, words.to_vec()));
    for (src, words) in golden_words(golden).into_iter().chain(extra).chain(clang) {
        rep.total += 1;
        // Words are compared, not decoded `Inst`s: `decode` ignores some bits
        // (fence's pred/succ, for one) that must still come out right.
        for w in words {
            match reassemble(w, xlen) {
                Ok(again) if again == w => {}
                Ok(again) => {
                    rep.fails
                        .push(format!("  {src:<28} {w:08x} assembles back to {again:08x}"));
                    break;
                }
                Err(why) => {
                    rep.fails.push(format!("  {src:<28} {why}"));
                    break;
                }
            }
        }
    }
    rep.finish();
}

/// The same round trip on a large pseudo-random sample of instruction words:
/// every word the decoder accepts must disassemble to text that assembles to
/// an instruction decoding the same way, and to the very same word unless it
/// is a fence (whose reserved fields the decoder ignores). On RV64 too, where
/// more words are instructions (OP-32, OP-IMM-32, ld, 6-bit shifts...).
#[test]
fn roundtrip_random_words() {
    check_random_words(RV32);
}

#[test]
fn roundtrip_random_words_on_rv64() {
    check_random_words(RV64);
}

fn check_random_words(xlen: Xlen) {
    let mut rep = Report::new("random words");
    let mut x: u32 = 0x1234_5678;
    let mut valid = 0;
    for _ in 0..400_000 {
        // xorshift32
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        // Force a 32-bit instruction, and pick the major opcode from the low
        // bits so every format gets a fair share.
        let w = x | 3;
        let inst = decode(w, xlen);
        if matches!(inst, Inst::Illegal(_)) {
            continue;
        }
        valid += 1;
        rep.total += 1;
        match reassemble(w, xlen) {
            Ok(again) if again == w => {}
            Ok(again) if w & 0x7f == 0b0001111 && decode(again, xlen) == inst => {}
            Ok(again) => rep.fails.push(format!(
                "  {w:08x} ({}) assembles back to {again:08x}",
                disasm(inst, xlen)
            )),
            Err(why) => rep.fails.push(format!("  {why}")),
        }
        if rep.fails.len() > 20 {
            break;
        }
    }
    assert!(valid > 20_000, "only {valid} valid words in the sample");
    rep.finish();
}
