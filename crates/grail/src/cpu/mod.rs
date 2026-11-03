//! The harts: registers, the step loop, instruction execution and trap entry.

mod atomic;
mod csr;
mod mmu;
pub mod reservation;

pub use csr::{Csrs, MISA_VALUE};
pub use reservation::Reservation;

use crate::bus::Bus;
use crate::trap::{Exception, Trap, INTERRUPT_BIT};
use excalibur::csr::*;
use excalibur::{decode, AluOp, AmoWidth, BranchOp, CsrOp, CsrSrc, Inst, LoadOp, MulOp, StoreOp, Xlen};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Mode {
    U = 0,
    S = 1,
    M = 3,
}

impl Mode {
    /// Decodes a 2-bit privilege field. The reserved value 2 maps to U.
    pub fn from_bits(b: u32) -> Mode {
        match b & 3 {
            3 => Mode::M,
            1 => Mode::S,
            _ => Mode::U,
        }
    }
}

/// One hart: its registers, CSRs, privilege mode and `wfi` state. Its LR/SC
/// reservation is kept by the bus, which sees every store (see
/// [`Bus::reservation`]); memory and devices are shared with the other harts.
///
/// A hart runs against the bus it is handed: [`Hart::step`] and everything under it
/// take `bus: &mut Bus`, so several harts can take turns on one bus.
pub struct Hart {
    /// `mhartid`: the hart's index in its machine, from 0.
    pub id: u32,
    pub x: [u32; 32],
    pub pc: u32,
    pub mode: Mode,
    pub csr: Csrs,
    /// Stalled in `wfi` until an interrupt becomes pending.
    pub waiting: bool,
    /// Set when the current instruction wrote minstret, which suppresses its increment.
    instret_written: bool,
}

/// Every hart of a machine, and the bus they share. Grail runs the harts on one host
/// thread, deterministically: each [`Cpu::step`] ticks `mtime` once, samples the
/// devices' interrupt lines, then lets every hart in turn, from hart 0 up, run one
/// instruction (or take an interrupt, or wait in `wfi`). One step is one cycle of every
/// hart, so a hart's clock and the timebase stay the same number however many harts
/// there are (spec section 7).
///
/// Between steps, a `Cpu` dereferences to hart 0, so a one-hart machine reads like a
/// single CPU (`cpu.pc`, `cpu.x[10]`, `cpu.csr.mcause`); while a step runs (as the
/// trace callback sees it), to the hart whose turn it is. [`Cpu::hart`] and
/// [`Cpu::harts`] reach any hart.
pub struct Cpu {
    harts: Vec<Hart>,
    /// The hart `Deref` shows: hart 0 between steps, the hart whose turn it is during one.
    current: usize,
    pub bus: Bus,
    /// When every hart is stalled in `wfi`, jump `mtime` straight to the next
    /// `mtimecmp` instead of letting it tick (the default, full-speed mode). A
    /// real-time machine turns this off and lets the wait take wall-clock time instead
    /// (see [`Cpu::idle`]).
    pub wfi_skips_to_timer: bool,
    /// The hart whose turn comes next in the current step: 0 between steps. A step is
    /// normally run whole, but a debugger stops between two turns ([`Cpu::turn_traced`]),
    /// and the step then resumes where it stopped.
    turn: usize,
    /// Every hart that has had its turn in the current step waited in `wfi`.
    step_idle: bool,
}

/// Interrupts in priority order (highest first).
const IRQ_PRIORITY: [u32; 6] = [
    IRQ_M_EXT,
    IRQ_M_SOFT,
    IRQ_M_TIMER,
    IRQ_S_EXT,
    IRQ_S_SOFT,
    IRQ_S_TIMER,
];

impl std::ops::Deref for Cpu {
    type Target = Hart;

    fn deref(&self) -> &Hart {
        &self.harts[self.current]
    }
}

impl std::ops::DerefMut for Cpu {
    fn deref_mut(&mut self) -> &mut Hart {
        &mut self.harts[self.current]
    }
}

