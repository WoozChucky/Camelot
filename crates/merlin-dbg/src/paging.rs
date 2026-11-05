//! Page tables, walked by the debugger: how a virtual address of an address space that
//! is not the hart's own (another process's) becomes a physical one, as OpenOCD's
//! RISC-V target walks them for `virt2phys`. The stub translates only through the
//! selected hart's `satp`; merlin-dbg reads the other tables itself, over physical
//! memory (QEMU's `Qqemu.PhyMemMode`, [`crate::remote::Remote::physical`]).
//!
//! The walk is the privileged spec's (section 12.3.2, "Virtual Address Translation
//! Process"), for the three modes Excalibur has (spec section 7, "Paging"): Sv32 on
//! RV32 harts (two levels of 1024 four-byte PTEs, a 22-bit PPN, so physical addresses
//! of 34 bits), Sv39 and Sv48 on RV64 ones (three or four levels of 512 eight-byte
//! PTEs, a 44-bit PPN, and virtual addresses that must be canonical: their upper bits
//! all equal to the top one translated). A debugger checks no permission, as Grail's
//! debug reads do not: a user page reads from anywhere.

use excalibur::Xlen;

const PAGE_SHIFT: u32 = 12;
const PTE_V: u64 = 1 << 0;
const PTE_R: u64 = 1 << 1;
const PTE_W: u64 = 1 << 2;
const PTE_X: u64 = 1 << 3;
/// A pointer PTE's A, D and U bits are reserved, and a walk faults on them.
const POINTER_RESERVED: u64 = 1 << 4 | 1 << 6 | 1 << 7;
/// Bits 63:54 of an RV64 PTE (Svnapot, Svpbmt, reserved), none of which Excalibur
/// has: a PTE with any of them set faults.
const RV64_RESERVED: u64 = 0x3ff << 54;

/// The shape of a paging mode's tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    pub name: &'static str,
    levels: u32,
    pte_size: usize,
    vpn_bits: u32,
    ppn_mask: u64,
    reserved: u64,
    canonical: bool,
}

pub const SV32: Mode = Mode {
    name: "Sv32",
    levels: 2,
    pte_size: 4,
    vpn_bits: 10,
    ppn_mask: (1 << 22) - 1,
    reserved: 0,
    canonical: false,
};
pub const SV39: Mode = Mode {
    name: "Sv39",
    levels: 3,
    pte_size: 8,
    vpn_bits: 9,
    ppn_mask: (1 << 44) - 1,
    reserved: RV64_RESERVED,
    canonical: true,
};
pub const SV48: Mode = Mode {
    name: "Sv48",
    levels: 4,
    ..SV39
};

/// What `satp` says: no translation (Bare), or a mode and the physical address of its
/// root table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Space {
    Bare,
    Paged { mode: Mode, root: u64 },
}

/// The address space a `satp` value of an `xlen` hart names. An error for a mode
/// Excalibur does not have (RV64's Sv57, or a reserved one).
pub fn space(satp: u64, xlen: Xlen) -> Result<Space, String> {
    let (mode, ppn) = match xlen {
        Xlen::Rv32 => (
            if satp & 1 << 31 != 0 { Some(SV32) } else { None },
            satp & ((1 << 22) - 1),
        ),
        Xlen::Rv64 => {
            let mode = match satp >> 60 {
                0 => None,
                8 => Some(SV39),
                9 => Some(SV48),
                m => {
                    return Err(format!(
                        "satp {satp:#x}: paging mode {m}, which Excalibur does not have"
                    ))
                }
            };
            (mode, satp & ((1 << 44) - 1))
        }
    };
    Ok(match mode {
        None => Space::Bare,
        Some(mode) => Space::Paged {
            mode,
            root: ppn << PAGE_SHIFT,
        },
    })
}

