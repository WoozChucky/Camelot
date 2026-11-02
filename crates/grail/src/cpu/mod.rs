//! The hart: registers, the step loop, instruction execution and trap entry.

mod csr;
mod mmu;

pub use csr::{Csrs, MISA_VALUE};

use crate::bus::Bus;
use crate::trap::{Exception, Trap, INTERRUPT_BIT};
use excalibur::csr::*;
use excalibur::{decode, AluOp, BranchOp, CsrOp, CsrSrc, Inst, LoadOp, MulOp, StoreOp};

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

pub struct Cpu {
    pub x: [u32; 32],
    pub pc: u32,
    pub mode: Mode,
    pub csr: Csrs,
    pub bus: Bus,
    /// Stalled in `wfi` until an interrupt becomes pending.
    pub waiting: bool,
    /// Set when the current instruction wrote minstret, which suppresses its increment.
    instret_written: bool,
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

impl Cpu {
    pub fn new(bus: Bus) -> Self {
        Cpu {
            x: [0; 32],
            pc: crate::bus::BOOT_ROM_BASE,
            mode: Mode::M,
            csr: Csrs::default(),
            bus,
            waiting: false,
            instret_written: false,
        }
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

    /// Runs one instruction (or takes one interrupt). Time advances by one tick per step.
    pub fn step(&mut self) {
        self.bus.clint.mtime = self.bus.clint.mtime.wrapping_add(1);

        if let Some(irq) = self.pending_interrupt() {
            self.waiting = false;
            self.enter_trap(INTERRUPT_BIT | irq, 0, self.pc);
            return;
        }

        if self.waiting {
            if self.mip() & self.csr.mie != 0 {
                self.waiting = false;
            } else {
                // Nothing to do until the timer fires: skip straight to it.
                let clint = &mut self.bus.clint;
                if clint.mtimecmp != u64::MAX && clint.mtimecmp > clint.mtime {
                    clint.mtime = clint.mtimecmp;
                }
                return;
            }
        }

        let pc = self.pc;
        self.instret_written = false;
        let result = self.fetch(pc).and_then(|word| {
            self.execute(word, pc).map_err(|mut t| {
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
            Err(t) => self.enter_trap(t.cause as u32, t.tval, pc),
        }
        if self.csr.mcountinhibit & 1 == 0 {
            self.csr.mcycle = self.csr.mcycle.wrapping_add(1);
        }
    }

    /// Highest-priority interrupt that is pending, enabled, and allowed in the current mode.
    fn pending_interrupt(&self) -> Option<u32> {
        let pending = self.mip() & self.csr.mie;
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
    fn enter_trap(&mut self, cause: u32, tval: u32, epc: u32) {
        let interrupt = cause & INTERRUPT_BIT != 0;
        let code = cause & !INTERRUPT_BIT;
        let deleg = if interrupt {
            self.csr.mideleg
        } else {
            self.csr.medeleg
        };
        let to_s = self.mode <= Mode::S && (deleg >> code) & 1 != 0;
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
    fn execute(&mut self, word: u32, pc: u32) -> Result<u32, Trap> {
        let next = pc.wrapping_add(4);
        match decode(word) {
            Inst::Lui { rd, imm } => self.set_reg(rd, imm),
            Inst::Auipc { rd, imm } => self.set_reg(rd, pc.wrapping_add(imm)),
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
                let v = self.load(addr, op.size())?;
                let v = match op {
                    LoadOp::Lb => v as i8 as i32 as u32,
                    LoadOp::Lh => v as i16 as i32 as u32,
                    LoadOp::Lw | LoadOp::Lbu | LoadOp::Lhu => v,
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
                };
                self.store(addr, size, v)?;
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
            // Single hart, no caches, no instruction cache: fences are no-ops.
            Inst::Fence | Inst::FenceI => {}
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
                let old = self.csr_read(csr)?;
                if writes {
                    let new = match op {
                        CsrOp::Rw => operand,
                        CsrOp::Rs => old | operand,
                        CsrOp::Rc => old & !operand,
                    };
                    self.csr_write(csr, new)?;
                }
                self.set_reg(rd, old);
            }
            Inst::XcalDbg { rs1 } => {
                let v = self.reg(rs1);
                let name = excalibur::reg::abi_name(rs1);
                let line = format!("[dbg] pc={pc:08x} {name} = {v:#010x} ({})\n", v as i32);
                self.bus.debug_out(line.as_bytes());
            }
            Inst::XcalHalt { rs1 } => {
                if self.mode != Mode::M {
                    return Err(Self::illegal());
                }
                self.bus.exit = Some(crate::bus::Exit::Code(self.reg(rs1) & 0xff));
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
