//! Memory-mapped devices.

use crate::bus::{BusFault, Exit};
use std::collections::VecDeque;
use std::io::Write;
use std::sync::mpsc::Receiver;

/// CLINT offset of hart 0's `msip`; hart `i`'s is at `CLINT_MSIP + 4·i`.
pub const CLINT_MSIP: u32 = 0x0;
/// CLINT offset of hart 0's `mtimecmp`; hart `i`'s is at `CLINT_MTIMECMP + 8·i`.
pub const CLINT_MTIMECMP: u32 = 0x4000;
/// CLINT offset of `mtime`, shared by every hart.
pub const CLINT_MTIME: u32 = 0xbff8;

/// Core-local interruptor, with the standard (SiFive) layout: one software interrupt
/// bit (`msip`) and one timer compare register (`mtimecmp`) per hart, and the `mtime`
/// counter they all share. Hart `i`'s `msip` drives its `mip.MSIP`, so another hart
/// writing it is an inter-processor interrupt; its `mtimecmp` drives its `mip.MTIP`.
pub struct Clint {
    /// Hart `i`'s software interrupt bit, at `CLINT_MSIP + 4·i`.
    pub msip: Vec<bool>,
    /// Hart `i`'s timer compare register, at `CLINT_MTIMECMP + 8·i`.
    pub mtimecmp: Vec<u64>,
    pub mtime: u64,
}

impl Default for Clint {
    /// The CLINT of a one-hart machine.
    fn default() -> Self {
        Clint::new(1)
    }
}

impl Clint {
    /// The CLINT of a machine with `harts` harts.
    pub fn new(harts: usize) -> Self {
        // mtimecmp resets to "never" so the timer does not fire before software sets it.
        Clint {
            msip: vec![false; harts],
            mtimecmp: vec![u64::MAX; harts],
            mtime: 0,
        }
    }

    /// Whether hart `hart`'s timer interrupt (MTIP) is pending.
    pub fn timer_pending(&self, hart: usize) -> bool {
        self.mtime >= self.mtimecmp[hart]
    }

    /// The earliest `mtimecmp` still ahead of `mtime`: where a machine whose harts all
    /// wait in `wfi` can jump `mtime` to. `None` if no timer is set to fire.
    pub fn next_deadline(&self) -> Option<u64> {
        self.mtimecmp
            .iter()
            .copied()
            .filter(|&cmp| cmp != u64::MAX && cmp > self.mtime)
            .min()
    }

    /// Decodes a register offset: hart `i`'s `msip` or one half of its `mtimecmp`, or
    /// one half of `mtime`. Offsets of harts the machine does not have are reserved.
    fn reg(&self, off: u32) -> ClintReg {
        let harts = self.msip.len() as u32;
        match off {
            o if o < CLINT_MTIMECMP => match (o - CLINT_MSIP) / 4 {
                hart if hart < harts && o % 4 == 0 => ClintReg::Msip(hart as usize),
                _ => ClintReg::Reserved,
            },
            CLINT_MTIME => ClintReg::Mtime { high: false },
            o if o == CLINT_MTIME + 4 => ClintReg::Mtime { high: true },
            o if o < CLINT_MTIME => match (o - CLINT_MTIMECMP) / 8 {
                hart if hart < harts && o % 4 == 0 => ClintReg::Mtimecmp {
                    hart: hart as usize,
                    high: o % 8 == 4,
                },
                _ => ClintReg::Reserved,
            },
            _ => ClintReg::Reserved,
        }
    }

    /// Reads a register. `msip` is 32-bit, so it takes 4-byte reads only; `mtimecmp`
    /// and `mtime` are 64-bit, read whole with an 8-byte read or by their 4-byte halves
    /// (spec section 7). Reserved offsets read as 0 at either width.
    pub fn read(&mut self, off: u32, size: u32) -> Result<u64, BusFault> {
        let half = |v: u64, high: bool| if high { v >> 32 } else { v & 0xffff_ffff };
        Ok(match (self.reg(off), size) {
            (ClintReg::Msip(hart), 4) => self.msip[hart] as u64,
            (ClintReg::Mtimecmp { hart, high }, 4) => half(self.mtimecmp[hart], high),
            (ClintReg::Mtimecmp { hart, high: false }, 8) => self.mtimecmp[hart],
            (ClintReg::Mtime { high }, 4) => half(self.mtime, high),
            (ClintReg::Mtime { high: false }, 8) => self.mtime,
            (ClintReg::Reserved, 4 | 8) => 0,
            _ => return Err(BusFault),
        })
    }