impl Cpu {
    /// The harts of a machine on `bus`, as many as the bus was built for
    /// ([`Bus::harts`]), at reset: in M-mode, hart 0 at the boot ROM's reset vector and
    /// the others at its park loop ([`crate::bus::PARK_PC`]), where they wait for an IPI.
    pub fn new(bus: Bus) -> Self {
        let harts = (0..bus.harts() as u32)
            .map(|id| {
                let pc = if id == 0 {
                    crate::bus::BOOT_ROM_BASE
                } else {
                    crate::bus::PARK_PC
                };
                Hart::new(id, pc)
            })
            .collect();
        Cpu {
            harts,
            current: 0,
            bus,
            wfi_skips_to_timer: true,
            turn: 0,
            step_idle: true,
        }
    }

    /// The hart whose turn comes next: 0 at the start of a step, more when a debugger
    /// stopped the machine in the middle of one.
    pub fn next_hart(&self) -> usize {
        self.turn
    }

    /// Every hart, hart `i` at index `i`.
    pub fn harts(&self) -> &[Hart] {
        &self.harts
    }

    /// Hart `id`. Panics if the machine has no such hart.
    pub fn hart(&self, id: usize) -> &Hart {
        &self.harts[id]
    }

    pub fn hart_mut(&mut self, id: usize) -> &mut Hart {
        &mut self.harts[id]
    }

    /// The current hart's `mip`, with the bits the devices drive (see [`Hart::mip`]).
    pub fn mip(&self) -> u32 {
        self.harts[self.current].mip(&self.bus)
    }

    /// The current hart's LR/SC reservation.
    pub fn reservation(&self) -> Reservation {
        self.bus.reservation(self.current)
    }

    /// Whether every hart is stalled in `wfi` with nothing pending that could wake it
    /// (an interrupt pending and enabled in `mie`). Only the timer or a device can
    /// then wake the machine.
    pub fn all_idle(&self) -> bool {
        self.harts
            .iter()
            .all(|h| h.waiting && h.mip(&self.bus) & h.csr.mie == 0)
    }

    /// While every hart is stalled in `wfi` with nothing pending, lets up to `max`
    /// ticks pass at once, exactly as `max` idle steps would: `mtime` advances and
    /// nothing else changes. When a timer can wake a hart (its `mie.MTIE` set), it stops
    /// one tick short of that hart's `mtimecmp`, so the next step raises the timer on
    /// time; a masked timer cannot end the wait (and nothing reads `mip` meanwhile), so
    /// it does not limit the skip. Returns the ticks skipped (0 when some hart is not
    /// stalled or has an interrupt pending).
    ///
    /// Device lines are not sampled during the skip, so host input that arrives in the
    /// meantime is seen at the next step: callers keep `max` small (a pacing batch).
    pub fn idle(&mut self, max: u64) -> u64 {
        if self.turn != 0 || !self.all_idle() {
            return 0;
        }
        let clint = &mut self.bus.clint;
        let mut ticks = max;
        for hart in &self.harts {
            if hart.csr.mie & (1 << IRQ_M_TIMER) != 0 {
                let to_timer = clint.mtimecmp[hart.id as usize]
                    .saturating_sub(clint.mtime)
                    .saturating_sub(1);
                ticks = ticks.min(to_timer);
            }
        }
        clint.mtime = clint.mtime.wrapping_add(ticks);
        ticks
    }

    /// Runs one step: every hart one instruction (or one interrupt taken, or one tick
    /// spent in `wfi`). Time advances by one tick per step.
    pub fn step(&mut self) {
        self.step_traced(&mut |_| {});
    }

    /// [`Cpu::step`], calling `trace` before each hart's turn, with the `Cpu`
    /// dereferencing to that hart (hart 0's call comes first, before the tick). A
    /// program that stops the machine (`xcal.halt`, the test device) ends the step
    /// there: the harts after it do not run. If a debugger stopped the machine in the
    /// middle of a step, this runs the rest of that step.
    pub fn step_traced(&mut self, trace: &mut impl FnMut(&Cpu)) {
        while !self.turn_traced(trace) {}
    }

