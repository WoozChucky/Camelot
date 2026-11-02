//! The A extension: `lr.w`, `sc.w` and the `amo*.w` read-modify-write
//! instructions (spec section 4.1).
//!
//! Grail runs one instruction at a time, its harts taking turns, so an AMO's
//! read and write cannot be split by another hart's access, and the `aq`/`rl`
//! bits need no work. What remains is the address checks and the LR/SC reservation
//! ([`super::Reservation`]).

use super::Hart;
use crate::bus::Bus;
use crate::trap::{Access, Trap};
use excalibur::AmoOp;

/// The word an AMO writes back, from the old memory word and rs2.
fn amo_result(op: AmoOp, old: u32, src: u32) -> u32 {
    match op {
        AmoOp::Swap => src,
        AmoOp::Add => old.wrapping_add(src),
        AmoOp::Xor => old ^ src,
        AmoOp::And => old & src,
        AmoOp::Or => old | src,
        AmoOp::Min => (old as i32).min(src as i32) as u32,
        AmoOp::Max => (old as i32).max(src as i32) as u32,
        AmoOp::Minu => old.min(src),
        AmoOp::Maxu => old.max(src),
    }
}

impl Hart {
    /// The checks every A instruction makes on its address, in this order: it
    /// must be word-aligned (else address-misaligned), translate with `access`
    /// permissions (page faults, A/D bits), and land in memory, RAM or VRAM
    /// (else access fault: device registers do not support atomics).
    fn atomic_paddr(&mut self, bus: &mut Bus, vaddr: u32, access: Access) -> Result<u32, Trap> {
        if vaddr & 3 != 0 {
            return Err(Trap::new(access.misaligned(), vaddr));
        }
        let paddr = self.translate(bus, vaddr, access)?;
        if !bus.is_memory(paddr, 4) {
            return Err(Trap::new(access.access_fault(), vaddr));
        }
        Ok(paddr)
    }

    /// `lr.w`: loads the word and reserves it. Faults like a load.
    pub(super) fn load_reserved(&mut self, bus: &mut Bus, vaddr: u32) -> Result<u32, Trap> {
        let paddr = self.atomic_paddr(bus, vaddr, Access::Load)?;
        let v = bus
            .read(paddr, 4)
            .map_err(|_| Trap::new(Access::Load.access_fault(), vaddr))?;
        bus.reservation_mut(self.id as usize).reserve(paddr);
        Ok(v)
    }

    /// `sc.w`: stores `val` if the hart holds a reservation on the word, and
    /// returns rd: 0 if it stored, 1 if not. The reservation is gone either
    /// way.
    ///
    /// A misaligned address always traps. Without any reservation the store
    /// fails before touching memory: no translation, so no page fault and no
    /// A/D update. With one, the address is checked like a store's (faults,
    /// A and D set) before it is compared with the reservation.
    pub(super) fn store_conditional(&mut self, bus: &mut Bus, vaddr: u32, val: u32) -> Result<u32, Trap> {
        if vaddr & 3 != 0 {
            return Err(Trap::new(Access::Store.misaligned(), vaddr));
        }
        if bus.reservation(self.id as usize).held().is_none() {
            return Ok(1);
        }
        let paddr = self.atomic_paddr(bus, vaddr, Access::Store)?;
        if !bus.reservation_mut(self.id as usize).take(paddr) {
            return Ok(1);
        }
        self.write_phys(bus, paddr, 4, val)
            .map_err(|_| Trap::new(Access::Store.access_fault(), vaddr))?;
        Ok(0)
    }

    /// `amo<op>.w`: reads the word, writes `op(old, src)` back and returns the
    /// old value for rd. Faults like a store (the page must be writable, and A
    /// and D get set), with store/AMO causes.
    pub(super) fn amo(&mut self, bus: &mut Bus, op: AmoOp, vaddr: u32, src: u32) -> Result<u32, Trap> {
        let paddr = self.atomic_paddr(bus, vaddr, Access::Store)?;
        let fault = |_| Trap::new(Access::Store.access_fault(), vaddr);
        let old = bus.read(paddr, 4).map_err(fault)?;
        self.write_phys(bus, paddr, 4, amo_result(op, old, src))
            .map_err(fault)?;
        Ok(old)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amo_results() {
        let (neg, pos) = (0x8000_0000, 1);
        assert_eq!(amo_result(AmoOp::Swap, 5, 7), 7);
        assert_eq!(amo_result(AmoOp::Add, u32::MAX, 2), 1);
        assert_eq!(amo_result(AmoOp::Xor, 0b1100, 0b1010), 0b0110);
        assert_eq!(amo_result(AmoOp::And, 0b1100, 0b1010), 0b1000);
        assert_eq!(amo_result(AmoOp::Or, 0b1100, 0b1010), 0b1110);
        assert_eq!(amo_result(AmoOp::Min, neg, pos), neg);
        assert_eq!(amo_result(AmoOp::Max, neg, pos), pos);
        assert_eq!(amo_result(AmoOp::Minu, neg, pos), pos);
        assert_eq!(amo_result(AmoOp::Maxu, neg, pos), neg);
    }
}