    /// Writes a register, with the widths [`Clint::read`] takes. A 4-byte write to one
    /// half of a 64-bit register leaves the other half as it was.
    pub fn write(&mut self, off: u32, size: u32, val: u64) -> Result<(), BusFault> {
        let set_half = |old: &mut u64, high: bool| {
            *old = if high {
                (*old & 0xffff_ffff) | (val << 32)
            } else {
                (*old & !0xffff_ffff) | (val & 0xffff_ffff)
            }
        };
        match (self.reg(off), size) {
            (ClintReg::Msip(hart), 4) => self.msip[hart] = val & 1 != 0,
            (ClintReg::Mtimecmp { hart, high }, 4) => set_half(&mut self.mtimecmp[hart], high),
            (ClintReg::Mtimecmp { hart, high: false }, 8) => self.mtimecmp[hart] = val,
            (ClintReg::Mtime { high }, 4) => set_half(&mut self.mtime, high),
            (ClintReg::Mtime { high: false }, 8) => self.mtime = val,
            (ClintReg::Reserved, 4 | 8) => {}
            _ => return Err(BusFault),
        }
        Ok(())
    }
}

/// A CLINT register, as decoded from its offset.
enum ClintReg {
    Msip(usize),
    Mtimecmp { hart: usize, high: bool },
    Mtime { high: bool },
    Reserved,
}

/// PLIC interrupt source of the UART's "received data available" line.
pub const UART_IRQ: u32 = 1;
/// PLIC interrupt source of the keyboard's "event available" line (spec section 9.4).
pub const KBD_IRQ: u32 = 2;
/// PLIC interrupt source of disk 0's completion line; disk `i` uses `BLK_IRQ + i`
/// (spec section 9.6).
pub const BLK_IRQ: u32 = 3;
/// PLIC interrupt source of the pointer's "event available" line (spec section 9.7):
/// the first one after the disks' (3 to 10).
pub const PTR_IRQ: u32 = 11;
/// PLIC interrupt source of network card 0; card `i` uses `NET_IRQ + i` (spec section
/// 9.8).
pub const NET_IRQ: u32 = 12;
/// PLIC interrupt source of the framebuffer's vertical blank (spec section 9.3): the
/// first one after the network cards' (12 to 15).
pub const FB_IRQ: u32 = 16;

/// Interrupt sources 1..=31 exist; source 0 means "no interrupt".
pub const PLIC_SOURCES: u32 = 32;
/// Contexts per hart: context `2·i` is hart `i` in M-mode (drives its MEIP), context
/// `2·i + 1` is hart `i` in S-mode (drives its SEIP).
pub const PLIC_CONTEXTS_PER_HART: usize = 2;
/// Priorities and thresholds are 3 bits wide (0..=7), as on QEMU `virt`.
const PLIC_PRIO_MASK: u32 = 7;

pub const PLIC_PENDING: u32 = 0x1000;
/// Context 0's enable bits; context `ctx`'s are at `PLIC_ENABLE + PLIC_ENABLE_STRIDE·ctx`.
pub const PLIC_ENABLE: u32 = 0x2000;
pub const PLIC_ENABLE_STRIDE: u32 = 0x80;
/// Context 0's threshold; context `ctx`'s is at `PLIC_CONTEXT + PLIC_CONTEXT_STRIDE·ctx`,
/// and its claim/complete register 4 bytes after it.
pub const PLIC_CONTEXT: u32 = 0x20_0000;
pub const PLIC_CONTEXT_STRIDE: u32 = 0x1000;

/// Platform-level interrupt controller, a minimal version of the RISC-V PLIC with
/// the QEMU `virt` register layout (spec section 7.1): two contexts per hart, M and S.
///
/// Every source has a level-triggered gateway. The bus samples each device's line once
/// per step: a high line makes the source pending (and pending stays set until claimed,
/// even if the line drops), a claim clears pending and marks the source in flight, and no
/// new request is forwarded until software completes it. A line still high at the first
/// sample after completion makes the source pending again. Pending and in-flight bits
/// are shared by every context: of several harts enabled for a source, the first to
/// claim it gets it, and the others' claims return 0.
pub struct Plic {
    priority: [u32; PLIC_SOURCES as usize],
    /// Bit n: source n has a request waiting to be claimed.
    pending: u32,
    /// Bit n: source n was claimed and not completed yet.
    in_flight: u32,
    /// Per context.
    enable: Vec<u32>,
    /// Per context.
    threshold: Vec<u32>,
}

impl Default for Plic {
    /// The PLIC of a one-hart machine: two contexts.
    fn default() -> Self {
        Plic::new(1)
    }
}

