//! The control and status register file, with privilege checks and WARL rules.

use super::{Hart, Mode};
use crate::bus::Bus;
use crate::trap::{Exception, Trap};
use excalibur::csr::*;

/// mstatus bits software can write. FS/VS/XS stay 0 (no F/V extensions).
const MSTATUS_WMASK: u32 = MSTATUS_SIE
    | MSTATUS_MIE
    | MSTATUS_SPIE
    | MSTATUS_MPIE
    | MSTATUS_SPP
    | MSTATUS_MPP
    | MSTATUS_MPRV
    | MSTATUS_SUM
    | MSTATUS_MXR
    | MSTATUS_TVM
    | MSTATUS_TW
    | MSTATUS_TSR;
/// The subset of mstatus visible through sstatus.
const SSTATUS_MASK: u32 = MSTATUS_SIE | MSTATUS_SPIE | MSTATUS_SPP | MSTATUS_SUM | MSTATUS_MXR;

const S_INTERRUPTS: u32 = (1 << IRQ_S_SOFT) | (1 << IRQ_S_TIMER) | (1 << IRQ_S_EXT);
const M_INTERRUPTS: u32 = (1 << IRQ_M_SOFT) | (1 << IRQ_M_TIMER) | (1 << IRQ_M_EXT);
/// mip bits that software can write directly (the M bits come from devices).
const MIP_WMASK: u32 = S_INTERRUPTS;
/// Every exception can be delegated except ecall-from-M.
const MEDELEG_MASK: u32 = 0xb3ff;

/// misa: MXL=1 (32-bit), extensions A, I, M, S, U, and X (non-standard: Xcal).
pub const MISA_VALUE: u32 = (1 << 30) | (1 << 0) | (1 << 8) | (1 << 12) | (1 << 18) | (1 << 20) | (1 << 23);

#[derive(Default)]
pub struct Csrs {
    pub mstatus: u32,
    pub medeleg: u32,
    pub mideleg: u32,
    pub mie: u32,
    /// Software-writable pending bits. MTIP and MSIP are added from the CLINT on read,
    /// MEIP and SEIP from the PLIC (SEIP is the OR of this bit and PLIC context 1).
    pub mip: u32,
    pub mtvec: u32,
    pub mcounteren: u32,
    pub mcountinhibit: u32,
    pub mscratch: u32,
    pub mepc: u32,
    pub mcause: u32,
    pub mtval: u32,
    pub stvec: u32,
    pub scounteren: u32,
    pub sscratch: u32,
    pub sepc: u32,
    pub scause: u32,
    pub stval: u32,
    pub satp: u32,
    pub mcycle: u64,
    pub minstret: u64,
    pub pmpcfg: [u32; 4],
    pub pmpaddr: [u32; 16],
}

fn illegal() -> Trap {
    // xtval for illegal instruction is filled in by the caller with the instruction bits.
    Trap::new(Exception::IllegalInstr, 0)
}

/// tvec registers: mode 0 (direct) or 1 (vectored); other modes are reserved, keep direct.
fn warl_tvec(v: u32) -> u32 {
    if v & 3 >= 2 {
        v & !3
    } else {
        v
    }
}

impl Hart {
    /// Current value of mip, including the device-driven bits: MTIP and MSIP from the
    /// hart's CLINT registers, MEIP and SEIP from its two PLIC contexts.
    pub fn mip(&self, bus: &Bus) -> u32 {
        let hart = self.id as usize;
        let mut mip = self.csr.mip;
        if bus.clint.timer_pending(hart) {
            mip |= 1 << IRQ_M_TIMER;
        }
        if bus.clint.msip[hart] {
            mip |= 1 << IRQ_M_SOFT;
        }
        if bus.plic.irq(2 * hart) {
            mip |= 1 << IRQ_M_EXT;
        }
        if bus.plic.irq(2 * hart + 1) {
            mip |= 1 << IRQ_S_EXT;
        }
        mip
    }