    /// Runs the next hart's turn of the current step (hart 0's first, which starts the
    /// step with the tick), calling `trace` before it. Returns true if that turn ended
    /// the step: it was the last hart's, or the program stopped the machine. A step is
    /// the same whether it runs whole or a turn at a time, which is what lets a debugger
    /// stop between two harts' turns without changing what the machine does.
    #[inline]
    pub fn turn_traced(&mut self, trace: &mut impl FnMut(&Cpu)) -> bool {
        let i = self.turn;
        self.current = i;
        trace(self);
        if i == 0 {
            self.bus.tick();
            self.step_idle = true;
        }
        self.step_idle &= self.harts[i].step(&mut self.bus);
        self.current = 0;
        self.turn = i + 1;
        if self.turn < self.harts.len() && self.bus.exit.is_none() {
            return false;
        }
        self.turn = 0;
        if self.step_idle && self.wfi_skips_to_timer {
            // Nothing to do until a timer fires or a device raises a line: skip straight
            // to the next timer. Device lines are sampled again next step.
            if let Some(deadline) = self.bus.clint.next_deadline() {
                self.bus.clint.mtime = deadline;
            }
        }
        true
    }
}

impl Hart {
    /// A hart at reset: M-mode, every register 0, about to fetch from `pc`.
    pub fn new(id: u32, pc: u32) -> Self {
        Hart {
            id,
            x: [0; 32],
            pc,
            mode: Mode::M,
            csr: Csrs::default(),
            waiting: false,
            instret_written: false,
        }
    }

    /// Whether the hart's next turn executes the instruction at `pc`, rather than taking
    /// an interrupt or waiting in `wfi`: when a debugger's breakpoint at `pc` is hit.
    pub fn will_execute(&self, bus: &Bus) -> bool {
        self.pending_interrupt(bus).is_none() && (!self.waiting || self.mip(bus) & self.csr.mie != 0)
    }

    #[inline]
    fn reg(&self, r: u8) -> u32 {
        self.x[r as usize]
    }

    #[inline]
    fn set_reg(&mut self, r: u8, v: u32) {
        if r != 0 {
            self.x[r as usize] = v;
        }
    }

    /// The hart's turn in a step ([`Cpu::step`]): takes an interrupt, or runs one
    /// instruction, or waits in `wfi`. Returns true if it waited: stalled in `wfi` with
    /// no interrupt pending and enabled to wake it.
    pub fn step(&mut self, bus: &mut Bus) -> bool {
        if let Some(irq) = self.pending_interrupt(bus) {
            self.waiting = false;
            self.enter_trap(bus, INTERRUPT_BIT | irq, 0, self.pc);
            return false;
        }

        if self.waiting {
            if self.mip(bus) & self.csr.mie != 0 {
                self.waiting = false;
            } else {
                return true;
            }
        }

        let pc = self.pc;
        self.instret_written = false;
        let result = self.fetch(bus, pc).and_then(|word| {
            self.execute(bus, word, pc).map_err(|mut t| {
                if t.cause == Exception::IllegalInstr {
                    t.tval = word;
                }
                t
            })
        });
        match result {
            Ok(next) => {
                self.pc = next;
                if !self.instret_written && self.csr.mcountinhibit & 4 == 0 {
                    self.csr.minstret = self.csr.minstret.wrapping_add(1);
                }
            }
            Err(t) => self.enter_trap(bus, t.cause as u32, t.tval, pc),
        }
        if self.csr.mcountinhibit & 1 == 0 {
            self.csr.mcycle = self.csr.mcycle.wrapping_add(1);
        }
        false
    }

    /// Highest-priority interrupt that is pending, enabled, and allowed in the current mode.
    fn pending_interrupt(&self, bus: &Bus) -> Option<u32> {
        let pending = self.mip(bus) & self.csr.mie;
        if pending == 0 {
            return None;
        }
        let status = self.csr.mstatus;
        let m_enabled = self.mode < Mode::M || status & MSTATUS_MIE != 0;
        let s_enabled = self.mode < Mode::S || (self.mode == Mode::S && status & MSTATUS_SIE != 0);
        let m_pending = pending & !self.csr.mideleg;
        let s_pending = pending & self.csr.mideleg;
        IRQ_PRIORITY.into_iter().find(|&irq| {
            let bit = 1 << irq;
            (m_enabled && m_pending & bit != 0) || (s_enabled && s_pending & bit != 0)
        })
    }