impl Plic {
    /// The PLIC of a machine with `harts` harts: [`PLIC_CONTEXTS_PER_HART`] each.
    pub fn new(harts: usize) -> Self {
        let contexts = harts * PLIC_CONTEXTS_PER_HART;
        Plic {
            priority: [0; PLIC_SOURCES as usize],
            pending: 0,
            in_flight: 0,
            enable: vec![0; contexts],
            threshold: vec![0; contexts],
        }
    }

    /// How many contexts there are.
    pub fn contexts(&self) -> usize {
        self.enable.len()
    }

    /// Samples source `src`'s interrupt line (the bus calls this between instructions).
    /// The gateway forwards a request when the line is high and the source is not in flight.
    pub fn set_level(&mut self, src: u32, high: bool) {
        if high && src != 0 && src < PLIC_SOURCES {
            self.pending |= (1 << src) & !self.in_flight;
        }
    }

    /// The highest-priority source pending and enabled for `ctx` with priority above its
    /// threshold (ties go to the lowest source number), or 0 if there is none.
    pub fn best(&self, ctx: usize) -> u32 {
        let candidates = self.pending & self.enable[ctx];
        if candidates == 0 {
            // The common case (checked every step): skip the scan over all sources.
            return 0;
        }
        let mut best = (0, self.threshold[ctx]);
        for src in 1..PLIC_SOURCES {
            if candidates & (1 << src) != 0 && self.priority[src as usize] > best.1 {
                best = (src, self.priority[src as usize]);
            }
        }
        best.0
    }

    /// Whether context `ctx`'s external interrupt line (its hart's MEIP for an even `ctx`, SEIP
    /// for an odd one) is high.
    pub fn irq(&self, ctx: usize) -> bool {
        self.best(ctx) != 0
    }

    /// Claim: returns the best source for `ctx` and moves it from pending to in flight.
    pub fn claim(&mut self, ctx: usize) -> u32 {
        let src = self.best(ctx);
        if src != 0 {
            self.pending &= !(1 << src);
            self.in_flight |= 1 << src;
        }
        src
    }

    /// Complete: ends the claim on `src`. Ignored if `src` is not enabled for `ctx`.
    /// The source can pend again at the next line sample ([`Plic::set_level`]), so a
    /// handler that drained its device before completing does not see a stale request.
    pub fn complete(&mut self, ctx: usize, src: u32) {
        if src < PLIC_SOURCES && self.enable[ctx] & (1 << src) != 0 {
            self.in_flight &= !(1 << src);
        }
    }

    /// Decodes a register offset. Only the first word of each enable block exists
    /// (sources 0..=31); everything not listed is reserved: reads 0, writes ignored.
    fn reg(&self, off: u32) -> PlicReg {
        let contexts = self.contexts();
        let enable_end = PLIC_ENABLE + PLIC_ENABLE_STRIDE * contexts as u32;
        let (ctx, rel) = match off {
            o if o < 4 * PLIC_SOURCES => return PlicReg::Priority((o / 4) as usize),
            PLIC_PENDING => return PlicReg::Pending,
            o if (PLIC_ENABLE..enable_end).contains(&o) => {
                let rel = o - PLIC_ENABLE;
                return match (rel / PLIC_ENABLE_STRIDE, rel % PLIC_ENABLE_STRIDE) {
                    (ctx, 0) => PlicReg::Enable(ctx as usize),
                    _ => PlicReg::Reserved,
                };
            }
            o if o >= PLIC_CONTEXT => {
                let rel = o - PLIC_CONTEXT;
                ((rel / PLIC_CONTEXT_STRIDE) as usize, rel % PLIC_CONTEXT_STRIDE)
            }
            _ => return PlicReg::Reserved,
        };
        match rel {
            _ if ctx >= contexts => PlicReg::Reserved,
            0 => PlicReg::Threshold(ctx),
            4 => PlicReg::ClaimComplete(ctx),
            _ => PlicReg::Reserved,
        }
    }

    pub fn read(&mut self, off: u32, size: u32) -> Result<u32, BusFault> {
        if size != 4 || off & 3 != 0 {
            return Err(BusFault);
        }
        Ok(match self.reg(off) {
            PlicReg::Priority(src) => self.priority[src],
            PlicReg::Pending => self.pending,
            PlicReg::Enable(ctx) => self.enable[ctx],
            PlicReg::Threshold(ctx) => self.threshold[ctx],
            PlicReg::ClaimComplete(ctx) => self.claim(ctx),
            PlicReg::Reserved => 0,
        })
    }

