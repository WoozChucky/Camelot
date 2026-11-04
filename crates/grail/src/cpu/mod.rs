//! The harts: registers, the step loop, instruction execution and trap entry.
//!
//! A hart is RV64 or RV32, as its machine is ([`crate::MachineConfig::xlen`]), and both
//! keep their registers in 64 bits. An RV32 hart holds each register's 32-bit value
//! sign-extended from bit 31: every result is computed on 32 bits and sign-extended
//! ([`Hart::set_reg`]), the rule RV64's W instructions follow, and every effective
//! address (a load's or store's, a jump target, the pc) is cut to its low 32 bits
//! ([`Hart::addr`]). With registers in that form, a comparison on all 64 bits (`blt`,
//! `bltu`, `slt`, `sltu`) gives the answer the 32-bit one would, signed or unsigned, so
//! most instructions run the same code on both widths.

mod atomic;
mod csr;
mod mmu;
pub mod reservation;

pub use csr::{misa, Csrs};
pub use reservation::Reservation;

use crate::bus::Bus;
use crate::trap::{interrupt_bit, Exception, Trap};
use excalibur::csr::*;
use excalibur::{decode, AluOp, BranchOp, CsrOp, CsrSrc, Inst, LoadOp, MulOp, Xlen};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Mode {
    U = 0,
    S = 1,
    M = 3,
}