/// The `satp` value that names `root`'s table in the same mode as `like` (the hart's
/// own `satp`): what the hart's `satp` holds while it runs on that table.
pub fn satp_for(like: u64, root: u64, xlen: Xlen) -> u64 {
    match xlen {
        Xlen::Rv32 => (like & 1 << 31) | (root >> PAGE_SHIFT) & ((1 << 22) - 1),
        Xlen::Rv64 => (like & 0xf << 60) | (root >> PAGE_SHIFT) & ((1 << 44) - 1),
    }
}

/// The physical address of `va` in `space`, reading each PTE with `read_pte` (its
/// physical address and size; `None` where nothing can be read). `None` for an address
/// the space does not map: an invalid or missing PTE, a reserved encoding, a misaligned
/// superpage, an address that is not canonical.
pub fn translate(space: Space, va: u64, read_pte: &mut impl FnMut(u64, usize) -> Option<u64>) -> Option<u64> {
    let Space::Paged { mode, root } = space else {
        return Some(va);
    };
    let va_bits = PAGE_SHIFT + mode.levels * mode.vpn_bits;
    if mode.canonical {
        let unused = 64 - va_bits;
        if ((va << unused) as i64 >> unused) as u64 != va {
            return None;
        }
    } else if va >> va_bits != 0 {
        return None;
    }
    let vpn_mask = (1u64 << mode.vpn_bits) - 1;
    let mut table = root;
    let mut level = mode.levels - 1;
    loop {
        let shift = PAGE_SHIFT + level * mode.vpn_bits;
        let entry = table + ((va >> shift) & vpn_mask) * mode.pte_size as u64;
        let pte = read_pte(entry, mode.pte_size)?;
        if pte & PTE_V == 0 || (pte & PTE_R == 0 && pte & PTE_W != 0) || pte & mode.reserved != 0 {
            return None;
        }
        let ppn = (pte >> 10) & mode.ppn_mask;
        if pte & (PTE_R | PTE_X) != 0 {
            // A leaf: a page of 2^shift bytes, whose PPN's low bits must be 0.
            let offset = (1u64 << shift) - 1;
            if (ppn << PAGE_SHIFT) & offset != 0 {
                return None;
            }
            return Some(ppn << PAGE_SHIFT | va & offset);
        }
        if level == 0 || pte & POINTER_RESERVED != 0 {
            return None;
        }
        level -= 1;
        table = ppn << PAGE_SHIFT;
    }
}

/// The size of the page at a virtual address, for reads cut at page boundaries: 4 KiB,
/// the smallest (a superpage is read a 4 KiB piece at a time).
pub const PAGE_SIZE: u64 = 1 << PAGE_SHIFT;

#[cfg(test)]
mod tests {
    use super::*;
    use grail::debug::Space as GrailSpace;
    use grail::{Machine, MachineConfig};

    const RAM: u64 = 0x8000_0000;

