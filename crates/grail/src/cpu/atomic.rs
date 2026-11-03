//! The A extension: `lr`, `sc` and the `amo*` read-modify-write instructions, in
//! their `.w` and `.d` (RV64) forms (spec section 4.1).
//!
//! Grail runs one instruction at a time, its harts taking turns, so an AMO's
//! read and write cannot be split by another hart's access, and the `aq`/`rl`
//! bits need no work. What remains is the address checks and the LR/SC reservation
//! ([`super::Reservation`]).
//!
//! The `.w` forms work on 32 bits: they load a word and sign-extend it into rd,
//! compute on the low 32 bits of rs2 (`amomin.w` compares as 32-bit signed values,
//! `amominu.w` as 32-bit unsigned ones) and store a word.

use super::Hart;
use crate::bus::Bus;
use crate::trap::{Access, Trap};
use excalibur::AmoOp;

/// The value an AMO writes back, from the old memory value and rs2.
fn amo_result64(op: AmoOp, old: u64, src: u64) -> u64 {
    match op {
        AmoOp::Swap => src,
        AmoOp::Add => old.wrapping_add(src),
        AmoOp::Xor => old ^ src,
        AmoOp::And => old & src,
        AmoOp::Or => old | src,
        AmoOp::Min => (old as i64).min(src as i64) as u64,
        AmoOp::Max => (old as i64).max(src as i64) as u64,
        AmoOp::Minu => old.min(src),
        AmoOp::Maxu => old.max(src),
    }
}

