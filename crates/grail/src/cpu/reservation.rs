//! LR/SC reservations (A extension, spec section 4.1).
//!
//! `lr.w`/`lr.d` registers a *reservation set*: the naturally aligned 64-byte line
//! (a cache line) that holds the bytes it read, by physical address. `sc` writes only
//! if the hart still holds the reservation made by an `lr` of the same address and
//! size, and reports whether it did. A reservation is lost when:
//!
//! - a write touches any byte of the reserved line: a store, an AMO or a successful
//!   `sc`, from this hart or from another one, a page walker's A/D update, or a
//!   device's DMA;
//! - the hart executes `sc` (successful or not);
//! - the hart takes a trap (an exception or an interrupt);
//! - the hart stores to a device register, which may start a DMA (the block
//!   device's) that writes RAM without any hart storing there.
//!
//! Reservations hold physical addresses, so two virtual mappings of one line
//! share a reservation, and a store from another hart (which may map the page
//! at a different virtual address) can be matched against it.
//!
//! Each hart owns one [`Reservation`], kept by the bus, which reports every write
//! to every hart's reservation ([`Reservation::snoop`]): that is what makes another
//! hart's store break an LR/SC sequence.

/// Size of a reservation set in bytes: one naturally aligned 64-byte line.
pub const LINE: u64 = 64;

/// What an `lr` reserved: the address and size it read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Reserved {
    paddr: u64,
    size: u32,
}

/// A hart's reservation, if it holds one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reservation {
    held: Option<Reserved>,
}

impl Reservation {
    /// The physical address the reserving `lr` read, if the hart holds a reservation.
    pub fn held(&self) -> Option<u64> {
        self.held.map(|r| r.paddr)
    }

    /// The reserved line's physical address, if the hart holds a reservation.
    pub fn line(&self) -> Option<u64> {
        self.held().map(|a| a & !(LINE - 1))
    }

    /// `lr` of `size` bytes at physical address `paddr`: replaces any earlier
    /// reservation.
    pub fn reserve(&mut self, paddr: u64, size: u32) {
        self.held = Some(Reserved { paddr, size });
    }

    /// Drops the reservation (`sc`, a trap, a device store).
    pub fn clear(&mut self) {
        self.held = None;
    }

    /// `sc` of `size` bytes at physical address `paddr`: whether the store may happen,
    /// that is whether the reservation is the one an `lr` of this address and size
    /// made. The reservation is gone afterwards either way.
    pub fn take(&mut self, paddr: u64, size: u32) -> bool {
        self.held.take() == Some(Reserved { paddr, size })
    }

    /// A write of `size` bytes at physical address `paddr` happened, by this hart, by
    /// another or by a device: drop the reservation if the write touched its line.
    pub fn snoop(&mut self, paddr: u64, size: u64) {
        if let Some(line) = self.line() {
            // 128-bit so neither range can wrap at the top of the address space.
            let (start, end) = (paddr as u128, paddr as u128 + size as u128);
            let line = line as u128;
            if start < line + LINE as u128 && line < end {
                self.held = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_matches_the_reserving_lr() {
        let mut r = Reservation::default();
        assert!(!r.take(0x8000_0000, 4), "no reservation");
        r.reserve(0x8000_0000, 4);
        assert_eq!(r.held(), Some(0x8000_0000));
        assert!(r.take(0x8000_0000, 4));
        assert!(!r.take(0x8000_0000, 4), "sc consumes the reservation");

        r.reserve(0x8000_0000, 4);
        assert!(!r.take(0x8000_0004, 4), "another word of the line");
        assert_eq!(r.held(), None, "a failed sc consumes it too");

        r.reserve(0x8000_0000, 8);
        assert!(!r.take(0x8000_0000, 4), "sc.w after lr.d");
        r.reserve(0x8000_0000, 4);
        assert!(!r.take(0x8000_0000, 8), "sc.d after lr.w");

        r.reserve(0x8000_0100, 4);
        r.reserve(0x8000_0200, 4);
        assert!(!r.take(0x8000_0100, 4), "a new lr replaces the old reservation");
    }

    #[test]
    fn snoop_drops_on_a_write_to_the_line_only() {
        let mut r = Reservation::default();
        r.reserve(0x8000_0050, 4);
        assert_eq!(r.line(), Some(0x8000_0040));
        r.snoop(0x8000_003c, 4);
        r.snoop(0x8000_0080, 1);
        r.snoop(0x8000_0000, 0x40);
        assert_eq!(r.held(), Some(0x8000_0050), "neighbouring lines");
        for (addr, size) in [
            (0x8000_0050, 4),
            (0x8000_0040, 1),
            (0x8000_007f, 1),
            (0x8000_0078, 8),
            (0x8000_003c, 8),
        ] {
            r.reserve(0x8000_0050, 4);
            r.snoop(addr, size);
            assert_eq!(r.held(), None, "{addr:#x}+{size}");
        }
        // The top of the address space does not wrap around to 0.
        r.reserve(0, 8);
        r.snoop(u64::MAX - 7, 8);
        assert_eq!(r.held(), Some(0));
        r.reserve(u64::MAX - 7, 8);
        r.snoop(u64::MAX - 7, 8);
        assert_eq!(r.held(), None);
        r.reserve(0x8000_0000, 4);
        r.clear();
        assert_eq!(r.held(), None);
    }
}
