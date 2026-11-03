//! The control and status register file, with privilege checks and WARL rules.
//!
//! Every CSR is kept in 64 bits (`mcounteren`, `scounteren` and `mcountinhibit`, 32
//! bits wide on both widths, in 32). An RV32 hart's CSRs are 32 bits wide: a write
//! keeps the low 32 bits of the value written ([`Hart::csr_store`]), so they read back
//! zero-extended, and the high halves of the 64-bit counters are CSRs of their own.
//! On RV64 those RV32-only CSRs do not exist ([`excalibur::csr::rv32_only`]).

use super::{Hart, Mode};
use crate::bus::Bus;
use crate::trap::{Exception, Trap};
use excalibur::csr::*;
use excalibur::Xlen;

/// mstatus bits software can write. FS/VS/XS stay 0 (no F/V extensions), and so does
/// SD; UXL and SXL are read-only (see [`Hart::mstatus_xl`]).
const MSTATUS_WMASK: u64 = MSTATUS_SIE
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
/// The subset of mstatus visible through sstatus (and UXL, on RV64).
const SSTATUS_MASK: u64 = MSTATUS_SIE | MSTATUS_SPIE | MSTATUS_SPP | MSTATUS_SUM | MSTATUS_MXR;

const S_INTERRUPTS: u64 = (1 << IRQ_S_SOFT) | (1 << IRQ_S_TIMER) | (1 << IRQ_S_EXT);
const M_INTERRUPTS: u64 = (1 << IRQ_M_SOFT) | (1 << IRQ_M_TIMER) | (1 << IRQ_M_EXT);
/// mip bits that software can write directly (the M bits come from devices).
const MIP_WMASK: u64 = S_INTERRUPTS;
/// Every exception can be delegated except ecall-from-M.
const MEDELEG_MASK: u64 = 0xb3ff;

/// misa's extensions: A, I, M, S, U, and X (non-standard: Xcal).
const MISA_EXTENSIONS: u64 = (1 << 0) | (1 << 8) | (1 << 12) | (1 << 18) | (1 << 20) | (1 << 23);

/// `misa` on a hart `xlen` wide: MXL in its top two bits (1 for RV32, 2 for RV64) and
/// the extensions: 0x4094_1101 on RV32, 0x8000_0000_0094_1101 on RV64.
pub fn misa(xlen: Xlen) -> u64 {
    match xlen {
        Xlen::Rv32 => (1 << 30) | MISA_EXTENSIONS,
        Xlen::Rv64 => (2 << 62) | MISA_EXTENSIONS,
    }
}

/// `pmpaddr` on RV64 holds address bits 55:2, so its top 10 bits read 0.
const PMPADDR_RV64_MASK: u64 = (1 << 54) - 1;

/// RV64 `satp`: MODE [63:60] and PPN [43:0] (spec section 5). There are no ASIDs, so
/// the ASID field [59:44] reads 0, and physical addresses are 48 bits, so the PPN keeps
/// its low 36 bits (bits 43:36 read 0).
pub(super) const SATP_RV64_MODE_SHIFT: u32 = 60;
pub(super) const SATP_RV64_PPN: u64 = (1 << 36) - 1;
/// `satp`'s modes on RV64: Bare (no translation), Sv39 and Sv48.
const SATP_MODE_BARE: u64 = 0;
pub(super) const SATP_MODE_SV39: u64 = 8;
pub(super) const SATP_MODE_SV48: u64 = 9;

#[derive(Default)]
pub struct Csrs {
    pub mstatus: u64,
    pub medeleg: u64,
    pub mideleg: u64,
    pub mie: u64,
    /// Software-writable pending bits. MTIP and MSIP are added from the CLINT on read,
    /// MEIP and SEIP from the PLIC (SEIP is the OR of this bit and PLIC context 1).
    pub mip: u64,
    pub mtvec: u64,
    pub mcounteren: u32,
    pub mcountinhibit: u32,
    pub mscratch: u64,
    pub mepc: u64,
    pub mcause: u64,
    pub mtval: u64,
    pub stvec: u64,
    pub scounteren: u32,
    pub sscratch: u64,
    pub sepc: u64,
    pub scause: u64,
    pub stval: u64,
    pub satp: u64,
    pub mcycle: u64,
    pub minstret: u64,
    /// `pmpcfg0`-`pmpcfg3`. On RV64 only the even two exist, each with 8 entries'
    /// configuration bytes.
    pub pmpcfg: [u64; 4],
    pub pmpaddr: [u64; 16],
}

