//! Register widths of the Xcal devices (spec section 9, "Register widths").
//!
//! A device's registers are 32-bit or 64-bit. A 32-bit register takes 4-byte accesses
//! only; a 64-bit one takes an 8-byte access of the whole register or a 4-byte access
//! of either half (the low half at the lower address), so that a 32-bit driver can
//! still reach it. Unused offsets read as 0 and ignore writes at either width. Every
//! other access is a fault.
//!
//! Version 1 of the interfaces (RV32 machines) has 32-bit registers only, so it takes
//! no 8-byte access at all, not even of unused space. The framebuffer's `FB_SCANOUT` and
//! `FB_CURSOR_ADDR` are newer than version 1 and 64-bit at both widths, so the framebuffer takes 8-byte
//! accesses on RV32 too. A device describes its layout with [`Field`]s, and [`access`]
//! applies the rules.

use crate::bus::BusFault;

/// What a 4-byte-aligned offset of a device holds: a 32-bit register `R`, one half of a
/// 64-bit one, or nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field<R> {
    Word(R),
    /// The low half of a 64-bit register, at its own offset.
    Low(R),
    /// The high half, 4 bytes above.
    High(R),
    Unused,
}

/// What an access reaches, once its width is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access<R> {
    /// A 32-bit register.
    Word(R),
    /// One half of a 64-bit register.
    Low(R),
    High(R),
    /// All of a 64-bit register (an 8-byte access).
    Whole(R),
    /// Nothing: reads as 0, ignores writes.
    Unused,
}

/// Checks an access of `size` bytes at `off` against a device's layout (`field` tells
/// what each 4-byte-aligned offset holds). `wide` says the device has 64-bit registers
/// (version 2 of the interfaces, or the framebuffer at either width); without it, 8-byte
/// accesses fault.
pub fn access<R>(
    off: u32,
    size: u32,
    wide: bool,
    field: impl Fn(u32) -> Field<R>,
) -> Result<Access<R>, BusFault> {
    // The harts fault on misaligned accesses before they reach the bus; whatever else
    // asks (a debugger) finds nothing between registers.
    let field = |off: u32| {
        if off.is_multiple_of(4) {
            field(off)
        } else {
            Field::Unused
        }
    };
    match size {
        4 => Ok(match field(off) {
            Field::Word(r) => Access::Word(r),
            Field::Low(r) => Access::Low(r),
            Field::High(r) => Access::High(r),
            Field::Unused => Access::Unused,
        }),
        8 if wide && off.is_multiple_of(8) => match (field(off), field(off + 4)) {
            (Field::Low(r), _) => Ok(Access::Whole(r)),
            (Field::Unused, Field::Unused) => Ok(Access::Unused),
            // A 32-bit register, or two, or one and unused space: not a width they take.
            _ => Err(BusFault),
        },
        _ => Err(BusFault),
    }
}

impl<R: Copy> Access<R> {
    /// The register reached, if any.
    pub fn reg(&self) -> Option<R> {
        match *self {
            Access::Word(r) | Access::Low(r) | Access::High(r) | Access::Whole(r) => Some(r),
            Access::Unused => None,
        }
    }

    /// What the access reads, `value` giving each register's value.
    pub fn read(&self, value: impl FnOnce(R) -> u64) -> u64 {
        match *self {
            Access::Word(r) | Access::Low(r) => value(r) & 0xffff_ffff,
            Access::High(r) => value(r) >> 32,
            Access::Whole(r) => value(r),
            Access::Unused => 0,
        }
    }

    /// A register that held `old`, after the access writes `val` (the low 4 bytes of it
    /// unless the access is [`Access::Whole`]): a half write keeps the other half, and a
    /// 32-bit register keeps only 32 bits.
    pub fn merge(&self, old: u64, val: u64) -> u64 {
        const LOW: u64 = 0xffff_ffff;
        match self {
            Access::Word(_) => val & LOW,
            Access::Low(_) => (old & !LOW) | (val & LOW),
            Access::High(_) => (old & LOW) | ((val & LOW) << 32),
            Access::Whole(_) => val,
            Access::Unused => old,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A word at 0, a 64-bit register at 8, nothing else.
    fn layout(off: u32) -> Field<char> {
        match off {
            0 => Field::Word('w'),
            8 => Field::Low('d'),
            0xc => Field::High('d'),
            _ => Field::Unused,
        }
    }

    #[test]
    fn widths() {
        let a = |off, size, wide| access(off, size, wide, layout);
        for wide in [false, true] {
            assert_eq!(a(0, 4, wide), Ok(Access::Word('w')));
            assert_eq!(a(8, 4, wide), Ok(Access::Low('d')));
            assert_eq!(a(0xc, 4, wide), Ok(Access::High('d')));
            assert_eq!(a(0x10, 4, wide), Ok(Access::Unused));
            assert_eq!(a(2, 4, wide), Ok(Access::Unused), "between registers");
            for size in [1, 2, 3, 16] {
                assert_eq!(a(0, size, wide), Err(BusFault));
            }
        }
        assert_eq!(a(8, 8, true), Ok(Access::Whole('d')));
        assert_eq!(a(0x10, 8, true), Ok(Access::Unused));
        assert_eq!(a(0, 8, true), Err(BusFault), "a word and unused space");
        assert_eq!(a(0xc, 8, true), Err(BusFault), "misaligned");
        // Version 1: no 8-byte access, of anything.
        assert_eq!(a(8, 8, false), Err(BusFault));
        assert_eq!(a(0x10, 8, false), Err(BusFault));
    }

    #[test]
    fn reads_and_merges() {
        let v = |_| 0x1122_3344_5566_7788;
        assert_eq!(Access::Word('w').read(v), 0x5566_7788);
        assert_eq!(Access::Low('d').read(v), 0x5566_7788);
        assert_eq!(Access::High('d').read(v), 0x1122_3344);
        assert_eq!(Access::Whole('d').read(v), 0x1122_3344_5566_7788);
        assert_eq!(Access::<char>::Unused.read(v), 0);

        let old = 0x1111_1111_2222_2222;
        let val = 0xaaaa_aaaa_bbbb_bbbb;
        assert_eq!(Access::Word('w').merge(old, val), 0xbbbb_bbbb);
        assert_eq!(Access::Low('d').merge(old, val), 0x1111_1111_bbbb_bbbb);
        assert_eq!(Access::High('d').merge(old, 0xbbbb_bbbb), 0xbbbb_bbbb_2222_2222);
        assert_eq!(Access::Whole('d').merge(old, val), val);
        assert_eq!(Access::<char>::Unused.merge(old, val), old);
        assert_eq!(Access::Whole('d').reg(), Some('d'));
    }
}