/// [`amo_result64`] for a `.w` AMO, on 32-bit values.
fn amo_result32(op: AmoOp, old: u32, src: u32) -> u32 {
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

/// The value an AMO of `size` bytes writes back.
fn amo_result(op: AmoOp, size: u32, old: u64, src: u64) -> u64 {
    match size {
        4 => amo_result32(op, old as u32, src as u32).into(),
        _ => amo_result64(op, old, src),
    }
}

/// What rd gets from an atomic's load of `size` bytes: a word sign-extended, a
/// doubleword whole.
fn loaded(size: u32, v: u64) -> u64 {
    match size {
        4 => v as i32 as i64 as u64,
        _ => v,
    }
}

impl Hart {
    /// The checks every A instruction makes on its address, in this order: it
    /// must be aligned to the access size (else address-misaligned), translate
    /// with `access` permissions (page faults, A/D bits), and land in memory, RAM
    /// or VRAM (else access fault: device registers do not support atomics).
    fn atomic_paddr(&mut self, bus: &mut Bus, vaddr: u64, size: u32, access: Access) -> Result<u64, Trap> {
        if vaddr & (size as u64 - 1) != 0 {
            return Err(Trap::new(access.misaligned(), vaddr));
        }
        let paddr = self.translate(bus, vaddr, access)?;
        let memory = u32::try_from(paddr).is_ok_and(|a| bus.is_memory(a, size));
        if !memory {
            return Err(Trap::new(access.access_fault(), vaddr));
        }
        Ok(paddr)
    }

    /// `lr.w`/`lr.d`: loads `size` bytes and reserves the line holding them. Faults
    /// like a load.
    pub(super) fn load_reserved(&mut self, bus: &mut Bus, vaddr: u64, size: u32) -> Result<u64, Trap> {
        let vaddr = self.addr(vaddr);
        let paddr = self.atomic_paddr(bus, vaddr, size, Access::Load)?;
        let v = self
            .read_phys(bus, paddr, size)
            .map_err(|_| Trap::new(Access::Load.access_fault(), vaddr))?;
        bus.reservation_mut(self.id as usize).reserve(paddr, size);
        bus.watches.note(self.id, vaddr as u32, size, true, false);
        Ok(loaded(size, v))
    }

    /// `sc.w`/`sc.d`: stores the low `size` bytes of `val` if the hart's reservation
    /// allows it (one made by an `lr` of this address and size, and not broken since),
    /// and returns rd: 0 if it stored, 1 if not. The reservation is gone either way.
    ///
    /// A misaligned address always traps. Without any reservation the store
    /// fails before touching memory: no translation, so no page fault and no
    /// A/D update. With one, the address is checked like a store's (faults,
    /// A and D set) before it is compared with the reservation.
    pub(super) fn store_conditional(
        &mut self,
        bus: &mut Bus,
        vaddr: u64,
        size: u32,
        val: u64,
    ) -> Result<u64, Trap> {
        let vaddr = self.addr(vaddr);
        if vaddr & (size as u64 - 1) != 0 {
            return Err(Trap::new(Access::Store.misaligned(), vaddr));
        }
        if bus.reservation(self.id as usize).held().is_none() {
            return Ok(1);
        }
        let paddr = self.atomic_paddr(bus, vaddr, size, Access::Store)?;
        if !bus.reservation_mut(self.id as usize).take(paddr, size) {
            return Ok(1);
        }
        self.write_phys(bus, paddr, size, val)
            .map_err(|_| Trap::new(Access::Store.access_fault(), vaddr))?;
        bus.watches.note(self.id, vaddr as u32, size, false, true);
        Ok(0)
    }

    /// `amo<op>.w`/`.d`: reads `size` bytes, writes `op(old, src)` back and returns the
    /// old value for rd (a word sign-extended). Faults like a store (the page must be
    /// writable, and A and D get set), with store/AMO causes.
    pub(super) fn amo(
        &mut self,
        bus: &mut Bus,
        op: AmoOp,
        vaddr: u64,
        size: u32,
        src: u64,
    ) -> Result<u64, Trap> {
        let vaddr = self.addr(vaddr);
        let paddr = self.atomic_paddr(bus, vaddr, size, Access::Store)?;
        let fault = |_| Trap::new(Access::Store.access_fault(), vaddr);
        let old = self.read_phys(bus, paddr, size).map_err(fault)?;
        self.write_phys(bus, paddr, size, amo_result(op, size, old, src))
            .map_err(fault)?;
        bus.watches.note(self.id, vaddr as u32, size, true, true);
        Ok(loaded(size, old))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amo_results() {
        let (neg, pos) = (0x8000_0000, 1);
        assert_eq!(amo_result32(AmoOp::Swap, 5, 7), 7);
        assert_eq!(amo_result32(AmoOp::Add, u32::MAX, 2), 1);
        assert_eq!(amo_result32(AmoOp::Xor, 0b1100, 0b1010), 0b0110);
        assert_eq!(amo_result32(AmoOp::And, 0b1100, 0b1010), 0b1000);
        assert_eq!(amo_result32(AmoOp::Or, 0b1100, 0b1010), 0b1110);
        assert_eq!(amo_result32(AmoOp::Min, neg, pos), neg);
        assert_eq!(amo_result32(AmoOp::Max, neg, pos), pos);
        assert_eq!(amo_result32(AmoOp::Minu, neg, pos), pos);
        assert_eq!(amo_result32(AmoOp::Maxu, neg, pos), neg);
    }

    #[test]
    fn amo_results_64() {
        let (neg, pos) = (1 << 63, 1);
        assert_eq!(amo_result64(AmoOp::Add, u64::MAX, 2), 1);
        assert_eq!(amo_result64(AmoOp::Min, neg, pos), neg);
        assert_eq!(amo_result64(AmoOp::Max, neg, pos), pos);
        assert_eq!(amo_result64(AmoOp::Minu, neg, pos), pos);
        assert_eq!(amo_result64(AmoOp::Maxu, neg, pos), neg);
    }

    /// A `.w` AMO works on the low 32 bits whatever rs2's upper ones hold, and compares
    /// as 32-bit values: 0x8000_0000 is negative to `amomin.w`, though positive in 64.
    #[test]
    fn word_amos_use_the_low_32_bits() {
        let src = 0xffff_ffff_8000_0000;
        assert_eq!(amo_result(AmoOp::Min, 4, 1, src), 0x8000_0000);
        assert_eq!(amo_result(AmoOp::Minu, 4, 1, src), 1);
        assert_eq!(amo_result(AmoOp::Add, 4, 0xffff_ffff, 0x1_0000_0001), 0);
        assert_eq!(amo_result(AmoOp::Min, 8, 1, 0x8000_0000), 1);
        assert_eq!(loaded(4, 0x8000_0000), 0xffff_ffff_8000_0000);
        assert_eq!(loaded(4, 0x7fff_ffff), 0x7fff_ffff);
        assert_eq!(loaded(8, 0x8000_0000), 0x8000_0000);
    }
}