impl Mode {
    /// Decodes a 2-bit privilege field. The reserved value 2 maps to U.
    pub fn from_bits(b: u64) -> Mode {
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
    /// The registers. An RV32 hart's hold their 32-bit values sign-extended to 64 bits
    /// (see the module documentation): write them with [`Hart::set_reg`].
    pub x: [u64; 32],
    /// The address of the instruction the hart executes next: below 4 GiB on an RV32
    /// hart.
    pub pc: u64,
    pub mode: Mode,
    pub csr: Csrs,
    /// Stalled in `wfi` until an interrupt becomes pending.
    pub waiting: bool,
    /// Set when the current instruction wrote minstret, which suppresses its increment.
    instret_written: bool,
    /// The register width: what the hart's instruction words mean, and how wide its
    /// registers, addresses and CSRs are.
    xlen: Xlen,
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
    /// ([`Bus::harts`]) and as wide ([`Bus::xlen`]), at reset: in M-mode, hart 0 at the
    /// boot ROM's reset vector and the others at its park loop
    /// ([`crate::bus::PARK_PC`]), where they wait for an IPI.
    pub fn new(bus: Bus) -> Self {
        let harts = (0..bus.harts() as u32)
            .map(|id| {
                let pc = if id == 0 {
                    crate::bus::BOOT_ROM_BASE
                } else {
                    crate::bus::PARK_PC
                };
                Hart::new(id, pc, bus.xlen())
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

    /// Makes every hart, and the bus, `xlen` wide: for a machine at reset, before it
    /// runs (loading an ELF64 program selects RV64, [`crate::Machine::load_elf`]). The
    /// harts keep their registers and CSRs, which reset sets alike on both widths.
    pub fn set_xlen(&mut self, xlen: Xlen) {
        self.bus.set_xlen(xlen);
        for hart in &mut self.harts {
            hart.xlen = xlen;
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
    pub fn mip(&self) -> u64 {
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
    /// it does not limit the skip. A device that keeps time (the framebuffer's vertical
    /// blank, with its interrupt enabled: [`Bus::device_deadline`]) ends it the same way.
    /// Returns the ticks skipped (0 when some hart is not stalled or has an interrupt
    /// pending).
    ///
    /// Device lines are not sampled during the skip, so host input that arrives in the
    /// meantime is seen at the next step: callers keep `max` small (a pacing batch).
    pub fn idle(&mut self, max: u64) -> u64 {
        if self.turn != 0 || !self.all_idle() {
            return 0;
        }
        let clint = &self.bus.clint;
        let mut ticks = max.min(self.bus.device_deadline().unwrap_or(u64::MAX));
        for hart in &self.harts {
            if hart.csr.mie & (1 << IRQ_M_TIMER) != 0 {
                let to_timer = clint.mtimecmp[hart.id as usize]
                    .saturating_sub(clint.mtime)
                    .saturating_sub(1);
                ticks = ticks.min(to_timer);
            }
        }
        self.bus.skip_time(ticks);
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
            // to the next timer, or to just before the next vertical blank that can
            // interrupt. Device lines are sampled again next step.
            let mtime = self.bus.clint.mtime;
            let to_timer = self.bus.clint.next_deadline().map(|d| d - mtime);
            if let Some(ticks) = [to_timer, self.bus.device_deadline()].into_iter().flatten().min() {
                self.bus.skip_time(ticks);
            }
        }
        true
    }
}

/// `v`'s low 32 bits, sign-extended to 64: a W instruction's result, and every RV32
/// register value.
#[inline]
fn sext32(v: u64) -> u64 {
    v as i32 as i64 as u64
}

/// An immediate as an operand: sign-extended to 64 bits.
#[inline]
fn imm64(imm: i32) -> u64 {
    imm as i64 as u64
}

impl Hart {
    /// A hart `xlen` wide at reset: M-mode, every register 0, about to fetch from `pc`.
    pub fn new(id: u32, pc: u64, xlen: Xlen) -> Self {
        Hart {
            id,
            x: [0; 32],
            pc,
            mode: Mode::M,
            csr: Csrs::default(),
            waiting: false,
            instret_written: false,
            xlen,
        }
    }

    /// The hart's register width.
    pub fn xlen(&self) -> Xlen {
        self.xlen
    }

    /// Whether the hart's next turn executes the instruction at `pc`, rather than taking
    /// an interrupt or waiting in `wfi`: when a debugger's breakpoint at `pc` is hit.
    pub fn will_execute(&self, bus: &Bus) -> bool {
        self.pending_interrupt(bus).is_none() && (!self.waiting || self.mip(bus) & self.csr.mie != 0)
    }

    #[inline]
    fn reg(&self, r: u8) -> u64 {
        self.x[r as usize]
    }

    /// Writes `v` to register `x<r>` as the hart holds it (writes to `x0` are ignored):
    /// all 64 bits on RV64; on RV32 its low 32 bits, sign-extended.
    #[inline]
    pub fn set_reg(&mut self, r: u8, v: u64) {
        if r != 0 {
            self.x[r as usize] = match self.xlen {
                Xlen::Rv64 => v,
                Xlen::Rv32 => sext32(v),
            };
        }
    }

    /// An address as the hart uses it: all 64 bits on RV64, the low 32 on RV32, where
    /// addresses wrap at 4 GiB.
    #[inline]
    pub fn addr(&self, v: u64) -> u64 {
        match self.xlen {
            Xlen::Rv64 => v,
            Xlen::Rv32 => v as u32 as u64,
        }
    }

    /// `rs1 + imm`: the address a load, a store or `jalr` uses.
    #[inline]
    fn ea(&self, rs1: u8, imm: i32) -> u64 {
        self.addr(self.reg(rs1).wrapping_add(imm64(imm)))
    }

    /// The hart's turn in a step ([`Cpu::step`]): takes an interrupt, or runs one
    /// instruction, or waits in `wfi`. Returns true if it waited: stalled in `wfi` with
    /// no interrupt pending and enabled to wake it.
    pub fn step(&mut self, bus: &mut Bus) -> bool {
        // `x` and `pc` are public (for a debugger, for tests): whoever writes them must keep
        // an RV32 hart's form, registers sign-extended and the pc below 4 GiB.
        debug_assert!(
            self.xlen == Xlen::Rv64 || (self.x.iter().all(|&v| v == sext32(v)) && self.pc >> 32 == 0),
            "hart {}: an RV32 register or pc out of its form: pc {:#x}, x {:x?}",
            self.id,
            self.pc,
            self.x
        );
        if let Some(irq) = self.pending_interrupt(bus) {
            self.waiting = false;
            self.enter_trap(bus, interrupt_bit(self.xlen) | irq as u64, 0, self.pc);
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
                    t.tval = word.into();
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
            Err(t) => self.enter_trap(bus, t.cause as u64, t.tval, pc),
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
            let bit = 1u64 << irq;
            (m_enabled && m_pending & bit != 0) || (s_enabled && s_pending & bit != 0)
        })
    }

    /// Trap entry (spec section 6). `cause` includes the interrupt bit, the top bit of
    /// the hart's width.
    fn enter_trap(&mut self, bus: &mut Bus, cause: u64, tval: u64, epc: u64) {
        let interrupt_bit = interrupt_bit(self.xlen);
        let interrupt = cause & interrupt_bit != 0;
        let code = cause & !interrupt_bit;
        let deleg = if interrupt {
            self.csr.mideleg
        } else {
            self.csr.medeleg
        };
        let to_s = self.mode <= Mode::S && (deleg >> code) & 1 != 0;
        // A trap breaks any LR/SC sequence: the handler may run other code (or
        // switch threads) before the interrupted `sc` executes.
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
            *s |= (self.mode as u64) << MSTATUS_MPP_SHIFT;
            self.mode = Mode::M;
            self.csr.mtvec
        };

        let base = tvec & !3;
        self.pc = if tvec & 3 == 1 && interrupt {
            self.addr(base.wrapping_add(4 * code))
        } else {
            base
        };
    }

    fn illegal() -> Trap {
        Trap::new(Exception::IllegalInstr, 0)
    }

    /// Checks a control-transfer target, cut to the hart's width. Without the C
    /// extension it must be 4-byte aligned.
    fn jump(&self, target: u64) -> Result<u64, Trap> {
        let target = self.addr(target);
        if target & 3 != 0 {
            Err(Trap::new(Exception::InstrMisaligned, target))
        } else {
            Ok(target)
        }
    }

    /// The ALU on the hart's width: all 64 bits on RV64; on RV32 the low 32 bits, as
    /// the W instructions compute.
    fn alu(&self, op: AluOp, a: u64, b: u64) -> u64 {
        match self.xlen {
            Xlen::Rv64 => alu64(op, a, b),
            Xlen::Rv32 => alu32(op, a, b),
        }
    }

    /// The M extension on the hart's width, like [`Hart::alu`].
    fn muldiv(&self, op: MulOp, a: u64, b: u64) -> u64 {
        match self.xlen {
            Xlen::Rv64 => muldiv64(op, a, b),
            Xlen::Rv32 => muldiv32(op, a, b),
        }
    }

    /// Executes one instruction and returns the next pc.
    fn execute(&mut self, bus: &mut Bus, word: u32, pc: u64) -> Result<u64, Trap> {
        let next = self.addr(pc.wrapping_add(4));
        match decode(word, self.xlen) {
            // U-type immediates are sign-extended from bit 31 (`lui` of 0x80000 gives
            // 0xffff_ffff_8000_0000 on RV64).
            Inst::Lui { rd, imm } => self.set_reg(rd, imm64(imm)),
            Inst::Auipc { rd, imm } => self.set_reg(rd, pc.wrapping_add(imm64(imm))),
            Inst::Jal { rd, imm } => {
                let target = self.jump(pc.wrapping_add(imm64(imm)))?;
                self.set_reg(rd, next);
                return Ok(target);
            }
            Inst::Jalr { rd, rs1, imm } => {
                let target = self.jump(self.ea(rs1, imm) & !1)?;
                self.set_reg(rd, next);
                return Ok(target);
            }
            Inst::Branch { op, rs1, rs2, imm } => {
                let (a, b) = (self.reg(rs1), self.reg(rs2));
                let taken = match op {
                    BranchOp::Beq => a == b,
                    BranchOp::Bne => a != b,
                    BranchOp::Blt => (a as i64) < (b as i64),
                    BranchOp::Bge => (a as i64) >= (b as i64),
                    BranchOp::Bltu => a < b,
                    BranchOp::Bgeu => a >= b,
                };
                if taken {
                    return self.jump(pc.wrapping_add(imm64(imm)));
                }
            }
            Inst::Load { op, rd, rs1, imm } => {
                let v = self.load(bus, self.ea(rs1, imm), op.size())?;
                let v = match op {
                    LoadOp::Lb => v as i8 as i64 as u64,
                    LoadOp::Lh => v as i16 as i64 as u64,
                    LoadOp::Lw => sext32(v),
                    LoadOp::Lbu | LoadOp::Lhu | LoadOp::Lwu | LoadOp::Ld => v,
                };
                self.set_reg(rd, v);
            }
            Inst::Store { op, rs1, rs2, imm } => {
                // `store` writes the register's low `size` bytes.
                self.store(bus, self.ea(rs1, imm), op.size(), self.reg(rs2))?;
            }
            Inst::OpImm { op, rd, rs1, imm } => {
                let v = self.alu(op, self.reg(rs1), imm64(imm));
                self.set_reg(rd, v);
            }
            Inst::Op { op, rd, rs1, rs2 } => {
                let v = self.alu(op, self.reg(rs1), self.reg(rs2));
                self.set_reg(rd, v);
            }
            Inst::Mul { op, rd, rs1, rs2 } => {
                let v = self.muldiv(op, self.reg(rs1), self.reg(rs2));
                self.set_reg(rd, v);
            }
            // RV64's W forms (RV32 decodes them as illegal): 32-bit operations whose
            // result is sign-extended, what every RV32 operation is.
            Inst::OpImm32 { op, rd, rs1, imm } => {
                let v = alu32(op, self.reg(rs1), imm64(imm));
                self.set_reg(rd, v);
            }
            Inst::Op32 { op, rd, rs1, rs2 } => {
                let v = alu32(op, self.reg(rs1), self.reg(rs2));
                self.set_reg(rd, v);
            }
            Inst::Mul32 { op, rd, rs1, rs2 } => {
                let v = muldiv32(op, self.reg(rs1), self.reg(rs2));
                self.set_reg(rd, v);
            }
            // One instruction at a time, in order: aq/rl need no work (see atomic.rs).
            // The `.d` forms are RV64 only: RV32 decodes them as illegal.
            Inst::Lr { width, rd, rs1, .. } => {
                let v = self.load_reserved(bus, self.reg(rs1), width.size())?;
                self.set_reg(rd, v);
            }
            Inst::Sc {
                width, rd, rs1, rs2, ..
            } => {
                let v = self.store_conditional(bus, self.reg(rs1), width.size(), self.reg(rs2))?;
                self.set_reg(rd, v);
            }
            Inst::Amo {
                width,
                op,
                rd,
                rs1,
                rs2,
                ..
            } => {
                let old = self.amo(bus, op, self.reg(rs1), width.size(), self.reg(rs2))?;
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
                    (CsrOp::Rw, CsrSrc::Imm(i)) => (i as u64, true),
                    // csrrs/csrrc with x0 or a zero immediate only read.
                    (_, CsrSrc::Reg(r)) => (self.reg(r), r != 0),
                    (_, CsrSrc::Imm(i)) => (i as u64, i != 0),
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
                let line = match self.xlen {
                    Xlen::Rv64 => format!("[dbg] pc={pc:016x} {name} = {v:#018x} ({})\n", v as i64),
                    Xlen::Rv32 => format!("[dbg] pc={pc:08x} {name} = {:#010x} ({})\n", v as u32, v as i32),
                };
                bus.debug_out(line.as_bytes());
            }
            Inst::XcalHalt { rs1 } => {
                if self.mode != Mode::M {
                    return Err(Self::illegal());
                }
                bus.exit = Some(crate::bus::Exit::Code(self.reg(rs1) as u32 & 0xff));
            }
            Inst::Illegal(_) => return Err(Self::illegal()),
        }
        Ok(next)
    }
}

/// The RV64 ALU: 64-bit operations, shift amounts from the low 6 bits of `b`.
fn alu64(op: AluOp, a: u64, b: u64) -> u64 {
    match op {
        AluOp::Add => a.wrapping_add(b),
        AluOp::Sub => a.wrapping_sub(b),
        AluOp::Sll => a << (b & 63),
        AluOp::Slt => ((a as i64) < (b as i64)) as u64,
        AluOp::Sltu => (a < b) as u64,
        AluOp::Xor => a ^ b,
        AluOp::Srl => a >> (b & 63),
        AluOp::Sra => ((a as i64) >> (b & 63)) as u64,
        AluOp::Or => a | b,
        AluOp::And => a & b,
    }
}

/// The 32-bit ALU of RV64's W instructions and of RV32: the operation on the low 32
/// bits of `a` and `b` (shift amounts from the low 5 bits of `b`), sign-extended.
fn alu32(op: AluOp, a: u64, b: u64) -> u64 {
    let (a, b) = (a as u32, b as u32);
    let v = match op {
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
    };
    sext32(v.into())
}

/// RV64's M extension. `mulh*` return the upper 64 bits of the 128-bit product.
/// Division by zero and overflow follow the spec, with no traps: dividing by zero gives
/// all ones (the remainder: the dividend), and `i64::MIN / -1` gives `i64::MIN`
/// (remainder 0).
fn muldiv64(op: MulOp, a: u64, b: u64) -> u64 {
    let (sa, sb) = (a as i64, b as i64);
    match op {
        MulOp::Mul => a.wrapping_mul(b),
        MulOp::Mulh => ((sa as i128 * sb as i128) >> 64) as u64,
        MulOp::Mulhsu => ((sa as i128 * b as i128) >> 64) as u64,
        MulOp::Mulhu => ((a as u128 * b as u128) >> 64) as u64,
        MulOp::Div => match sb {
            0 => u64::MAX,
            -1 => sa.wrapping_neg() as u64,
            _ => (sa / sb) as u64,
        },
        MulOp::Divu => a.checked_div(b).unwrap_or(u64::MAX),
        MulOp::Rem => match sb {
            0 => a,
            -1 => 0,
            _ => (sa % sb) as u64,
        },
        MulOp::Remu => a.checked_rem(b).unwrap_or(a),
    }
}

/// The 32-bit M extension of RV64's W instructions and of RV32: the operation on the
/// low 32 bits of `a` and `b`, sign-extended, with [`muldiv64`]'s rules on 32-bit
/// values (`i32::MIN / -1` gives `i32::MIN`). `mulh*` (RV32 only: RV64 has no 32-bit
/// form) return the upper 32 bits of the 64-bit product.
fn muldiv32(op: MulOp, a: u64, b: u64) -> u64 {
    let (a, b) = (a as u32, b as u32);
    let (sa, sb) = (a as i32, b as i32);
    let v = match op {
        MulOp::Mul => a.wrapping_mul(b),
        MulOp::Mulh => ((sa as i64 * sb as i64) >> 32) as u32,
        MulOp::Mulhsu => ((sa as i64 * b as i64) >> 32) as u32,
        MulOp::Mulhu => ((a as u64 * b as u64) >> 32) as u32,
        MulOp::Div => match sb {
            0 => u32::MAX,
            -1 => sa.wrapping_neg() as u32,
            _ => (sa / sb) as u32,
        },
        MulOp::Divu => a.checked_div(b).unwrap_or(u32::MAX),
        MulOp::Rem => match sb {
            0 => a,
            -1 => 0,
            _ => (sa % sb) as u32,
        },
        MulOp::Remu => a.checked_rem(b).unwrap_or(a),
    };
    sext32(v.into())
}
