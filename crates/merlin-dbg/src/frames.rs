//! Backtraces by frame pointer, and the prologue reading they rest on.
//!
//! ## The frame layout
//!
//! merlin and clang (without `-fomit-frame-pointer`) give every function that calls
//! another the same kind of frame: `s0` (the frame pointer, `fp`) holds the value `sp`
//! had on entry, the function's canonical frame address (CFA), and the prologue saves
//! `ra` and the caller's `s0` just below it. merlin's prologue (see
//! `crates/merlin/src/codegen.rs`, "Stack frame"):
//!
//! ```text
//!     addi  sp, sp, -16        sp = CFA - 16
//!     sw    ra, 12(sp)         ra        at CFA - 4
//!     sw    s0, 8(sp)          caller s0 at CFA - 8
//!     addi  s0, sp, 16         s0 = CFA
//! ```
//!
//! clang's is the same with the frame's whole size in the first `addi`. The slots move
//! in a variadic function, whose saved argument registers come first (merlin puts
//! `ra` at `CFA - 32` then), so the unwinder does not assume them: it reads the
//! function's prologue, from its symbol's address, and works out where `sp` stood when
//! each of `ra` and `s0` was stored, and what `s0` was set to ([`Prologue`]). From
//! one frame's `s0`, the next frame is then
//!
//! ```text
//!     caller's pc = the word at s0 - CFA offset of s0 + CFA offset of ra's slot
//!     caller's s0 = the word at s0 - CFA offset of s0 + CFA offset of s0's slot
//! ```
//!
//! and so on up the chain of saved `s0`s, until a frame pointer is 0, misaligned, not
//! above the one before, or not readable, or the pc is 0 (`_start` and the kernel's
//! entry leave `s0` at 0, the reset value).
//!
//! The innermost frame needs care: its function may not have built its frame yet (the
//! pc is in the prologue, before `addi s0`) or may have taken it down already (the pc
//! is at the `ret`, or right after the `lw s0` that restores the caller's), and a leaf
//! function may have no frame at all. In all three cases `ra` still holds the return
//! address and `s0` is still the caller's frame pointer, so the caller's pc is `ra`
//! and its frame pointer `s0`.
//!
//! Without frame pointers (`clang -O2 -fomit-frame-pointer`, hand-written assembly),
//! `s0` is an ordinary register: a function whose prologue sets no `s0` cannot be
//! unwound past, and the backtrace stops at it (after using `ra` if it is the
//! innermost frame). A trap handler's frame is not a call either: the backtrace of an
//! interrupted program ends at the handler, or goes on with whatever `s0` held.

use excalibur::{decode, AluOp, Inst, StoreOp};

/// Registers by number.
const RA: u8 = 1;
const SP: u8 = 2;
const S0: u8 = 8;

/// The longest prologue read, in instructions.
const PROLOGUE_MAX: u32 = 16;
/// The deepest backtrace.
pub const MAX_FRAMES: usize = 64;

/// Where the machine's memory is read from: the stub, or a test's array.
pub trait Memory {
    fn word(&mut self, addr: u32) -> Option<u32>;
}

impl Memory for crate::remote::Remote {
    fn word(&mut self, addr: u32) -> Option<u32> {
        self.read_word(addr)
    }
}

/// What a function's prologue does with its frame, as offsets from the CFA (the `sp`
/// on entry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prologue {
    /// The address of the `addi s0, sp, N` that sets up the frame pointer.
    pub fp_set_at: u32,
    /// `s0`'s value, as an offset from the CFA (0 for merlin and clang).
    pub fp: i32,
    /// Where `ra` and the caller's `s0` are saved.
    pub ra_slot: i32,
    pub s0_slot: i32,
}