    /// The value csrrs/csrrc modify. Same as a read, except that in `mip` the SEIP bit is the
    /// software-writable one only, so a read-modify-write cannot latch the PLIC's SEIP into it
    /// (privileged spec, section 3.1.9).
    pub(super) fn csr_rmw_base(&self, addr: u16, read: u32) -> u32 {
        if addr == MIP {
            let seip = 1 << IRQ_S_EXT;
            (read & !seip) | (self.csr.mip & seip)
        } else {
            read
        }
    }

    /// S/U counter access is gated by mcounteren (and scounteren for U).
    fn counter_allowed(&self, addr: u16) -> bool {
        let bit = 1u32 << (addr & 0x1f);
        match self.mode {
            Mode::M => true,
            Mode::S => self.csr.mcounteren & bit != 0,
            Mode::U => self.csr.mcounteren & bit != 0 && self.csr.scounteren & bit != 0,
        }
    }

    fn check_access(&self, addr: u16, write: bool) -> Result<(), Trap> {
        let min_priv = (addr >> 8) & 3;
        if (self.mode as u16) < min_priv {
            return Err(illegal());
        }
        if write && (addr >> 10) & 3 == 3 {
            return Err(illegal());
        }
        if addr == SATP && self.mode == Mode::S && self.csr.mstatus & MSTATUS_TVM != 0 {
            return Err(illegal());
        }
        Ok(())
    }

    pub fn csr_read(&mut self, bus: &Bus, addr: u16) -> Result<u32, Trap> {
        self.check_access(addr, false)?;
        let counter = (CYCLE..=0xc1f).contains(&addr) || (CYCLEH..=0xc9f).contains(&addr);
        if counter && !self.counter_allowed(addr) {
            return Err(illegal());
        }
        self.csr_value(bus, addr).ok_or_else(illegal)
    }

    /// The value of CSR `addr` as an M-mode read would see it, without the privilege
    /// checks: `None` if Grail has no such CSR. For [`Hart::csr_read`], and for a
    /// debugger, which may read any CSR whatever the hart's mode.
    pub fn csr_value(&self, bus: &Bus, addr: u16) -> Option<u32> {
        let c = &self.csr;
        Some(match addr {
            SSTATUS => c.mstatus & SSTATUS_MASK,
            SIE => c.mie & c.mideleg,
            STVEC => c.stvec,
            SCOUNTEREN => c.scounteren,
            SENVCFG => 0,
            SSCRATCH => c.sscratch,
            SEPC => c.sepc,
            SCAUSE => c.scause,
            STVAL => c.stval,
            SIP => self.mip(bus) & c.mideleg,
            SATP => c.satp,

            MSTATUS => c.mstatus,
            MISA => MISA_VALUE,
            MEDELEG => c.medeleg,
            MIDELEG => c.mideleg,
            MIE => c.mie,
            MTVEC => c.mtvec,
            MCOUNTEREN => c.mcounteren,
            MENVCFG | MENVCFGH | MSTATUSH => 0,
            MCOUNTINHIBIT => c.mcountinhibit,
            MSCRATCH => c.mscratch,
            MEPC => c.mepc,
            MCAUSE => c.mcause,
            MTVAL => c.mtval,
            MIP => self.mip(bus),
            a if (PMPCFG0..PMPCFG0 + 4).contains(&a) => c.pmpcfg[(a - PMPCFG0) as usize],
            a if (PMPADDR0..PMPADDR0 + 16).contains(&a) => c.pmpaddr[(a - PMPADDR0) as usize],
            MHPMEVENT3..=0x33f => 0,

            MCYCLE => c.mcycle as u32,
            MCYCLEH => (c.mcycle >> 32) as u32,
            MINSTRET => c.minstret as u32,
            MINSTRETH => (c.minstret >> 32) as u32,
            MHPMCOUNTER3..=0xb1f | MHPMCOUNTER3H..=0xb9f => 0,

            CYCLE..=0xc1f | CYCLEH..=0xc9f => {
                let time = bus.clint.mtime;
                match addr {
                    CYCLE => c.mcycle as u32,
                    CYCLEH => (c.mcycle >> 32) as u32,
                    TIME => time as u32,
                    TIMEH => (time >> 32) as u32,
                    INSTRET => c.minstret as u32,
                    INSTRETH => (c.minstret >> 32) as u32,
                    _ => 0,
                }
            }

            // Debug triggers: tselect hard-wired to a non-zero value means "no triggers".
            TSELECT => 1,
            TDATA1..=TCONTROL => 0,

            MHARTID => self.id,
            MVENDORID | MARCHID | MIMPID | MCONFIGPTR => 0,
            _ => return None,
        })
    }