    /// Trap entry (spec section 6). `cause` includes the interrupt bit.
    fn enter_trap(&mut self, bus: &mut Bus, cause: u32, tval: u32, epc: u32) {
        let interrupt = cause & INTERRUPT_BIT != 0;
        let code = cause & !INTERRUPT_BIT;
        let deleg = if interrupt {
            self.csr.mideleg
        } else {
            self.csr.medeleg
        };
        let to_s = self.mode <= Mode::S && (deleg >> code) & 1 != 0;
        // A trap breaks any LR/SC sequence: the handler may run other code (or
        // switch threads) before the interrupted `sc.w` executes.
        bus.reservation_mut(self.id as usize).clear();
        let s = &mut self.csr.mstatus;

        let tvec = if to_s {
            self.csr.sepc = epc;
            self.csr.scause = cause;
            self.csr.stval = tval;
            let sie = *s & MSTATUS_SIE != 0;
            *s &= !(MSTATUS_SPIE | MSTATUS_SIE | MSTATUS_SPP);
            if sie {
                *s |= MSTATUS_SPIE;
            }
            if self.mode == Mode::S {
                *s |= MSTATUS_SPP;
            }
            self.mode = Mode::S;
            self.csr.stvec
        } else {
            self.csr.mepc = epc;
            self.csr.mcause = cause;
            self.csr.mtval = tval;
            let mie = *s & MSTATUS_MIE != 0;
            *s &= !(MSTATUS_MPIE | MSTATUS_MIE | MSTATUS_MPP);
            if mie {
                *s |= MSTATUS_MPIE;
            }
            *s |= (self.mode as u32) << MSTATUS_MPP_SHIFT;
            self.mode = Mode::M;
            self.csr.mtvec
        };

        let base = tvec & !3;
        self.pc = if tvec & 3 == 1 && interrupt {
            base.wrapping_add(4 * code)
        } else {
            base
        };
    }

    fn illegal() -> Trap {
        Trap::new(Exception::IllegalInstr, 0)
    }

    /// Checks a control-transfer target. Without the C extension it must be 4-byte aligned.
    fn jump(target: u32) -> Result<u32, Trap> {
        if target & 3 != 0 {
            Err(Trap::new(Exception::InstrMisaligned, target))
        } else {
            Ok(target)
        }
    }

