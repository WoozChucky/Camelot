//! Backtraces by frame pointer, and the prologue reading they rest on.
//!
//! ## The frame layout
//!
//! merlin and clang (without `-fomit-frame-pointer`) give every function that calls
//! another the same kind of frame: `s0` (the frame pointer, `fp`) holds the value `sp`
//! had on entry, the function's canonical frame address (CFA), and the prologue saves
//! `ra` and the caller's `s0` just below it, in doublewords saved with `sd` (and
//! restored with `ld`). merlin's prologue (see `crates/merlin/src/codegen.rs`, "Stack
//! frame"), as clang lays it out too:
//!
//! ```text
//!     addi  sp, sp, -16        sp = CFA - 16
//!     sd    ra, 8(sp)          ra        at CFA - 8
//!     sd    s0, 0(sp)          caller s0 at CFA - 16
//!     addi  s0, sp, 16         s0 = CFA
//! ```
//!
//! clang's is the same with the frame's whole size in the first `addi`. The slots move
//! in a variadic function, whose saved argument registers come first: merlin saves the
//! `8 - k` argument registers its named parameters leave free (they take `k` argument
//! words, at most 8) right below the CFA, and `ra` and `s0` below those:
//!
//! ```text
//!     ra        at CFA - 8 * (9 - k)
//!     caller s0 at CFA - 8 * (10 - k)
//! ```
//!
//! which with one named parameter, as kprintf, is `CFA - 64` and `CFA - 72`. So the
//! unwinder does not assume the slots
//! anywhere: it reads the function's prologue, from its symbol's address, and works
//! out where `sp` stood when each of `ra` and `s0` was stored (by a store as wide as a
//! register, `sd`), and what `s0` was set to ([`Prologue`]).
//! From one frame's `s0`, the next frame is then found in two steps, with the
//! prologue's offsets from the CFA (`fp`, `ra_slot` and `s0_slot` of [`Prologue`]):
//!
//! ```text
//!     CFA         = s0 - fp                   (fp is 0 for merlin and clang: s0 = CFA)
//!     caller's pc = xword(CFA + ra_slot)      (xword: a register's worth of memory,
//!     caller's s0 = xword(CFA + s0_slot)       8 bytes)
//! ```
//!
//! and so on up the chain of saved `s0`s, until a frame pointer is 0, misaligned (the
//! CFA is the `sp` of a call, 16-byte aligned by the psABI), not above
//! the one before, or not readable, or the pc is 0 (`_start` and the kernel's entry
//! leave `s0` at 0, the reset value).
//!
//! The innermost frame needs care: its function may not have built its frame yet (the
//! pc is in the prologue, before `addi s0`) or may have taken it down already (the pc
//! is at the `ret`, or right after the `ld s0` that restores the caller's), and a leaf
//! function may have no frame at all. In all three cases `ra` still holds the return
//! address and `s0` is still the caller's frame pointer, so the caller's pc is `ra`
//! and its frame pointer `s0`.
//!
//! Without frame pointers (`clang -O2 -fomit-frame-pointer`, hand-written assembly),
//! `s0` is an ordinary register: a function whose prologue sets no `s0` cannot be
//! unwound past, and the backtrace stops at it (after using `ra` if it is the
//! innermost frame). A trap handler's frame is not a call either: the backtrace of an
//! interrupted program ends at the handler, or goes on with whatever `s0` held.

use excalibur::{AluOp, Inst, LoadOp, StoreOp, Xlen};

/// Registers by number.
const RA: u8 = 1;
const SP: u8 = 2;
const S0: u8 = 8;

/// The longest prologue read, in instructions.
const PROLOGUE_MAX: u64 = 16;
/// The deepest backtrace.
pub const MAX_FRAMES: usize = 64;
/// A frame pointer's alignment: the CFA is `sp` at a call, which the psABI keeps
/// 16-byte aligned.
const FP_ALIGN: u64 = 16;

/// Where the machine's memory is read from: the stub, or a test's array. Instructions
/// are fetched a word at a time; saved registers are doublewords.
pub trait Memory {
    /// The instruction at `addr`.
    fn word(&mut self, addr: u64) -> Option<u32>;
    /// A saved register at `addr`: 8 bytes.
    fn xword(&mut self, addr: u64) -> Option<u64>;
}