    pub fn write(&mut self, off: u32, size: u32, val: u32) -> Result<(), BusFault> {
        if size != 4 || off & 3 != 0 {
            return Err(BusFault);
        }
        match self.reg(off) {
            // Source 0 does not exist: its priority stays 0 and it cannot be enabled.
            PlicReg::Priority(src) if src > 0 => self.priority[src] = val & PLIC_PRIO_MASK,
            PlicReg::Enable(ctx) => self.enable[ctx] = val & !1,
            PlicReg::Threshold(ctx) => self.threshold[ctx] = val & PLIC_PRIO_MASK,
            PlicReg::ClaimComplete(ctx) => self.complete(ctx, val),
            _ => {} // pending is read-only
        }
        Ok(())
    }
}

/// A PLIC register, as decoded from its offset.
enum PlicReg {
    Priority(usize),
    Pending,
    Enable(usize),
    Threshold(usize),
    ClaimComplete(usize),
    Reserved,
}

/// A small subset of a 16550 UART. Transmit goes to the machine's own output sink,
/// receive comes from its own input channel: nothing is tied to the process's stdio,
/// so each machine in a process has a separate console (the CLI wires stdin/stdout).
/// The only interrupt is "received data available" (IER bit 0), raised on PLIC source
/// [`UART_IRQ`].
#[derive(Default)]
pub struct Uart {
    pub input: Option<Receiver<u8>>,
    /// Where transmitted bytes go, flushed after each byte. `None` drops them.
    pub output: Option<Box<dyn Write + Send>>,
    rx: VecDeque<u8>,
    ier: u8,
    lcr: u8,
    mcr: u8,
    scr: u8,
    /// Everything transmitted, kept instead of going to `output` when `capture` is on
    /// (used by tests).
    pub captured: Vec<u8>,
    pub capture: bool,
}

const IER_ERBFI: u8 = 1 << 0; // enable received data available interrupt
const IIR_NONE: u8 = 0x01; // no interrupt pending
const IIR_RDA: u8 = 0x04; // received data available
const LSR_DR: u8 = 1 << 0; // data ready
const LSR_THRE: u8 = 1 << 5; // transmit holding register empty
const LSR_TEMT: u8 = 1 << 6; // transmitter empty

impl Uart {
    /// Level of the UART's PLIC line: high while IER.ERBFI is set and a byte is waiting in RBR.
    /// The bus samples it every step, so the host channel is only polled when the answer
    /// depends on it (interrupt enabled, nothing buffered yet).
    pub fn irq(&mut self) -> bool {
        if self.ier & IER_ERBFI == 0 {
            return false;
        }
        if self.rx.is_empty() {
            self.poll_input();
        }
        !self.rx.is_empty()
    }

    fn poll_input(&mut self) {
        if let Some(rx) = &self.input {
            while let Ok(b) = rx.try_recv() {
                self.rx.push_back(b);
            }
        }
    }

    pub fn read(&mut self, off: u32, size: u32) -> Result<u32, BusFault> {
        if size != 1 {
            return Err(BusFault);
        }
        self.poll_input();
        let dlab = self.lcr & 0x80 != 0;
        Ok(match off {
            0 if !dlab => self.rx.pop_front().unwrap_or(0) as u32,
            1 if !dlab => self.ier as u32,
            2 => (if self.irq() { IIR_RDA } else { IIR_NONE }) as u32,
            3 => self.lcr as u32,
            4 => self.mcr as u32,
            5 => (LSR_THRE | LSR_TEMT | if self.rx.is_empty() { 0 } else { LSR_DR }) as u32,
            7 => self.scr as u32,
            _ => 0,
        })
    }

    pub fn write(&mut self, off: u32, size: u32, val: u32) -> Result<(), BusFault> {
        if size != 1 {
            return Err(BusFault);
        }
        let b = val as u8;
        let dlab = self.lcr & 0x80 != 0;
        match off {
            0 if !dlab => {
                if self.capture {
                    self.captured.push(b);
                } else if let Some(out) = &mut self.output {
                    let _ = out.write_all(&[b]);
                    let _ = out.flush();
                }
            }
            1 if !dlab => self.ier = b & 0x0f,
            3 => self.lcr = b,
            4 => self.mcr = b,
            7 => self.scr = b,
            _ => {} // divisor latch, FCR: accepted and ignored
        }
        Ok(())
    }
}

/// SiFive-style test finisher: write 0x5555 to pass, 0x3333 | (code << 16) to fail.
pub struct TestDevice;