    /// Executes one instruction and returns the next pc.
    fn execute(&mut self, bus: &mut Bus, word: u32, pc: u32) -> Result<u32, Trap> {
        let next = pc.wrapping_add(4);
        // Grail's harts are RV32 until #145 gives them 64-bit registers.
        match decode(word, Xlen::Rv32) {
            Inst::Lui { rd, imm } => self.set_reg(rd, imm as u32),
            Inst::Auipc { rd, imm } => self.set_reg(rd, pc.wrapping_add(imm as u32)),
            Inst::Jal { rd, imm } => {
                let target = Self::jump(pc.wrapping_add(imm as u32))?;
                self.set_reg(rd, next);
                return Ok(target);
            }
            Inst::Jalr { rd, rs1, imm } => {
                let target = Self::jump(self.reg(rs1).wrapping_add(imm as u32) & !1)?;
                self.set_reg(rd, next);
                return Ok(target);
            }
            Inst::Branch { op, rs1, rs2, imm } => {
                let (a, b) = (self.reg(rs1), self.reg(rs2));
                let taken = match op {
                    BranchOp::Beq => a == b,
                    BranchOp::Bne => a != b,
                    BranchOp::Blt => (a as i32) < (b as i32),
                    BranchOp::Bge => (a as i32) >= (b as i32),
                    BranchOp::Bltu => a < b,
                    BranchOp::Bgeu => a >= b,
                };
                if taken {
                    return Self::jump(pc.wrapping_add(imm as u32));
                }
            }
            Inst::Load { op, rd, rs1, imm } => {
                let addr = self.reg(rs1).wrapping_add(imm as u32);
                let v = self.load(bus, addr, op.size())?;
                let v = match op {
                    LoadOp::Lb => v as i8 as i32 as u32,
                    LoadOp::Lh => v as i16 as i32 as u32,
                    LoadOp::Lw | LoadOp::Lbu | LoadOp::Lhu => v,
                    LoadOp::Lwu | LoadOp::Ld => unreachable!("RV64 only: an RV32 hart decodes it as illegal"),
                };
                self.set_reg(rd, v);
            }
            Inst::Store { op, rs1, rs2, imm } => {
                let addr = self.reg(rs1).wrapping_add(imm as u32);
                let size = op.size();
                let v = match op {
                    StoreOp::Sb => self.reg(rs2) & 0xff,
                    StoreOp::Sh => self.reg(rs2) & 0xffff,
                    StoreOp::Sw => self.reg(rs2),
                    StoreOp::Sd => unreachable!("RV64 only: an RV32 hart decodes it as illegal"),
                };
                self.store(bus, addr, size, v)?;
            }
            Inst::OpImm { op, rd, rs1, imm } => {
                let v = alu(op, self.reg(rs1), imm as u32);
                self.set_reg(rd, v);
            }
            Inst::Op { op, rd, rs1, rs2 } => {
                let v = alu(op, self.reg(rs1), self.reg(rs2));
                self.set_reg(rd, v);
            }
            Inst::Mul { op, rd, rs1, rs2 } => {
                let v = muldiv(op, self.reg(rs1), self.reg(rs2));
                self.set_reg(rd, v);
            }
            // One instruction at a time, in order: aq/rl need no work (see atomic.rs).
            Inst::Lr {
                width: AmoWidth::W,
                rd,
                rs1,
                ..
            } => {
                let v = self.load_reserved(bus, self.reg(rs1))?;
                self.set_reg(rd, v);
            }
            Inst::Sc {
                width: AmoWidth::W,
                rd,
                rs1,
                rs2,
                ..
            } => {
                let v = self.store_conditional(bus, self.reg(rs1), self.reg(rs2))?;
                self.set_reg(rd, v);
            }
            Inst::Amo {
                width: AmoWidth::W,
                op,
                rd,
                rs1,
                rs2,
                ..
            } => {
                let old = self.amo(bus, op, self.reg(rs1), self.reg(rs2))?;
                self.set_reg(rd, old);
            }
            // Harts take turns one instruction at a time on one memory, with no caches and no
            // instruction cache: every hart sees every access in order, so fences are no-ops.
            Inst::Fence { .. } | Inst::FenceTso | Inst::FenceI => {}
            Inst::Ecall => {
                let cause = match self.mode {
                    Mode::U => Exception::EcallU,
                    Mode::S => Exception::EcallS,
                    Mode::M => Exception::EcallM,
                };
                return Err(Trap::new(cause, 0));
            }
            Inst::Ebreak => return Err(Trap::new(Exception::Breakpoint, pc)),
            Inst::Mret => {
                if self.mode != Mode::M {
                    return Err(Self::illegal());
                }
                let s = &mut self.csr.mstatus;
                let mpp = Mode::from_bits((*s & MSTATUS_MPP) >> MSTATUS_MPP_SHIFT);
                let mpie = *s & MSTATUS_MPIE != 0;
                *s &= !(MSTATUS_MIE | MSTATUS_MPP);
                *s |= MSTATUS_MPIE;
                if mpie {
                    *s |= MSTATUS_MIE;
                }
                if mpp != Mode::M {
                    *s &= !MSTATUS_MPRV;
                }
                self.mode = mpp;
                return Ok(self.csr.mepc);
            }
            Inst::Sret => {
                let tsr = self.csr.mstatus & MSTATUS_TSR != 0;
                if self.mode == Mode::U || (self.mode == Mode::S && tsr) {
                    return Err(Self::illegal());
                }
                let s = &mut self.csr.mstatus;
                let spp = if *s & MSTATUS_SPP != 0 { Mode::S } else { Mode::U };
                let spie = *s & MSTATUS_SPIE != 0;
                *s &= !(MSTATUS_SIE | MSTATUS_SPP | MSTATUS_MPRV);
                *s |= MSTATUS_SPIE;
                if spie {
                    *s |= MSTATUS_SIE;
                }
                self.mode = spp;
                return Ok(self.csr.sepc);
            }
            Inst::Wfi => {
                let tw = self.csr.mstatus & MSTATUS_TW != 0;
                if self.mode == Mode::U || (self.mode == Mode::S && tw) {
                    return Err(Self::illegal());
                }
                self.waiting = true;
            }
            Inst::SfenceVma { .. } => {
                let tvm = self.csr.mstatus & MSTATUS_TVM != 0;
                if self.mode == Mode::U || (self.mode == Mode::S && tvm) {
                    return Err(Self::illegal());
                }
            }
            Inst::Csr { op, rd, src, csr } => {
                let (operand, writes) = match (op, src) {
                    (CsrOp::Rw, CsrSrc::Reg(r)) => (self.reg(r), true),
                    (CsrOp::Rw, CsrSrc::Imm(i)) => (i as u32, true),
                    // csrrs/csrrc with x0 or a zero immediate only read.
                    (_, CsrSrc::Reg(r)) => (self.reg(r), r != 0),
                    (_, CsrSrc::Imm(i)) => (i as u32, i != 0),
                };
                let old = self.csr_read(bus, csr)?;
                if writes {
                    let base = self.csr_rmw_base(csr, old);
                    let new = match op {
                        CsrOp::Rw => operand,
                        CsrOp::Rs => base | operand,
                        CsrOp::Rc => base & !operand,
                    };
                    self.csr_write(csr, new)?;
                }
                self.set_reg(rd, old);
            }
            Inst::XcalDbg { rs1 } => {
                let v = self.reg(rs1);
                let name = excalibur::reg::abi_name(rs1);
                let line = format!("[dbg] pc={pc:08x} {name} = {v:#010x} ({})\n", v as i32);
                bus.debug_out(line.as_bytes());
            }
            Inst::XcalHalt { rs1 } => {
                if self.mode != Mode::M {
                    return Err(Self::illegal());
                }
                bus.exit = Some(crate::bus::Exit::Code(self.reg(rs1) & 0xff));
            }
            // RV64 only: an RV32 hart decodes them as illegal. #145 runs them.
            Inst::OpImm32 { .. }
            | Inst::Op32 { .. }
            | Inst::Mul32 { .. }
            | Inst::Lr {
                width: AmoWidth::D, ..
            }
            | Inst::Sc {
                width: AmoWidth::D, ..
            }
            | Inst::Amo {
                width: AmoWidth::D, ..
            } => {
                unreachable!("RV64 only: an RV32 hart decodes it as illegal")
            }
            Inst::Illegal(_) => return Err(Self::illegal()),
        }
        Ok(next)
    }
}