impl Memory for crate::remote::Remote {
    fn word(&mut self, addr: u64) -> Option<u32> {
        self.read_word(addr)
    }

    fn xword(&mut self, addr: u64) -> Option<u64> {
        self.read_xword(addr)
    }
}

/// What a function's prologue does with its frame, as offsets from the CFA (the `sp`
/// on entry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prologue {
    /// The address of the `addi s0, sp, N` that sets up the frame pointer.
    pub fp_set_at: u64,
    /// `s0`'s value, as an offset from the CFA (0 for merlin and clang).
    pub fp: i64,
    /// Where `ra` and the caller's `s0` are saved.
    pub ra_slot: i64,
    pub s0_slot: i64,
}

/// The store that saves a whole register.
const SAVE_OP: StoreOp = StoreOp::Sd;

/// Reads the prologue of the function at `start`. `None` if it sets no
/// frame pointer within its first instructions, or saves `ra` or `s0` somewhere it
/// cannot follow.
pub fn prologue(mem: &mut impl Memory, start: u64) -> Option<Prologue> {
    // `sp` relative to the CFA as the prologue goes.
    let mut sp = 0i64;
    let (mut ra_slot, mut s0_slot) = (None, None);
    let save = SAVE_OP;
    for i in 0..PROLOGUE_MAX {
        let pc = start.wrapping_add(4 * i);
        match excalibur::decode(mem.word(pc)?, Xlen::Rv64) {
            Inst::OpImm {
                op: AluOp::Add,
                rd: SP,
                rs1: SP,
                imm,
            } => sp += i64::from(imm),
            Inst::Store {
                op,
                rs1: SP,
                rs2,
                imm,
            } if op == save => match rs2 {
                RA => ra_slot = Some(sp + i64::from(imm)),
                S0 => s0_slot = Some(sp + i64::from(imm)),
                _ => {}
            },
            Inst::OpImm {
                op: AluOp::Add,
                rd: S0,
                rs1: SP,
                imm,
            } => {
                return Some(Prologue {
                    fp_set_at: pc,
                    fp: sp + i64::from(imm),
                    ra_slot: ra_slot?,
                    s0_slot: s0_slot?,
                })
            }
            // A call, a branch or a return: past the prologue.
            Inst::Jal { .. } | Inst::Jalr { .. } | Inst::Branch { .. } => return None,
            _ => {}
        }
    }
    None
}

/// How far GDB's RISC-V prologue analysis looks, in bytes (`riscv_scan_prologue`'s
/// "arbitrary large number" when the line tables give no bound).
const SCAN_LIMIT: u64 = 100;

/// Where the function at `start` is past its prologue, by GDB's prologue analysis
/// (`riscv_scan_prologue` in GDB's `riscv-tdep.c`), for a function with no line
/// information: what `break FUNC` stops at then. The prologue goes on as long as
/// the instructions are those GDB counts as one's, at most 100 bytes:
///
/// - `addi` (any registers: `addi sp, sp, -N`, `addi s0, sp, N`, `mv`, `li`, `nop`),
///   and `addiw sp, sp, -N`;
/// - `sw` and `sd` to `sp` or `s0` (saving registers);
/// - `add` (any registers) and `addw s0, sp, zero`;
/// - `auipc` and `lui` (an address or a constant being built);
/// - `lw` and `ld` (any registers).
///
/// The first instruction of another kind is where the prologue ends: hello's
/// `puts`, `li t0, UART_BASE` (`lui`), `mv t1, a0`, then `lbu`, so `puts+8`. An
/// unreadable instruction gives `start`, as GDB keeps the pc it was given when the
/// analysis fails.
pub fn scan_prologue(mem: &mut impl Memory, start: u64) -> u64 {
    let mut pc = start;
    while pc.wrapping_sub(start) < SCAN_LIMIT {
        let Some(word) = mem.word(pc) else {
            return start;
        };
        // GDB's cases, in its order.
        let prologue = matches!(
            excalibur::decode(word, Xlen::Rv64),
            Inst::OpImm { op: AluOp::Add, .. }
                | Inst::OpImm32 {
                    op: AluOp::Add,
                    rd: SP,
                    rs1: SP,
                    ..
                }
                | Inst::Store {
                    op: StoreOp::Sw | StoreOp::Sd,
                    rs1: SP | S0,
                    ..
                }
                | Inst::Op { op: AluOp::Add, .. }
                | Inst::Op32 {
                    op: AluOp::Add,
                    rd: S0,
                    rs1: SP,
                    rs2: 0,
                }
                | Inst::Auipc { .. }
                | Inst::Lui { .. }
                | Inst::Load {
                    op: LoadOp::Lw | LoadOp::Ld,
                    ..
                }
        );
        if !prologue {
            return pc;
        }
        pc = pc.wrapping_add(4);
    }
    pc
}

