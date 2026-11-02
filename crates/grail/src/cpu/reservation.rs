//! LR/SC reservations (A extension, spec section 4.1).
//!
//! `lr.w` registers a *reservation set*: the naturally aligned word it read,
//! by physical address. `sc.w` writes only if the hart still holds a
//! reservation on the word it is writing, and reports whether it did. A
//! reservation is lost when:
//!
//! - a store writes any byte of the reserved word: `sb`/`sh`/`sw`, an AMO or a
//!   successful `sc.w`, from this hart or from another one;
//! - the hart executes `sc.w` (successful or not);
//! - the hart takes a trap (an exception or an interrupt);
//! - the hart stores to a device register, which may start a DMA (the block
//!   device's) that writes RAM without any hart storing there.
//!
//! Reservations hold physical addresses, so two virtual mappings of one word
//! share a reservation, and a store from another hart (which may map the page
//! at a different virtual address) can be matched against it.
//!
//! Each hart owns one [`Reservation`]. Grail has a single hart, which reports
//! its own stores to it ([`Reservation::snoop`]). A multi-hart machine reports
//! every store to every hart's reservation, which is what makes another hart's
//! store break an LR/SC sequence; nothing in this type assumes a single hart.

/// Size of a reservation set in bytes: one naturally aligned word.
pub const GRANULE: u32 = 4;

/// A hart's reservation: the physical address of the reserved word, if any.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reservation {
    word: Option<u32>,
}

impl Reservation {
    /// The reserved word's physical address, if the hart holds a reservation.
    pub fn held(&self) -> Option<u32> {
        self.word
    }

    /// `lr.w` at physical address `paddr`: replaces any earlier reservation.
    pub fn reserve(&mut self, paddr: u32) {
        self.word = Some(paddr & !(GRANULE - 1));
    }

    /// Drops the reservation (`sc.w`, a trap, a device store).
    pub fn clear(&mut self) {
        self.word = None;
    }

    /// `sc.w` at physical address `paddr`: whether the store may happen,
    /// that is whether the reservation covers `paddr`. The reservation is gone
    /// afterwards either way.
    pub fn take(&mut self, paddr: u32) -> bool {
        self.word.take() == Some(paddr & !(GRANULE - 1))
    }

    /// A store of `size` bytes at physical address `paddr` happened, by this
    /// hart or by another: drop the reservation if the store touched it.
    pub fn snoop(&mut self, paddr: u32, size: u32) {
        if let Some(word) = self.word {
            // 64-bit so a store at the top of the address space cannot wrap.
            let (start, end) = (paddr as u64, paddr as u64 + size as u64);
            let word = word as u64;
            if start < word + GRANULE as u64 && word < end {
                self.word = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_matches_the_reserved_word() {
        let mut r = Reservation::default();
        assert!(!r.take(0x8000_0000), "no reservation");
        r.reserve(0x8000_0000);
        assert_eq!(r.held(), Some(0x8000_0000));
        assert!(r.take(0x8000_0000));
        assert!(!r.take(0x8000_0000), "sc consumes the reservation");

        r.reserve(0x8000_0000);
        assert!(!r.take(0x8000_0004), "another word");
        assert_eq!(r.held(), None, "a failed sc consumes it too");

        r.reserve(0x8000_0100);
        r.reserve(0x8000_0200);
        assert!(!r.take(0x8000_0100), "a new lr replaces the old reservation");
    }

    #[test]
    fn snoop_drops_on_overlap_only() {
        let mut r = Reservation::default();
        r.reserve(0x8000_0010);
        r.snoop(0x8000_000c, 4);
        r.snoop(0x8000_0014, 1);
        r.snoop(0x8000_0008, 8);
        assert_eq!(r.held(), Some(0x8000_0010), "neighbouring stores");
        for (addr, size) in [
            (0x8000_0010, 4),
            (0x8000_0013, 1),
            (0x8000_0012, 2),
            (0x8000_000c, 8),
        ] {
            r.reserve(0x8000_0010);
            r.snoop(addr, size);
            assert_eq!(r.held(), None, "{addr:#x}+{size}");
        }
        // The top of the address space does not wrap around to 0.
        r.reserve(0);
        r.snoop(0xffff_fffc, 4);
        assert_eq!(r.held(), Some(0));
        r.reserve(0xffff_fffc);
        r.snoop(0xffff_fffc, 4);
        assert_eq!(r.held(), None);
        r.reserve(0x8000_0000);
        r.clear();
        assert_eq!(r.held(), None);
    }
}