impl TestDevice {
    pub fn write(&mut self, off: u32, val: u32) -> Option<Exit> {
        if off != 0 {
            return None;
        }
        match val & 0xffff {
            0x5555 => Some(Exit::Code(0)),
            0x3333 => Some(Exit::Code(val >> 16)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clint_layout_has_a_msip_and_a_mtimecmp_per_hart() {
        let mut c = Clint::new(3);
        let r = |c: &mut Clint, off| c.read(off, 4).unwrap();
        c.write(8, 4, 0xffff_ffff).unwrap(); // msip[2]: bit 0 only
        assert_eq!(c.msip, [false, false, true]);
        assert_eq!(
            [r(&mut c, 0), r(&mut c, 4), r(&mut c, 8), r(&mut c, 12)],
            [0, 0, 1, 0]
        );
        c.write(0x4010, 4, 7).unwrap(); // mtimecmp[2], low half
        assert_eq!(c.mtimecmp, [u64::MAX, u64::MAX, 0xffff_ffff_0000_0007]);
        c.write(0x4014, 4, 0).unwrap();
        assert_eq!(r(&mut c, 0x4010), 7);
        assert_eq!(r(&mut c, 0x4018), 0, "no hart 3");
        c.write(0x4018, 4, 1).unwrap();
        // Misaligned and narrow accesses.
        assert_eq!(r(&mut c, 2), 0);
        assert_eq!(c.read(0, 2), Err(BusFault));
        assert_eq!(c.write(0x4000, 1, 0), Err(BusFault));
        c.write(CLINT_MTIME, 4, 7).unwrap();
        assert!(c.timer_pending(2));
        assert!(!c.timer_pending(0));
        assert_eq!(
            c.next_deadline(),
            None,
            "hart 2's is already due, the others unset"
        );
        c.mtimecmp[1] = 10;
        c.mtimecmp[0] = 20;
        assert_eq!(c.next_deadline(), Some(10));
    }

    #[test]
    fn clint_64_bit_registers_take_whole_and_half_accesses() {
        let mut c = Clint::new(2);
        // mtimecmp[1] whole, then each half read back.
        c.write(0x4008, 8, 0x1122_3344_5566_7788).unwrap();
        assert_eq!(c.mtimecmp[1], 0x1122_3344_5566_7788);
        assert_eq!(c.read(0x4008, 4), Ok(0x5566_7788));
        assert_eq!(c.read(0x400c, 4), Ok(0x1122_3344));
        // A half write keeps the other half; an 8-byte read sees both.
        c.write(0x400c, 4, 0xaabb_ccdd).unwrap();
        assert_eq!(c.read(0x4008, 8), Ok(0xaabb_ccdd_5566_7788));
        // mtime the same way.
        c.write(CLINT_MTIME, 8, 0x0000_0001_ffff_fffe).unwrap();
        assert_eq!(c.mtime, 0x0000_0001_ffff_fffe);
        assert_eq!(c.read(CLINT_MTIME + 4, 4), Ok(1));
        assert_eq!(c.read(CLINT_MTIME, 8), Ok(0x0000_0001_ffff_fffe));
        // msip is 32-bit: no 8-byte access, even to a pair of them.
        assert_eq!(c.read(0, 8), Err(BusFault));
        assert_eq!(c.write(0, 8, 1), Err(BusFault));
        assert_eq!(c.msip, [false, false]);
        // An 8-byte access to a register's high half straddles two registers.
        assert_eq!(c.read(0x400c, 8), Err(BusFault));
        assert_eq!(c.write(CLINT_MTIME + 4, 8, 0), Err(BusFault));
        // Reserved offsets (no hart 2) read as 0 and ignore writes at either width.
        assert_eq!(c.read(0x4010, 8), Ok(0));
        c.write(0x4010, 8, 5).unwrap();
        assert_eq!(c.mtimecmp, [u64::MAX, 0xaabb_ccdd_5566_7788]);
    }

    #[test]
    fn plic_contexts_follow_the_hart_count() {
        let mut p = Plic::new(3);
        assert_eq!(p.contexts(), 6);
        w(&mut p, 4, 1);
        p.set_level(1, true);
        let last = PLIC_CONTEXTS_PER_HART as u32 * 3 - 1;
        w(&mut p, PLIC_ENABLE + PLIC_ENABLE_STRIDE * last, 0b10);
        assert_eq!(r(&mut p, PLIC_ENABLE + PLIC_ENABLE_STRIDE * last), 0b10);
        assert!(p.irq(last as usize));
        assert!((0..last as usize).all(|ctx| !p.irq(ctx)));
        // Nothing past the last context.
        w(&mut p, PLIC_ENABLE + PLIC_ENABLE_STRIDE * (last + 1), 0b10);
        assert_eq!(r(&mut p, PLIC_ENABLE + PLIC_ENABLE_STRIDE * (last + 1)), 0);
        assert_eq!(r(&mut p, PLIC_CONTEXT + PLIC_CONTEXT_STRIDE * (last + 1) + 4), 0);
        assert_eq!(r(&mut p, PLIC_CONTEXT + PLIC_CONTEXT_STRIDE * last + 4), 1);
    }

    const CLAIM0: u32 = PLIC_CONTEXT + 4;
    const CLAIM1: u32 = PLIC_CONTEXT + PLIC_CONTEXT_STRIDE + 4;
    const THRESHOLD0: u32 = PLIC_CONTEXT;
    const THRESHOLD1: u32 = PLIC_CONTEXT + PLIC_CONTEXT_STRIDE;

    fn w(p: &mut Plic, off: u32, val: u32) {
        p.write(off, 4, val).unwrap();
    }

    fn r(p: &mut Plic, off: u32) -> u32 {
        p.read(off, 4).unwrap()
    }

    /// Sources 1..=3 with priorities 1, 3, 2, all enabled for context 0.
    fn plic3() -> Plic {
        let mut p = Plic::default();
        w(&mut p, 4, 1);
        w(&mut p, 8, 3);
        w(&mut p, 12, 2);
        w(&mut p, PLIC_ENABLE, 0b1110);
        p
    }

    #[test]
    fn claim_returns_highest_priority_pending_source() {
        let mut p = plic3();
        for src in 1..=3 {
            p.set_level(src, true);
        }
        assert_eq!(r(&mut p, PLIC_PENDING), 0b1110);
        assert!(p.irq(0));
        assert_eq!(r(&mut p, CLAIM0), 2);
        assert_eq!(r(&mut p, PLIC_PENDING), 0b1010, "claim clears pending");
        assert_eq!(r(&mut p, CLAIM0), 3);
        assert_eq!(r(&mut p, CLAIM0), 1);
        assert_eq!(r(&mut p, CLAIM0), 0, "nothing left to claim");
        assert!(!p.irq(0));
    }

    #[test]
    fn equal_priority_goes_to_lowest_source() {
        let mut p = Plic::default();
        w(&mut p, 4 * 5, 4);
        w(&mut p, 4 * 9, 4);
        w(&mut p, PLIC_ENABLE, (1 << 5) | (1 << 9));
        p.set_level(9, true);
        p.set_level(5, true);
        assert_eq!(r(&mut p, CLAIM0), 5);
        assert_eq!(r(&mut p, CLAIM0), 9);
    }

    #[test]
    fn disabled_and_zero_priority_sources_are_not_claimed() {
        let mut p = plic3();
        w(&mut p, PLIC_ENABLE, 0b0100); // only source 2
        p.set_level(1, true);
        p.set_level(3, true);
        assert!(!p.irq(0));
        assert_eq!(r(&mut p, CLAIM0), 0);
        assert_eq!(r(&mut p, PLIC_PENDING), 0b1010, "still pending, just not enabled");

        w(&mut p, PLIC_ENABLE, 0b1110);
        w(&mut p, 12, 0); // priority 0 never interrupts
        assert_eq!(r(&mut p, CLAIM0), 1);
        assert_eq!(r(&mut p, CLAIM0), 0);
    }

    #[test]
    fn complete_ends_the_claim_and_a_high_line_pends_again() {
        let mut p = plic3();
        p.set_level(2, true);
        assert_eq!(r(&mut p, CLAIM0), 2);
        // In flight: a line that stays high does not make it pending again.
        p.set_level(2, true);
        assert_eq!(r(&mut p, PLIC_PENDING), 0);
        assert!(!p.irq(0));

        w(&mut p, CLAIM0, 2);
        assert_eq!(
            r(&mut p, PLIC_PENDING),
            0,
            "requests are forwarded when lines are sampled"
        );
        p.set_level(2, true);
        assert_eq!(r(&mut p, PLIC_PENDING), 0b0100, "line still high: pending again");
        assert_eq!(r(&mut p, CLAIM0), 2);

        w(&mut p, CLAIM0, 2);
        p.set_level(2, false);
        assert_eq!(r(&mut p, PLIC_PENDING), 0, "line low: complete leaves it idle");
        assert!(!p.irq(0));
    }

    #[test]
    fn complete_for_a_source_not_enabled_is_ignored() {
        let mut p = plic3();
        p.set_level(3, true);
        assert_eq!(r(&mut p, CLAIM0), 3);
        w(&mut p, PLIC_ENABLE, 0b0110); // disable source 3
        w(&mut p, CLAIM0, 3);
        w(&mut p, PLIC_ENABLE, 0b1110);
        p.set_level(3, true);
        assert_eq!(r(&mut p, PLIC_PENDING), 0, "still in flight");
        w(&mut p, CLAIM0, 3);
        p.set_level(3, true);
        assert_eq!(r(&mut p, PLIC_PENDING), 0b1000);
    }

    #[test]
    fn pending_latches_until_claimed() {
        let mut p = plic3();
        p.set_level(1, true);
        p.set_level(1, false);
        assert_eq!(r(&mut p, PLIC_PENDING), 0b0010);
        assert_eq!(r(&mut p, CLAIM0), 1);
    }

    #[test]
    fn threshold_masks_priorities_at_or_below_it() {
        let mut p = plic3();
        p.set_level(1, true); // priority 1
        p.set_level(3, true); // priority 2
        w(&mut p, THRESHOLD0, 2);
        assert_eq!(r(&mut p, THRESHOLD0), 2);
        assert!(!p.irq(0), "priorities 1 and 2 are not above threshold 2");
        assert_eq!(r(&mut p, CLAIM0), 0);

        w(&mut p, THRESHOLD0, 1);
        assert!(p.irq(0));
        assert_eq!(r(&mut p, CLAIM0), 3);
        assert_eq!(r(&mut p, CLAIM0), 0, "source 1 is masked by threshold 1");

        w(&mut p, THRESHOLD0, 0);
        assert_eq!(r(&mut p, CLAIM0), 1);
    }

    #[test]
    fn contexts_have_their_own_enables_and_thresholds() {
        let mut p = plic3();
        w(&mut p, PLIC_ENABLE + PLIC_ENABLE_STRIDE, 0b0010); // context 1: source 1 only
        w(&mut p, PLIC_ENABLE, 0b0100); // context 0: source 2 only
        assert_eq!(r(&mut p, PLIC_ENABLE + PLIC_ENABLE_STRIDE), 0b0010);
        p.set_level(1, true);
        assert!(!p.irq(0));
        assert!(p.irq(1));
        w(&mut p, THRESHOLD1, 7);
        assert!(!p.irq(1), "context 1 threshold masks it");
        w(&mut p, THRESHOLD1, 0);
        assert_eq!(r(&mut p, CLAIM0), 0);
        assert_eq!(r(&mut p, CLAIM1), 1);

        // In flight: the line stays high, but nothing pends until context 1 completes.
        p.set_level(1, true);
        assert!(!p.irq(1));
        w(&mut p, CLAIM0, 1);
        p.set_level(1, true);
        assert_eq!(
            r(&mut p, PLIC_PENDING),
            0,
            "context 0 does not enable source 1, so its complete is ignored"
        );
        w(&mut p, CLAIM1, 1);
        p.set_level(1, true);
        assert_eq!(
            r(&mut p, PLIC_PENDING),
            0b0010,
            "context 1's complete ended the claim"
        );
        assert!(p.irq(1));

        assert_eq!(r(&mut p, CLAIM1), 1);
        p.set_level(1, false);
        w(&mut p, CLAIM1, 1);
        p.set_level(1, false);
        assert_eq!(r(&mut p, PLIC_PENDING), 0);
        assert!(!p.irq(1));
    }

    #[test]
    fn register_warl_rules() {
        let mut p = Plic::default();
        w(&mut p, 0, 7);
        assert_eq!(r(&mut p, 0), 0, "source 0 has no priority");
        w(&mut p, 4, 0xffff_ffff);
        assert_eq!(r(&mut p, 4), 7, "priorities are 3 bits");
        w(&mut p, PLIC_ENABLE, 0xffff_ffff);
        assert_eq!(r(&mut p, PLIC_ENABLE), 0xffff_fffe, "source 0 cannot be enabled");
        w(&mut p, THRESHOLD0, 0xff);
        assert_eq!(r(&mut p, THRESHOLD0), 7);
        w(&mut p, PLIC_PENDING, 0xffff_ffff);
        assert_eq!(r(&mut p, PLIC_PENDING), 0, "pending is read-only");
        assert_eq!(
            r(&mut p, PLIC_CONTEXT + 2 * PLIC_CONTEXT_STRIDE),
            0,
            "no context 2"
        );
        assert_eq!(r(&mut p, PLIC_ENABLE + 2 * PLIC_ENABLE_STRIDE), 0, "no context 2");
        // Only the first word of each enable block (sources 0..=31) exists. Drivers that
        // clear every enable word must not wipe the real one.
        w(&mut p, PLIC_ENABLE + PLIC_ENABLE_STRIDE, 0b0110);
        for ctx in 0..PLIC_CONTEXTS_PER_HART as u32 {
            for word in [4, 8, PLIC_ENABLE_STRIDE - 4] {
                let off = PLIC_ENABLE + ctx * PLIC_ENABLE_STRIDE + word;
                w(&mut p, off, 0);
                w(&mut p, off, 0xffff_ffff);
                assert_eq!(r(&mut p, off), 0, "enable word {off:#x} is reserved");
            }
        }
        assert_eq!(r(&mut p, PLIC_ENABLE), 0xffff_fffe);
        assert_eq!(r(&mut p, PLIC_ENABLE + PLIC_ENABLE_STRIDE), 0b0110);
        // Same for the words after each context's threshold and claim/complete registers.
        w(&mut p, THRESHOLD1, 5);
        for ctx in 0..PLIC_CONTEXTS_PER_HART as u32 {
            for word in [8, 12, PLIC_CONTEXT_STRIDE - 4] {
                let off = PLIC_CONTEXT + ctx * PLIC_CONTEXT_STRIDE + word;
                w(&mut p, off, 0xffff_ffff);
                assert_eq!(r(&mut p, off), 0, "context word {off:#x} is reserved");
            }
        }
        assert_eq!(r(&mut p, THRESHOLD0), 7);
        assert_eq!(r(&mut p, THRESHOLD1), 5);
        assert_eq!(p.read(4, 1), Err(BusFault), "32-bit accesses only");
        assert_eq!(p.write(6, 2, 0), Err(BusFault));
        p.set_level(0, true);
        p.set_level(PLIC_SOURCES, true);
        assert_eq!(r(&mut p, PLIC_PENDING), 0, "out-of-range sources are ignored");
    }

    #[test]
    fn uart_line_follows_ier_and_data_ready() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut u = Uart {
            input: Some(rx),
            ..Uart::default()
        };
        tx.send(b'x').unwrap();
        assert!(!u.irq(), "IER.ERBFI is off");
        assert_eq!(u.read(2, 1).unwrap(), IIR_NONE as u32);
        u.write(1, 1, 1).unwrap();
        assert!(u.irq());
        assert_eq!(u.read(2, 1).unwrap(), IIR_RDA as u32);
        assert_eq!(u.read(5, 1).unwrap() & LSR_DR as u32, LSR_DR as u32);
        assert_eq!(u.read(0, 1).unwrap(), b'x' as u32);
        assert!(!u.irq(), "RBR drained");
        assert_eq!(u.read(2, 1).unwrap(), IIR_NONE as u32);
    }

    /// Records writes and flushes, so the test can see each byte is flushed on its own.
    struct Log(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

    impl Write for Log {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let s = String::from_utf8_lossy(buf).into_owned();
            self.0.lock().unwrap().push(s);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.0.lock().unwrap().push("<flush>".into());
            Ok(())
        }
    }

    #[test]
    fn uart_transmits_to_its_own_sink() {
        let log = std::sync::Arc::default();
        let mut u = Uart::default();
        u.write(0, 1, b'x' as u32).unwrap(); // no sink: dropped
        u.output = Some(Box::new(Log(std::sync::Arc::clone(&log))));
        u.write(0, 1, b'h' as u32).unwrap();
        u.write(0, 1, 0x100 | b'i' as u32).unwrap(); // only the low byte is sent
        assert_eq!(*log.lock().unwrap(), ["h", "<flush>", "i", "<flush>"]);

        // DLAB set: offset 0 is the divisor latch, nothing is sent.
        u.write(3, 1, 0x80).unwrap();
        u.write(0, 1, b'z' as u32).unwrap();
        u.write(3, 1, 0x03).unwrap();
        u.capture = true;
        u.write(0, 1, b'c' as u32).unwrap();
        assert_eq!(u.captured, b"c");
        assert_eq!(
            log.lock().unwrap().len(),
            4,
            "captured bytes do not reach the sink"
        );
    }

    #[test]
    fn uart_line_control_and_divisor_latch() {
        // Spec section 7: LCR, MCR and SCR read back what was written; with
        // LCR.DLAB set, +0 and +1 are the divisor latch (writes ignored,
        // reads 0), so IER keeps its value.
        let mut u = Uart::default();
        for (off, v) in [(3, 0x03), (4, 0x0b), (7, 0x5a)] {
            u.write(off, 1, v).unwrap();
            assert_eq!(u.read(off, 1).unwrap(), v, "+{off}");
        }
        u.write(1, 1, 0x01).unwrap();
        u.write(3, 1, 0x83).unwrap();
        u.write(1, 1, 0x00).unwrap();
        assert_eq!((u.read(0, 1).unwrap(), u.read(1, 1).unwrap()), (0, 0));
        u.write(3, 1, 0x03).unwrap();
        assert_eq!(u.read(1, 1).unwrap(), 0x01, "IER untouched while DLAB was set");
        // FCR (+2 on write) is ignored; +2 reads IIR.
        u.write(2, 1, 0xc7).unwrap();
        assert_eq!(u.read(2, 1).unwrap(), 0x01);
    }
}