/// One frame of a backtrace: where its function will go on (for the innermost one,
/// the pc), and its frame pointer, if known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub pc: u64,
    pub fp: u64,
}

/// The registers the unwinder starts from.
#[derive(Debug, Clone, Copy)]
pub struct Regs {
    pub pc: u64,
    pub ra: u64,
    pub fp: u64,
}

/// The frames of the selected hart's stack, innermost first. `function` gives the start
/// of the function holding an address (from the symbols).
pub fn backtrace(mem: &mut impl Memory, regs: Regs, function: impl Fn(u64) -> Option<u64>) -> Vec<Frame> {
    let decode = |w| excalibur::decode(w, Xlen::Rv64);
    let mut frames = vec![Frame {
        pc: regs.pc,
        fp: regs.fp,
    }];
    let prologue_of = |mem: &mut _, pc| function(pc).and_then(|start| prologue(mem, start));

    // The innermost frame: built, or not (yet, any more, at all).
    let pro = prologue_of(mem, regs.pc);
    let building = pro.is_none_or(|p| regs.pc <= p.fp_set_at);
    let leaving = matches!(
        mem.word(regs.pc).map(decode),
        Some(Inst::Jalr {
            rd: 0,
            rs1: RA,
            imm: 0
        })
    ) || matches!(
        mem.word(regs.pc.wrapping_sub(4)).map(decode),
        Some(Inst::Load { rd: S0, rs1: SP, .. })
    );
    let (mut pc, mut fp) = if building || leaving {
        (regs.ra, regs.fp)
    } else {
        match caller(mem, pro, regs.fp) {
            Some(next) => next,
            None => return frames,
        }
    };

    while frames.len() < MAX_FRAMES && pc != 0 {
        frames.push(Frame { pc, fp });
        if fp == 0 || !fp.is_multiple_of(FP_ALIGN) {
            break;
        }
        // A return address is just past the call: the call is in the caller's function.
        let pro = prologue_of(mem, pc.wrapping_sub(4));
        let Some((caller_pc, caller_fp)) = caller(mem, pro, fp) else {
            break;
        };
        if caller_fp != 0 && caller_fp <= fp {
            break;
        }
        (pc, fp) = (caller_pc, caller_fp);
    }
    frames
}