fn illegal() -> Trap {
    // xtval for illegal instruction is filled in by the caller with the instruction bits.
    Trap::new(Exception::IllegalInstr, 0)
}

/// tvec registers: mode 0 (direct) or 1 (vectored); other modes are reserved, keep direct.
fn warl_tvec(v: u64) -> u64 {
    if v & 3 >= 2 {
        v & !3
    } else {
        v
    }
}

impl Hart {
    /// Current value of mip, including the device-driven bits: MTIP and MSIP from the
    /// hart's CLINT registers, MEIP and SEIP from its two PLIC contexts.
    pub fn mip(&self, bus: &Bus) -> u64 {
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

    /// mstatus's read-only XLEN fields: on RV64, UXL and SXL hard-wired to 2 (64-bit, no
    /// compat mode); RV32 has neither.
    fn mstatus_xl(&self) -> u64 {
        match self.xlen {
            Xlen::Rv64 => MSTATUS_UXL_64 | MSTATUS_SXL_64,
            Xlen::Rv32 => 0,
        }
    }

    /// The value csrrs/csrrc modify. Same as a read, except that in `mip` the SEIP bit is the
    /// software-writable one only, so a read-modify-write cannot latch the PLIC's SEIP into it
    /// (privileged spec, section 3.1.9).
    pub(super) fn csr_rmw_base(&self, addr: u16, read: u64) -> u64 {
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

    pub fn csr_read(&mut self, bus: &Bus, addr: u16) -> Result<u64, Trap> {
        self.check_access(addr, false)?;
        let counter = (CYCLE..=0xc1f).contains(&addr) || (CYCLEH..=0xc9f).contains(&addr);
        if counter && !self.counter_allowed(addr) {
            return Err(illegal());
        }
        self.csr_value(bus, addr).ok_or_else(illegal)
    }

    /// Whether the hart has CSR `addr` at all: the RV32-only ones are missing on RV64.
    /// (Grail's other unknown CSRs are the ones [`Hart::csr_value`] does not list.)
    fn csr_exists(&self, addr: u16) -> bool {
        self.xlen == Xlen::Rv32 || !rv32_only(addr)
    }

    /// The value of CSR `addr` as an M-mode read would see it, without the privilege
    /// checks: `None` if the hart has no such CSR. For [`Hart::csr_read`], and for a
    /// debugger, which may read any CSR whatever the hart's mode.
    pub fn csr_value(&self, bus: &Bus, addr: u16) -> Option<u64> {
        if !self.csr_exists(addr) {
            return None;
        }
        let c = &self.csr;
        // A 64-bit counter as one CSR shows it: whole on RV64, its low half on RV32
        // (whose high half is the `...h` CSR).
        let low = |v: u64| match self.xlen {
            Xlen::Rv64 => v,
            Xlen::Rv32 => v as u32 as u64,
        };
        Some(match addr {
            SSTATUS => (c.mstatus & SSTATUS_MASK) | (self.mstatus_xl() & MSTATUS_UXL),
            SIE => c.mie & c.mideleg,
            STVEC => c.stvec,
            SCOUNTEREN => c.scounteren.into(),
            SENVCFG => 0,
            SSCRATCH => c.sscratch,
            SEPC => c.sepc,
            SCAUSE => c.scause,
            STVAL => c.stval,
            SIP => self.mip(bus) & c.mideleg,
            SATP => c.satp,

            MSTATUS => c.mstatus | self.mstatus_xl(),
            MISA => misa(self.xlen),
            MEDELEG => c.medeleg,
            MIDELEG => c.mideleg,
            MIE => c.mie,
            MTVEC => c.mtvec,
            MCOUNTEREN => c.mcounteren.into(),
            MENVCFG | MENVCFGH | MSTATUSH => 0,
            MCOUNTINHIBIT => c.mcountinhibit.into(),
            MSCRATCH => c.mscratch,
            MEPC => c.mepc,
            MCAUSE => c.mcause,
            MTVAL => c.mtval,
            MIP => self.mip(bus),
            a if (PMPCFG0..PMPCFG0 + 4).contains(&a) => c.pmpcfg[(a - PMPCFG0) as usize],
            a if (PMPADDR0..PMPADDR0 + 16).contains(&a) => c.pmpaddr[(a - PMPADDR0) as usize],
            MHPMEVENT3..=0x33f => 0,

            MCYCLE => low(c.mcycle),
            MCYCLEH => c.mcycle >> 32,
            MINSTRET => low(c.minstret),
            MINSTRETH => c.minstret >> 32,
            MHPMCOUNTER3..=0xb1f | MHPMCOUNTER3H..=0xb9f => 0,

            CYCLE..=0xc1f | CYCLEH..=0xc9f => {
                let time = bus.clint.mtime;
                match addr {
                    CYCLE => low(c.mcycle),
                    CYCLEH => c.mcycle >> 32,
                    TIME => low(time),
                    TIMEH => time >> 32,
                    INSTRET => low(c.minstret),
                    INSTRETH => c.minstret >> 32,
                    _ => 0,
                }
            }

            // Debug triggers: tselect hard-wired to a non-zero value means "no triggers".
            TSELECT => 1,
            TDATA1..=TCONTROL => 0,

            MHARTID => self.id.into(),
            MVENDORID | MARCHID | MIMPID | MCONFIGPTR => 0,
            _ => return None,
        })
    }

    pub fn csr_write(&mut self, addr: u16, v: u64) -> Result<(), Trap> {
        self.check_access(addr, true)?;
        if self.csr_store(addr, v) {
            Ok(())
        } else {
            Err(illegal())
        }
    }

    /// The new `satp` after a write of `v` (WARL). RV32: any value, MODE [31] picking
    /// Bare or Sv32. RV64: a write whose MODE the hart does not implement changes
    /// nothing, as the privileged spec requires; the others keep MODE and the PPN bits
    /// a 48-bit physical address has (spec section 5): Bare, Sv39 and Sv48.
    fn warl_satp(&self, v: u64) -> u64 {
        match self.xlen {
            Xlen::Rv32 => v,
            Xlen::Rv64 => match v >> SATP_RV64_MODE_SHIFT {
                SATP_MODE_BARE | SATP_MODE_SV39 | SATP_MODE_SV48 => {
                    v & ((0xf << SATP_RV64_MODE_SHIFT) | SATP_RV64_PPN)
                }
                _ => self.csr.satp,
            },
        }
    }

    /// Writes `v` to CSR `addr` as an M-mode write would, with its WARL rules but
    /// without the privilege and read-only checks: false if the hart has no such CSR or
    /// it is read-only (`misa`'s writes are ignored, a read-only counter's are refused).
    /// On RV32 only `v`'s low 32 bits are written. For [`Hart::csr_write`], and for a
    /// debugger.
    pub fn csr_store(&mut self, addr: u16, v: u64) -> bool {
        if !self.csr_exists(addr) {
            return false;
        }
        let rv64 = self.xlen == Xlen::Rv64;
        let v = if rv64 { v } else { v as u32 as u64 };
        let satp = self.warl_satp(v);
        let c = &mut self.csr;
        match addr {
            SSTATUS => c.mstatus = (c.mstatus & !SSTATUS_MASK) | (v & SSTATUS_MASK),
            SIE => c.mie = (c.mie & !c.mideleg) | (v & c.mideleg),
            STVEC => c.stvec = warl_tvec(v),
            SCOUNTEREN => c.scounteren = v as u32 & 7,
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
            SATP => c.satp = satp,

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
            MCOUNTEREN => c.mcounteren = v as u32 & 7,
            MCOUNTINHIBIT => c.mcountinhibit = v as u32 & 5,
            MSCRATCH => c.mscratch = v,
            MEPC => c.mepc = v & !3,
            MCAUSE => c.mcause = v,
            MTVAL => c.mtval = v,
            MIP => c.mip = (c.mip & !MIP_WMASK) | (v & MIP_WMASK),
            a if (PMPCFG0..PMPCFG0 + 4).contains(&a) => c.pmpcfg[(a - PMPCFG0) as usize] = v,
            a if (PMPADDR0..PMPADDR0 + 16).contains(&a) => {
                c.pmpaddr[(a - PMPADDR0) as usize] = if rv64 { v & PMPADDR_RV64_MASK } else { v }
            }
            MHPMEVENT3..=0x33f => {}

            MCYCLE if rv64 => c.mcycle = v,
            MCYCLE => c.mcycle = (c.mcycle & !0xffff_ffff) | v,
            MCYCLEH => c.mcycle = (c.mcycle & 0xffff_ffff) | (v << 32),
            MINSTRET => {
                c.minstret = if rv64 { v } else { (c.minstret & !0xffff_ffff) | v };
                self.instret_written = true;
            }
            MINSTRETH => {
                c.minstret = (c.minstret & 0xffff_ffff) | (v << 32);
                self.instret_written = true;
            }
            MHPMCOUNTER3..=0xb1f | MHPMCOUNTER3H..=0xb9f => {}

            TSELECT..=TCONTROL => {}
            _ => return false,
        }
        true
    }
}