/// Reads the prologue of the function at `start`. `None` if it sets no frame pointer
/// within its first instructions, or saves `ra` or `s0` somewhere it cannot follow.
pub fn prologue(mem: &mut impl Memory, start: u32) -> Option<Prologue> {
    // `sp` relative to the CFA as the prologue goes.
    let mut sp = 0i32;
    let (mut ra_slot, mut s0_slot) = (None, None);
    for i in 0..PROLOGUE_MAX {
        let pc = start.wrapping_add(4 * i);
        match decode(mem.word(pc)?) {
            Inst::OpImm {
                op: AluOp::Add,
                rd: SP,
                rs1: SP,
                imm,
            } => sp += imm,
            Inst::Store {
                op: StoreOp::Sw,
                rs1: SP,
                rs2,
                imm,
            } => match rs2 {
                RA => ra_slot = Some(sp + imm),
                S0 => s0_slot = Some(sp + imm),
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
                    fp: sp + imm,
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

/// One frame of a backtrace: where its function will go on (for the innermost one,
/// the pc), and its frame pointer, if known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub pc: u32,
    pub fp: u32,
}

/// The registers the unwinder starts from.
#[derive(Debug, Clone, Copy)]
pub struct Regs {
    pub pc: u32,
    pub ra: u32,
    pub fp: u32,
}

/// The frames of the selected hart's stack, innermost first. `function` gives the start
/// of the function holding an address (from the symbols).
pub fn backtrace(mem: &mut impl Memory, regs: Regs, function: impl Fn(u32) -> Option<u32>) -> Vec<Frame> {
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
        if fp == 0 || !fp.is_multiple_of(4) {
            break;
        }
        // A return address is just past the call: the call is in the caller's function.
        let pro = prologue_of(mem, pc.wrapping_sub(4));
        let Some((next_pc, next_fp)) = caller(mem, pro, fp) else {
            break;
        };
        if next_fp != 0 && next_fp <= fp {
            break;
        }
        (pc, fp) = (next_pc, next_fp);
    }
    frames
}

/// The caller's pc and frame pointer, from a frame whose function has this prologue
/// and whose `s0` is `fp`.
fn caller(mem: &mut impl Memory, pro: Option<Prologue>, fp: u32) -> Option<(u32, u32)> {
    let p = pro?;
    let cfa = fp.wrapping_sub(p.fp as u32);
    let ra = mem.word(cfa.wrapping_add(p.ra_slot as u32))?;
    let s0 = mem.word(cfa.wrapping_add(p.s0_slot as u32))?;
    Some((ra, s0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Words of a fake memory.
    struct Words(HashMap<u32, u32>);

    impl Memory for Words {
        fn word(&mut self, addr: u32) -> Option<u32> {
            self.0.get(&addr).copied()
        }
    }

    fn asm(mem: &mut Words, at: u32, code: &[u32]) {
        for (i, &w) in code.iter().enumerate() {
            mem.0.insert(at + 4 * i as u32, w);
        }
    }

    // Encodings (from llvm-mc): merlin's prologue and epilogue pieces.
    const ADDI_SP_M16: u32 = 0xff010113; // addi sp, sp, -16
    const SW_RA_12: u32 = 0x00112623; // sw ra, 12(sp)
    const SW_S0_8: u32 = 0x00812423; // sw s0, 8(sp)
    const ADDI_S0_16: u32 = 0x01010413; // addi s0, sp, 16
    const ADDI_SP_M32: u32 = 0xfe010113; // addi sp, sp, -32
    const NOP: u32 = 0x00000013;
    const RET: u32 = 0x00008067;
    const LW_S0_8: u32 = 0x00812403; // lw s0, 8(sp)
    const ADDI_SP_16: u32 = 0x01010113; // addi sp, sp, 16
                                        // kprintf's: addi sp, sp, -48; sw ra, 16(sp); sw s0, 12(sp); addi s0, sp, 48
    const VARIADIC: [u32; 4] = [0xfd010113, 0x00112823, 0x00812623, 0x03010413];

    #[test]
    fn reads_prologues() {
        let mut mem = Words(HashMap::new());
        asm(
            &mut mem,
            0x100,
            &[ADDI_SP_M16, SW_RA_12, SW_S0_8, ADDI_S0_16, ADDI_SP_M32, NOP],
        );
        assert_eq!(
            prologue(&mut mem, 0x100),
            Some(Prologue {
                fp_set_at: 0x10c,
                fp: 0,
                ra_slot: -4,
                s0_slot: -8
            })
        );
        asm(&mut mem, 0x200, &VARIADIC);
        assert_eq!(
            prologue(&mut mem, 0x200),
            Some(Prologue {
                fp_set_at: 0x20c,
                fp: 0,
                ra_slot: -32,
                s0_slot: -36
            })
        );
        // A leaf without a frame, and a function that saves no ra.
        asm(&mut mem, 0x300, &[NOP, RET]);
        assert_eq!(prologue(&mut mem, 0x300), None);
        asm(&mut mem, 0x400, &[ADDI_SP_M16, SW_S0_8, ADDI_S0_16]);
        assert_eq!(prologue(&mut mem, 0x400), None);
        assert_eq!(prologue(&mut mem, 0x900), None, "unreadable");
    }

    /// main (0x100) calls f (0x200, variadic) calls leaf (0x300, no frame).
    fn program() -> Words {
        let mut mem = Words(HashMap::new());
        asm(
            &mut mem,
            0x100,
            &[ADDI_SP_M16, SW_RA_12, SW_S0_8, ADDI_S0_16, NOP, NOP, NOP],
        );
        asm(&mut mem, 0x200, &VARIADIC);
        asm(&mut mem, 0x210, &[NOP, NOP, NOP, LW_S0_8, ADDI_SP_16, RET]);
        asm(&mut mem, 0x300, &[NOP, RET]);
        // main's frame: CFA 0x1000, called by _start with s0 = 0 and ra = 0x48.
        mem.0.insert(0x1000 - 4, 0x48);
        mem.0.insert(0x1000 - 8, 0);
        // f's frame: CFA 0xff0, called from main+16 (returns to 0x114).
        mem.0.insert(0xff0 - 32, 0x114);
        mem.0.insert(0xff0 - 36, 0x1000);
        mem
    }

    fn function(pc: u32) -> Option<u32> {
        match pc {
            0x100..0x200 => Some(0x100),
            0x200..0x300 => Some(0x200),
            0x300..0x400 => Some(0x300),
            _ => None,
        }
    }

    fn pcs(frames: &[Frame]) -> Vec<u32> {
        frames.iter().map(|f| f.pc).collect()
    }

    #[test]
    fn walks_frame_pointers() {
        let mut mem = program();
        // In f's body: f, main, _start.
        let f = backtrace(
            &mut mem,
            Regs {
                pc: 0x214,
                ra: 0xdead,
                fp: 0xff0,
            },
            function,
        );
        assert_eq!(pcs(&f), [0x214, 0x114, 0x48]);
        assert_eq!(f[1].fp, 0x1000);
        // In leaf, which has no frame: ra and s0 are f's.
        let f = backtrace(
            &mut mem,
            Regs {
                pc: 0x304,
                ra: 0x218,
                fp: 0xff0,
            },
            function,
        );
        assert_eq!(pcs(&f), [0x304, 0x218, 0x114, 0x48]);
    }

    #[test]
    fn the_innermost_frame_may_be_half_built() {
        let mut mem = program();
        // At f's first instruction, and at its addi s0: s0 is still main's.
        for pc in [0x200, 0x20c] {
            let f = backtrace(
                &mut mem,
                Regs {
                    pc,
                    ra: 0x114,
                    fp: 0x1000,
                },
                function,
            );
            assert_eq!(pcs(&f), [pc, 0x114, 0x48], "{pc:#x}");
        }
        // Right after lw s0 (main's s0 back), and at ret.
        for pc in [0x220, 0x224] {
            let f = backtrace(
                &mut mem,
                Regs {
                    pc,
                    ra: 0x114,
                    fp: 0x1000,
                },
                function,
            );
            assert_eq!(pcs(&f), [pc, 0x114, 0x48], "{pc:#x}");
        }
    }

    #[test]
    fn stops_where_the_chain_breaks() {
        let mut mem = program();
        // A saved frame pointer that leads to memory that is not there.
        mem.0.insert(0xff0 - 36, 0x800);
        let f = backtrace(
            &mut mem,
            Regs {
                pc: 0x214,
                ra: 0,
                fp: 0xff0,
            },
            function,
        );
        assert_eq!(pcs(&f), [0x214, 0x114]);
        // Unknown functions (no symbols): only ra.
        let mut mem = program();
        let f = backtrace(
            &mut mem,
            Regs {
                pc: 0x214,
                ra: 0x114,
                fp: 0xff0,
            },
            |_| None,
        );
        assert_eq!(pcs(&f), [0x214, 0x114]);
        // A misaligned frame pointer.
        let f = backtrace(
            &mut mem,
            Regs {
                pc: 0x304,
                ra: 0x218,
                fp: 0xff1,
            },
            function,
        );
        assert_eq!(pcs(&f), [0x304, 0x218]);
    }
}