    /// A little xorshift, for random tables the same every run.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    fn machine(xlen: Xlen) -> Machine {
        Machine::new(MachineConfig {
            xlen,
            ram_size: 16 << 20,
            ..MachineConfig::default()
        })
        .unwrap()
    }

    fn pte_read(m: &Machine) -> impl FnMut(u64, usize) -> Option<u64> + '_ {
        |addr, size| {
            let mut b = [0u8; 8];
            (m.read_memory(addr, &mut b[..size], GrailSpace::Physical) == size).then(|| u64::from_le_bytes(b))
        }
    }

    /// Fills the tables of a mode with random entries: pointers to further tables,
    /// leaves at every level (aligned or not), invalid and reserved encodings; then
    /// translates random addresses (in the mapped tables' ranges and anywhere) with
    /// this walk and with Grail's, the machine's own, which must agree.
    fn agrees_with_grail(xlen: Xlen, satp_mode: u64, mode: Mode) {
        let mut m = machine(xlen);
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ satp_mode);
        let tables = 24u64;
        let table_at = |i: u64| RAM + 0x10_0000 + i * 0x1000;
        let entries = 4096 / mode.pte_size as u64;
        for t in 0..tables {
            for e in 0..entries {
                let r = rng.next();
                let flags = match r % 8 {
                    0 | 1 => 0b0001,      // a pointer
                    2 => 0b1011,          // R X leaf
                    3 => 0b0111,          // R W leaf
                    4 => 0b0101,          // W without R: reserved
                    5 => 0b1001 | 1 << 4, // X leaf, U
                    6 => 0b0001 | 1 << 6, // a pointer with A: reserved
                    _ => 0,               // invalid
                };
                let ppn = if flags & 0b1110 == 0 {
                    table_at(rng.next() % tables) >> 12
                } else if r.is_multiple_of(3) {
                    // Misaligned for a superpage, often.
                    (RAM >> 12) + rng.next() % 4096
                } else {
                    (RAM >> 12) + (rng.next() % 8) * (1 << (mode.vpn_bits * 2))
                };
                let mut pte = ppn << 10 | flags;
                if mode.pte_size == 8 && r.is_multiple_of(29) {
                    pte |= 1 << 60; // reserved bits of an RV64 PTE
                }
                let bytes = pte.to_le_bytes();
                m.write_memory(
                    table_at(t) + e * mode.pte_size as u64,
                    &bytes[..mode.pte_size],
                    GrailSpace::Physical,
                );
            }
        }
        let satp = satp_mode | (table_at(0) >> 12);
        assert!(m.set_csr(0x180, satp));
        m.set_mode(grail::cpu::Mode::S);
        let space = space(satp, xlen).unwrap();
        assert_eq!(
            space,
            Space::Paged {
                mode,
                root: table_at(0)
            }
        );
        assert_eq!(satp_for(satp, table_at(3), xlen), satp_mode | table_at(3) >> 12);
        let va_bits = 12 + mode.levels * mode.vpn_bits;
        let (mut mapped, mut unmapped) = (0, 0);
        for i in 0..20_000u64 {
            let mut va = rng.next();
            if i % 2 == 0 {
                va &= (1u64 << va_bits) - 1;
                if mode.canonical && va >> (va_bits - 1) & 1 != 0 {
                    va |= !0 << va_bits;
                }
            }
            if xlen == Xlen::Rv32 {
                va &= 0xffff_ffff;
            }
            let ours = translate(space, va, &mut pte_read(&m));
            assert_eq!(
                ours,
                m.translate(va, GrailSpace::Virtual),
                "{} va {va:#x}",
                mode.name
            );
            if ours.is_some() {
                mapped += 1;
            } else {
                unmapped += 1;
            }
        }
        assert!(
            mapped > 300 && unmapped > 1000,
            "{} {mapped} {unmapped}",
            mode.name
        );
    }

    #[test]
    fn the_walk_is_the_machines() {
        agrees_with_grail(Xlen::Rv32, 1 << 31, SV32);
        agrees_with_grail(Xlen::Rv64, 8 << 60, SV39);
        agrees_with_grail(Xlen::Rv64, 9 << 60, SV48);
    }

    #[test]
    fn satp_values() {
        assert_eq!(space(0, Xlen::Rv32).unwrap(), Space::Bare);
        assert_eq!(
            space(0x8008_0123, Xlen::Rv32).unwrap(),
            Space::Paged {
                mode: SV32,
                root: 0x8012_3000,
            }
        );
        // Sv32's 22-bit PPN reaches past 4 GiB.
        assert_eq!(
            space(0x803f_ffff, Xlen::Rv32).unwrap(),
            Space::Paged {
                mode: SV32,
                root: 0x3_ffff_f000,
            }
        );
        assert_eq!(
            space(9 << 60 | 0x80123, Xlen::Rv64).unwrap(),
            Space::Paged {
                mode: SV48,
                root: 0x8012_3000,
            }
        );
        assert_eq!(space(0x80123, Xlen::Rv64).unwrap(), Space::Bare);
        assert!(space(10 << 60, Xlen::Rv64).is_err(), "Sv57");
        assert_eq!(translate(Space::Bare, 0x1234, &mut |_, _| None), Some(0x1234));
    }
}