    pub fn csr_write(&mut self, addr: u16, v: u32) -> Result<(), Trap> {
        self.check_access(addr, true)?;
        if self.csr_store(addr, v) {
            Ok(())
        } else {
            Err(illegal())
        }
    }

    /// Writes `v` to CSR `addr` as an M-mode write would, with its WARL rules but
    /// without the privilege and read-only checks: false if Grail has no such CSR or it
    /// is read-only (`misa`'s writes are ignored, a read-only counter's are refused).
    /// For [`Hart::csr_write`], and for a debugger.
    pub fn csr_store(&mut self, addr: u16, v: u32) -> bool {
        let c = &mut self.csr;
        match addr {
            SSTATUS => c.mstatus = (c.mstatus & !SSTATUS_MASK) | (v & SSTATUS_MASK),
            SIE => c.mie = (c.mie & !c.mideleg) | (v & c.mideleg),
            STVEC => c.stvec = warl_tvec(v),
            SCOUNTEREN => c.scounteren = v & 7,
            SENVCFG => {}
            SSCRATCH => c.sscratch = v,
            SEPC => c.sepc = v & !3,
            SCAUSE => c.scause = v,
            STVAL => c.stval = v,
            // Only SSIP is writable through sip.
            SIP => {
                let m = c.mideleg & (1 << IRQ_S_SOFT);
                c.mip = (c.mip & !m) | (v & m);
            }
            SATP => c.satp = v,

            MSTATUS => {
                let mut v = v & MSTATUS_WMASK;
                // MPP = 2 is reserved: treat as U.
                if (v & MSTATUS_MPP) >> MSTATUS_MPP_SHIFT == 2 {
                    v &= !MSTATUS_MPP;
                }
                c.mstatus = (c.mstatus & !MSTATUS_WMASK) | v;
            }
            MISA | MENVCFG | MENVCFGH | MSTATUSH => {}
            MEDELEG => c.medeleg = v & MEDELEG_MASK,
            MIDELEG => c.mideleg = v & S_INTERRUPTS,
            MIE => c.mie = v & (S_INTERRUPTS | M_INTERRUPTS),
            MTVEC => c.mtvec = warl_tvec(v),
            MCOUNTEREN => c.mcounteren = v & 7,
            MCOUNTINHIBIT => c.mcountinhibit = v & 5,
            MSCRATCH => c.mscratch = v,
            MEPC => c.mepc = v & !3,
            MCAUSE => c.mcause = v,
            MTVAL => c.mtval = v,
            MIP => c.mip = (c.mip & !MIP_WMASK) | (v & MIP_WMASK),
            a if (PMPCFG0..PMPCFG0 + 4).contains(&a) => c.pmpcfg[(a - PMPCFG0) as usize] = v,
            a if (PMPADDR0..PMPADDR0 + 16).contains(&a) => c.pmpaddr[(a - PMPADDR0) as usize] = v,
            MHPMEVENT3..=0x33f => {}

            MCYCLE => c.mcycle = (c.mcycle & !0xffff_ffff) | v as u64,
            MCYCLEH => c.mcycle = (c.mcycle & 0xffff_ffff) | ((v as u64) << 32),
            MINSTRET => {
                c.minstret = (c.minstret & !0xffff_ffff) | v as u64;
                self.instret_written = true;
            }
            MINSTRETH => {
                c.minstret = (c.minstret & 0xffff_ffff) | ((v as u64) << 32);
                self.instret_written = true;
            }
            MHPMCOUNTER3..=0xb1f | MHPMCOUNTER3H..=0xb9f => {}

            TSELECT..=TCONTROL => {}
            _ => return false,
        }
        true
    }
}