fn alu(op: AluOp, a: u32, b: u32) -> u32 {
    match op {
        AluOp::Add => a.wrapping_add(b),
        AluOp::Sub => a.wrapping_sub(b),
        AluOp::Sll => a << (b & 31),
        AluOp::Slt => ((a as i32) < (b as i32)) as u32,
        AluOp::Sltu => (a < b) as u32,
        AluOp::Xor => a ^ b,
        AluOp::Srl => a >> (b & 31),
        AluOp::Sra => ((a as i32) >> (b & 31)) as u32,
        AluOp::Or => a | b,
        AluOp::And => a & b,
    }
}

/// M extension. Division by zero and overflow follow the spec: no traps.
fn muldiv(op: MulOp, a: u32, b: u32) -> u32 {
    let (sa, sb) = (a as i32, b as i32);
    match op {
        MulOp::Mul => a.wrapping_mul(b),
        MulOp::Mulh => ((sa as i64 * sb as i64) >> 32) as u32,
        MulOp::Mulhsu => ((sa as i64 * b as i64) >> 32) as u32,
        MulOp::Mulhu => ((a as u64 * b as u64) >> 32) as u32,
        MulOp::Div => match sb {
            0 => u32::MAX,
            -1 => sa.wrapping_neg() as u32, // i32::MIN / -1 = i32::MIN
            _ => (sa / sb) as u32,
        },
        MulOp::Divu => a.checked_div(b).unwrap_or(u32::MAX),
        MulOp::Rem => match sb {
            0 => a,
            -1 => 0,
            _ => (sa % sb) as u32,
        },
        MulOp::Remu => a.checked_rem(b).unwrap_or(a),
    }
}