/// The caller's pc and frame pointer, from a frame whose function has this prologue
/// and whose `s0` is `fp`.
fn caller(mem: &mut impl Memory, pro: Option<Prologue>, fp: u64) -> Option<(u64, u64)> {
    let p = pro?;
    // The CFA first: s0 is `p.fp` bytes from it.
    let cfa = fp.wrapping_sub(p.fp as u64);
    // Then the saved registers, in their slots below the CFA.
    let slot = |off: i64| cfa.wrapping_add(off as u64);
    let ra = mem.xword(slot(p.ra_slot))?;
    let s0 = mem.xword(slot(p.s0_slot))?;
    Some((ra, s0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A fake memory: instruction words, and saved registers.
    #[derive(Default)]
    struct Words {
        code: HashMap<u64, u32>,
        data: HashMap<u64, u64>,
    }

    impl Memory for Words {
        fn word(&mut self, addr: u64) -> Option<u32> {
            self.code.get(&addr).copied()
        }

        fn xword(&mut self, addr: u64) -> Option<u64> {
            self.data.get(&addr).copied()
        }
    }

    fn asm(mem: &mut Words, at: u64, code: &[u32]) {
        for (i, &w) in code.iter().enumerate() {
            mem.code.insert(at + 4 * i as u64, w);
        }
    }

    // Encodings (from llvm-mc): merlin's prologue and epilogue pieces.
    const ADDI_SP_M16: u32 = 0xff010113; // addi sp, sp, -16
    const ADDI_S0_16: u32 = 0x01010413; // addi s0, sp, 16
    const ADDI_SP_M32: u32 = 0xfe010113; // addi sp, sp, -32
    const NOP: u32 = 0x00000013;
    const RET: u32 = 0x00008067;
    const ADDI_SP_16: u32 = 0x01010113; // addi sp, sp, 16

    /// A frame's code: the saves, the restore of `s0`, a variadic prologue and
    /// where each puts the slots.
    struct Width {
        save_ra: u32,
        save_s0: u32,
        restore_s0: u32,
        /// ra's and s0's slots in the plain prologue, from the CFA.
        slots: (i64, i64),
        variadic: [u32; 4],
        variadic_slots: (i64, i64),
    }

    const RV64: Width = Width {
        save_ra: 0x00113423,    // sd ra, 8(sp)
        save_s0: 0x00813023,    // sd s0, 0(sp)
        restore_s0: 0x00013403, // ld s0, 0(sp)
        slots: (-8, -16),
        // kprintf's: addi sp, sp, -80; sd ra, 16(sp); sd s0, 8(sp); addi s0, sp, 80
        variadic: [0xfb010113, 0x00113823, 0x00813423, 0x05010413],
        variadic_slots: (-64, -72),
    };

    fn reads_prologues(w: &Width) {
        let mut mem = Words::default();
        asm(
            &mut mem,
            0x100,
            &[ADDI_SP_M16, w.save_ra, w.save_s0, ADDI_S0_16, ADDI_SP_M32, NOP],
        );
        assert_eq!(
            prologue(&mut mem, 0x100),
            Some(Prologue {
                fp_set_at: 0x10c,
                fp: 0,
                ra_slot: w.slots.0,
                s0_slot: w.slots.1
            })
        );
        asm(&mut mem, 0x200, &w.variadic);
        assert_eq!(
            prologue(&mut mem, 0x200),
            Some(Prologue {
                fp_set_at: 0x20c,
                fp: 0,
                ra_slot: w.variadic_slots.0,
                s0_slot: w.variadic_slots.1
            })
        );
        // A leaf without a frame, and a function that saves no ra.
        asm(&mut mem, 0x300, &[NOP, RET]);
        assert_eq!(prologue(&mut mem, 0x300), None);
        asm(&mut mem, 0x400, &[ADDI_SP_M16, w.save_s0, ADDI_S0_16]);
        assert_eq!(prologue(&mut mem, 0x400), None);
        assert_eq!(prologue(&mut mem, 0x900), None, "unreadable");
    }

    #[test]
    fn reads_prologues_of_merlin_and_clang() {
        reads_prologues(&RV64);
    }

    /// `tests/prog/prog.c` (with `crt0.s`) built with merlin, merlin-as and merlin-ld,
    /// as `tests/session.rs` builds it: its loaded words, and its symbols.
    fn merlin_program() -> (Words, crate::symbols::Symbols) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/prog");
        let read = |name: &str| std::fs::read_to_string(format!("{dir}/{name}")).unwrap();
        let opts = merlin::Options::default();
        let asm = merlin::compile_with("prog.c", &read("prog.c"), &opts, &merlin::no_files)
            .unwrap_or_else(|e| panic!("merlin: {e:?}"));
        let inputs: Vec<merlin_ld::Input> = [("crt0.o", read("crt0.s")), ("prog.o", asm)]
            .into_iter()
            .map(|(name, asm)| merlin_ld::Input {
                name: name.into(),
                data: merlin_as::assemble_to_elf(&asm).unwrap_or_else(|e| panic!("merlin-as {name}: {e:?}")),
            })
            .collect();
        let linked = merlin_ld::link(&inputs, &merlin_ld::Options::default())
            .unwrap_or_else(|e| panic!("merlin-ld: {e:?}"));
        let file = elf::Elf::parse(&linked.elf).unwrap();
        let mut mem = Words::default();
        const PT_LOAD: u32 = 1;
        for seg in file.segments.iter().filter(|p| p.ty == PT_LOAD) {
            let (words, _) = file.segment_data(seg).unwrap().as_chunks::<4>();
            for (i, &w) in words.iter().enumerate() {
                mem.code.insert(seg.vaddr + 4 * i as u64, u32::from_le_bytes(w));
            }
        }
        (mem, crate::symbols::Symbols::from_elf(&linked.elf).unwrap())
    }

    /// The prologues above are merlin's own: prog.c's sum (variadic, one named
    /// parameter, as kprintf) and fact (not) start with exactly those words, and
    /// their slots are where the module doc's formula puts them (ra at
    /// CFA - 8*(9 - k), s0 at CFA - 8*(10 - k), k = 1).
    #[test]
    fn the_prologues_are_merlins() {
        let w = &RV64;
        let (mut mem, symbols) = merlin_program();
        let words =
            |mem: &mut Words, at: u64| -> Vec<u32> { (0..4).map(|i| mem.code[&(at + 4 * i)]).collect() };
        let sum = symbols.lookup("sum").unwrap();
        assert_eq!(words(&mut mem, sum), w.variadic, "sum");
        let reg = 8;
        assert_eq!(w.variadic_slots, (-reg * 8, -reg * 9));
        assert_eq!(
            prologue(&mut mem, sum),
            Some(Prologue {
                fp_set_at: sum + 12,
                fp: 0,
                ra_slot: w.variadic_slots.0,
                s0_slot: w.variadic_slots.1
            })
        );
        let fact = symbols.lookup("fact").unwrap();
        assert_eq!(
            words(&mut mem, fact),
            [ADDI_SP_M16, w.save_ra, w.save_s0, ADDI_S0_16],
            "fact"
        );
        assert_eq!(w.slots, (-reg, -reg * 2));
        assert_eq!(
            prologue(&mut mem, fact),
            Some(Prologue {
                fp_set_at: fact + 12,
                fp: 0,
                ra_slot: w.slots.0,
                s0_slot: w.slots.1
            })
        );
    }

    /// GDB's prologue analysis, for `break FUNC` on a function without lines.
    #[test]
    fn scans_prologues_as_gdb() {
        const LUI_T0: u32 = 0x100002b7; // lui t0, 0x10000 (li t0, UART_BASE)
        const MV_T1_A0: u32 = 0x00050313; // mv t1, a0
        const LBU_T2: u32 = 0x00034383; // lbu t2, 0x0(t1)
        const SW_RA_A0: u32 = 0x00152023; // sw ra, 0x0(a0): not to sp or s0
        let w = &RV64;
        let mut mem = Words::default();
        // hello's puts: li and mv are prologue to GDB, the lbu is not.
        asm(&mut mem, 0x100, &[LUI_T0, MV_T1_A0, LBU_T2]);
        assert_eq!(scan_prologue(&mut mem, 0x100), 0x108);
        // merlin's prologue, then a return.
        asm(
            &mut mem,
            0x200,
            &[ADDI_SP_M16, w.save_ra, w.save_s0, ADDI_S0_16, RET],
        );
        assert_eq!(scan_prologue(&mut mem, 0x200), 0x210);
        asm(&mut mem, 0x300, &[NOP, SW_RA_A0]);
        assert_eq!(scan_prologue(&mut mem, 0x300), 0x304);
        // At most 100 bytes; and nothing to read gives the start back.
        asm(&mut mem, 0x400, &[NOP; 30]);
        assert_eq!(scan_prologue(&mut mem, 0x400), 0x464);
        assert_eq!(scan_prologue(&mut mem, 0x900), 0x900);
    }

    /// A save must be as wide as a register: a `sw ra` keeps half of it.
    #[test]
    fn a_save_is_as_wide_as_a_register() {
        const SW_RA_12: u32 = 0x00112623; // sw ra, 12(sp)
        const SW_S0_8: u32 = 0x00812423; // sw s0, 8(sp)
        let mut mem = Words::default();
        asm(&mut mem, 0x100, &[ADDI_SP_M16, SW_RA_12, SW_S0_8, ADDI_S0_16]);
        assert_eq!(prologue(&mut mem, 0x100), None);
    }

    /// main (0x100) calls f (0x200, variadic) calls leaf (0x300, no frame), all of
    /// it at `base` (0, or the top of an RV64 address space).
    fn program(w: &Width, base: u64) -> Words {
        let mut mem = Words::default();
        asm(
            &mut mem,
            base + 0x100,
            &[ADDI_SP_M16, w.save_ra, w.save_s0, ADDI_S0_16, NOP, NOP, NOP],
        );
        asm(&mut mem, base + 0x200, &w.variadic);
        asm(
            &mut mem,
            base + 0x210,
            &[NOP, NOP, NOP, w.restore_s0, ADDI_SP_16, RET],
        );
        asm(&mut mem, base + 0x300, &[NOP, RET]);
        let slot = |cfa: u64, off: i64| (base + cfa).wrapping_add(off as u64);
        // main's frame: CFA 0x1000, called by _start with s0 = 0 and ra = 0x48.
        mem.data.insert(slot(0x1000, w.slots.0), base + 0x48);
        mem.data.insert(slot(0x1000, w.slots.1), 0);
        // f's frame: CFA 0xff0, called from main+16 (returns to 0x114).
        mem.data.insert(slot(0xff0, w.variadic_slots.0), base + 0x114);
        mem.data.insert(slot(0xff0, w.variadic_slots.1), base + 0x1000);
        mem
    }

    fn function(base: u64) -> impl Fn(u64) -> Option<u64> {
        move |pc| match pc.checked_sub(base)? {
            0x100..0x200 => Some(base + 0x100),
            0x200..0x300 => Some(base + 0x200),
            0x300..0x400 => Some(base + 0x300),
            _ => None,
        }
    }

    fn pcs(frames: &[Frame], base: u64) -> Vec<u64> {
        frames.iter().map(|f| f.pc - base).collect()
    }

    /// Where the programs are: low, and in the upper half (a higher-half kernel's
    /// addresses).
    fn cases() -> [(&'static Width, u64); 2] {
        [(&RV64, 0), (&RV64, 0xffff_ffff_8000_0000)]
    }

    #[test]
    fn walks_frame_pointers() {
        for (w, base) in cases() {
            let mut mem = program(w, base);
            let regs = |pc: u64, ra: u64, fp: u64| Regs {
                pc: base + pc,
                ra: base + ra,
                fp: base + fp,
            };
            // In f's body: f, main, _start.
            let f = backtrace(&mut mem, regs(0x214, 0xdead, 0xff0), function(base));
            assert_eq!(pcs(&f, base), [0x214, 0x114, 0x48], "{base:#x}");
            assert_eq!(f[1].fp, base + 0x1000);
            // In leaf, which has no frame: ra and s0 are f's.
            let f = backtrace(&mut mem, regs(0x304, 0x218, 0xff0), function(base));
            assert_eq!(pcs(&f, base), [0x304, 0x218, 0x114, 0x48], "{base:#x}");
        }
    }

    #[test]
    fn the_innermost_frame_may_be_half_built() {
        for (w, base) in cases() {
            let mut mem = program(w, base);
            // At f's first instruction, and at its addi s0: s0 is still main's. Right
            // after ld s0 (main's s0 back), and at ret.
            for pc in [0x200, 0x20c, 0x220, 0x224] {
                let regs = Regs {
                    pc: base + pc,
                    ra: base + 0x114,
                    fp: base + 0x1000,
                };
                let f = backtrace(&mut mem, regs, function(base));
                assert_eq!(pcs(&f, base), [pc, 0x114, 0x48], "{pc:#x} {base:#x}");
            }
        }
    }

    #[test]
    fn stops_where_the_chain_breaks() {
        for (w, base) in cases() {
            let regs = |pc: u64, ra: u64, fp: u64| Regs {
                pc: base + pc,
                ra: base + ra,
                fp: base + fp,
            };
            // A saved frame pointer that leads to memory that is not there.
            let mut mem = program(w, base);
            let slot = (base + 0xff0).wrapping_add(w.variadic_slots.1 as u64);
            mem.data.insert(slot, base + 0x800);
            let f = backtrace(&mut mem, regs(0x214, 0, 0xff0), function(base));
            assert_eq!(pcs(&f, base), [0x214, 0x114]);
            // Unknown functions (no symbols): only ra.
            let mut mem = program(w, base);
            let f = backtrace(&mut mem, regs(0x214, 0x114, 0xff0), |_| None);
            assert_eq!(pcs(&f, base), [0x214, 0x114]);
            // Misaligned frame pointers: no CFA is, at 16 bytes.
            for fp in [0xff1, 0xff8] {
                let f = backtrace(&mut mem, regs(0x304, 0x218, fp), function(base));
                assert_eq!(pcs(&f, base), [0x304, 0x218], "{fp:#x}");
            }
        }
    }
}
