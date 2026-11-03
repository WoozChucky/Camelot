//! Address translation (Sv32 on RV32 harts, Sv39 and Sv48 on RV64 ones) and the
//! load/store/fetch paths built on it.
//!
//! No TLB yet: every access in a paged mode walks the page table. That is slow
//! but simple, and it means `sfence.vma` has nothing to flush.
//!
//! The three modes share one walk ([`walk`]), driven by a [`Scheme`]: how many levels,
//! how wide a PTE is, how many bits of the virtual address index each level.
//!
//! The bus still takes 32-bit physical addresses and accesses of at most 4 bytes
//! (#147 widens it): a physical address above 4 GiB is an access fault, and an
//! 8-byte access to memory is made as two 4-byte ones, the low word first. Devices
//! refuse 8-byte accesses.

use super::csr::{SATP_MODE_SV39, SATP_MODE_SV48, SATP_RV64_MODE_SHIFT};
use super::{Hart, Mode};
use crate::bus::{Bus, BusFault};
use crate::trap::{Access, Trap};
use excalibur::csr::*;
use excalibur::Xlen;

const PTE_V: u64 = 1 << 0;
const PTE_R: u64 = 1 << 1;
const PTE_W: u64 = 1 << 2;
const PTE_X: u64 = 1 << 3;
const PTE_U: u64 = 1 << 4;
const PTE_A: u64 = 1 << 6;
const PTE_D: u64 = 1 << 7;

/// An Sv39 or Sv48 PTE's PPN, bits 53:10 (44 bits).
const PTE_PPN_SV48: u64 = (1 << 44) - 1;
/// Bits 63:54 of an Sv39 or Sv48 PTE: Svnapot's N, Svpbmt's PBMT and reserved bits,
/// none of which Excalibur implements, so they must be 0 (spec section 7, "Paging").
const PTE_RESERVED_SV48: u64 = 0x3ff << 54;

/// `satp`'s PPN field: bits 21:0 on RV32, 43:0 on RV64.
const SATP_PPN_SV32: u64 = (1 << 22) - 1;
const SATP_PPN_SV48: u64 = (1 << 44) - 1;

const PAGE_SHIFT: u32 = 12;

/// A paging mode's page-table shape.
#[derive(Clone, Copy)]
struct Scheme {
    /// Levels of tables: the walk starts at level `levels - 1`, at the root.
    levels: u32,
    /// Bytes per PTE.
    pte_size: u32,
    /// Bits of the virtual page number each level takes: 10 for Sv32's 1024-entry
    /// tables, 9 for Sv39's and Sv48's 512-entry ones.
    vpn_bits: u32,
    /// The PPN field of a PTE, once shifted down by 10.
    ppn_mask: u64,
    /// The bits of a PTE that must be 0.
    reserved: u64,
}

const SV32: Scheme = Scheme {
    levels: 2,
    pte_size: 4,
    vpn_bits: 10,
    ppn_mask: (1 << 22) - 1,
    reserved: 0,
};
const SV39: Scheme = Scheme {
    levels: 3,
    pte_size: 8,
    vpn_bits: 9,
    ppn_mask: PTE_PPN_SV48,
    reserved: PTE_RESERVED_SV48,
};
const SV48: Scheme = Scheme { levels: 4, ..SV39 };

impl Scheme {
    /// The virtual address bits the mode translates: 32, 39 or 48.
    fn va_bits(self) -> u32 {
        PAGE_SHIFT + self.levels * self.vpn_bits
    }

    /// Whether `vaddr` is canonical: Sv39's and Sv48's upper bits must all equal the
    /// top bit they translate (bits 63:39 equal to bit 38, 63:48 to bit 47). Sv32
    /// translates all 32 bits of an RV32 address, so there is nothing to check.
    fn canonical(self, vaddr: u64) -> bool {
        let unused = 64 - self.va_bits();
        unused == 32 || ((vaddr << unused) as i64 >> unused) as u64 == vaddr
    }
}

/// The leaf of a successful page-table walk.
struct Leaf {
    pte: u64,
    /// Where the PTE is, and its size, for the A/D update.
    addr: u64,
    size: u32,
    /// The physical address the walk translated to.
    paddr: u64,
}

/// Why a walk failed: an invalid or missing entry (a page fault), or a PTE outside
/// memory (an access fault).
enum WalkError {
    PageFault,
    AccessFault,
}

impl WalkError {
    fn trap(self, page_fault: Trap, access_fault: Trap) -> Trap {
        match self {
            WalkError::PageFault => page_fault,
            WalkError::AccessFault => access_fault,
        }
    }
}

/// Reads the `size`-byte (4 or 8) PTE at physical address `addr`. Page tables must live
/// in memory: a PTE address on a device, or above the bus's 4 GiB, is an access fault.
fn read_pte(bus: &Bus, addr: u64, size: u32) -> Result<u64, BusFault> {
    let a = bus_addr(addr, size)?;
    let lo = bus.read_mem(a, 4)?;
    if size == 4 {
        return Ok(lo.into());
    }
    let hi = bus.read_mem(a + 4, 4)?;
    Ok(u64::from(lo) | u64::from(hi) << 32)
}

/// Walks the page table rooted at physical page `root` for `vaddr`, down to its leaf,
/// as the privileged spec's algorithm does: no permission checks and no A/D update,
/// which are the caller's, and no canonical check (also the caller's, before).
///
/// At each level, from the root down, the walk indexes the table with that level's
/// slice of the virtual page number and reads the PTE. An entry with R or X set is a
/// leaf: at level 0 it maps a 4 KiB page, above it a superpage that covers everything
/// the levels below would have (Sv32: 4 MiB at level 1; Sv39 and Sv48: 2 MiB, 1 GiB,
/// 512 GiB at levels 1, 2, 3). A superpage's PPN must be aligned to its size, and the
/// virtual address's low bits pass through to the physical one.
fn walk(bus: &Bus, scheme: Scheme, root: u64, vaddr: u64) -> Result<Leaf, WalkError> {
    let vpn_mask = (1u64 << scheme.vpn_bits) - 1;
    let mut table = root << PAGE_SHIFT;
    let mut level = scheme.levels - 1;
    loop {
        // VPN[level] of the address: the bits above the page offset and the levels below.
        let shift = PAGE_SHIFT + level * scheme.vpn_bits;
        let vpn = (vaddr >> shift) & vpn_mask;
        let addr = table + vpn * u64::from(scheme.pte_size);
        let pte = read_pte(bus, addr, scheme.pte_size).map_err(|_| WalkError::AccessFault)?;
        // Invalid, writable but not readable (a reserved encoding), or a reserved bit set.
        if pte & PTE_V == 0 || (pte & PTE_R == 0 && pte & PTE_W != 0) || pte & scheme.reserved != 0 {
            return Err(WalkError::PageFault);
        }
        let ppn = (pte >> 10) & scheme.ppn_mask;
        if pte & (PTE_R | PTE_X) != 0 {
            // A leaf. The page covers 2^shift bytes: its PPN's low bits, those the levels
            // below would have supplied, must be 0.
            let offset = (1u64 << shift) - 1;
            if (ppn << PAGE_SHIFT) & offset != 0 {
                return Err(WalkError::PageFault);
            }
            let paddr = ppn << PAGE_SHIFT | (vaddr & offset);
            return Ok(Leaf {
                pte,
                addr,
                size: scheme.pte_size,
                paddr,
            });
        }
        // A pointer to the next level's table; at level 0 there is none.
        if level == 0 {
            return Err(WalkError::PageFault);
        }
        level -= 1;
        table = ppn << PAGE_SHIFT;
    }
}

/// The bus address of the `size` bytes at physical address `paddr`: the bus takes
/// 32-bit addresses (#147 widens it), so bytes above 4 GiB are unmapped.
fn bus_addr(paddr: u64, size: u32) -> Result<u32, BusFault> {
    let addr = u32::try_from(paddr).map_err(|_| BusFault)?;
    addr.checked_add(size - 1).ok_or(BusFault)?;
    Ok(addr)
}

/// Reads `size` (1, 2, 4 or 8) bytes at physical address `paddr`, zero-extended. 8
/// bytes are read only from RAM, VRAM or the boot ROM, as two words.
fn read_phys(bus: &mut Bus, paddr: u64, size: u32) -> Result<u64, BusFault> {
    let addr = bus_addr(paddr, size)?;
    if size < 8 {
        return bus.read(addr, size).map(u64::from);
    }
    // RAM or VRAM ([`Bus::is_memory`]), else the boot ROM (which `read_mem` reads too).
    let in_rom = || bus.read_mem(addr, 4).is_ok() && bus.read_mem(addr + 4, 4).is_ok();
    if !bus.is_memory(addr, 8) && !in_rom() {
        return Err(BusFault);
    }
    let lo = bus.read(addr, 4)?;
    let hi = bus.read(addr + 4, 4)?;
    Ok(u64::from(lo) | u64::from(hi) << 32)
}

impl Hart {
    /// Privilege used for data accesses: MPRV makes M-mode loads/stores act as MPP.
    fn data_mode(&self) -> Mode {
        let s = self.csr.mstatus;
        if self.mode == Mode::M && s & MSTATUS_MPRV != 0 {
            Mode::from_bits((s & MSTATUS_MPP) >> MSTATUS_MPP_SHIFT)
        } else {
            self.mode
        }
    }

    /// The paging mode `satp` selects, and its root table's page number; `None` for
    /// Bare. `satp`'s WARL rule ([`Hart::csr_store`]) leaves an RV64 hart only Bare,
    /// Sv39 (MODE 8) and Sv48 (9).
    fn paging(&self) -> Option<(Scheme, u64)> {
        let satp = self.csr.satp;
        match self.xlen {
            Xlen::Rv32 => (satp >> 31 != 0).then_some((SV32, satp & SATP_PPN_SV32)),
            Xlen::Rv64 => match satp >> SATP_RV64_MODE_SHIFT {
                SATP_MODE_SV39 => Some((SV39, satp & SATP_PPN_SV48)),
                SATP_MODE_SV48 => Some((SV48, satp & SATP_PPN_SV48)),
                _ => None,
            },
        }
    }

    /// Virtual to physical. Returns the physical address or the fault to raise.
    pub fn translate(&mut self, bus: &mut Bus, vaddr: u64, access: Access) -> Result<u64, Trap> {
        let mode = if access == Access::Fetch {
            self.mode
        } else {
            self.data_mode()
        };
        let Some((scheme, root)) = self.paging().filter(|_| mode != Mode::M) else {
            return Ok(vaddr);
        };

        let page_fault = Trap::new(access.page_fault(), vaddr);
        let access_fault = Trap::new(access.access_fault(), vaddr);
        // A non-canonical address faults before any table is read (spec section 6).
        if !scheme.canonical(vaddr) {
            return Err(page_fault);
        }
        let leaf = walk(bus, scheme, root, vaddr).map_err(|e| e.trap(page_fault, access_fault))?;
        let pte = leaf.pte;

        // Permission checks on the leaf.
        let status = self.csr.mstatus;
        let allowed = match access {
            Access::Fetch => pte & PTE_X != 0,
            Access::Load => pte & PTE_R != 0 || (status & MSTATUS_MXR != 0 && pte & PTE_X != 0),
            Access::Store => pte & PTE_W != 0,
        };
        if !allowed {
            return Err(page_fault);
        }
        let user_page = pte & PTE_U != 0;
        match mode {
            Mode::U if !user_page => return Err(page_fault),
            Mode::S if user_page && (access == Access::Fetch || status & MSTATUS_SUM == 0) => {
                return Err(page_fault)
            }
            _ => {}
        }

        // Hardware-managed accessed/dirty bits, written back as a whole PTE.
        let mut new_pte = pte | PTE_A;
        if access == Access::Store {
            new_pte |= PTE_D;
        }
        if new_pte != pte {
            self.write_phys(bus, leaf.addr, leaf.size, new_pte)
                .map_err(|_| access_fault)?;
        }
        Ok(leaf.paddr)
    }

    /// A debugger's view of `vaddr`: the physical address the hart's instruction fetches
    /// would use, in its current mode, with the hart's `satp`. Unlike [`Hart::translate`]
    /// it changes nothing (no A/D bits) and checks no permissions, so a debugger can read
    /// a page the hart could not (a U-mode page from S-mode, execute-only code). `None`
    /// when the page is not mapped or the address is not canonical.
    pub fn debug_translate(&self, bus: &Bus, vaddr: u64) -> Option<u64> {
        let Some((scheme, root)) = self.paging().filter(|_| self.mode != Mode::M) else {
            return Some(vaddr);
        };
        if !scheme.canonical(vaddr) {
            return None;
        }
        walk(bus, scheme, root, vaddr).ok().map(|leaf| leaf.paddr)
    }

    pub fn fetch(&mut self, bus: &mut Bus, pc: u64) -> Result<u32, Trap> {
        let paddr = self.translate(bus, pc, Access::Fetch)?;
        // Only memory is executable: fetching from a device is an access fault.
        bus_addr(paddr, 4)
            .and_then(|a| bus.read_mem(a, 4))
            .map_err(|_| Trap::new(Access::Fetch.access_fault(), pc))
    }

    /// Loads `size` bytes, zero-extended. Misaligned accesses trap (spec section 4).
    pub fn load(&mut self, bus: &mut Bus, vaddr: u64, size: u32) -> Result<u64, Trap> {
        if vaddr & (size as u64 - 1) != 0 {
            return Err(Trap::new(Access::Load.misaligned(), vaddr));
        }
        let paddr = self.translate(bus, vaddr, Access::Load)?;
        let v = read_phys(bus, paddr, size).map_err(|_| Trap::new(Access::Load.access_fault(), vaddr))?;
        bus.watches.note(self.id, vaddr as u32, size, true, false);
        Ok(v)
    }

    /// Stores the low `size` bytes of `val`.
    pub fn store(&mut self, bus: &mut Bus, vaddr: u64, size: u32, val: u64) -> Result<(), Trap> {
        if vaddr & (size as u64 - 1) != 0 {
            return Err(Trap::new(Access::Store.misaligned(), vaddr));
        }
        let paddr = self.translate(bus, vaddr, Access::Store)?;
        self.write_phys(bus, paddr, size, val)
            .map_err(|_| Trap::new(Access::Store.access_fault(), vaddr))?;
        bus.watches.note(self.id, vaddr as u32, size, false, true);
        Ok(())
    }

    /// Reads `size` bytes at physical address `paddr` (an atomic's, whose address
    /// [`Hart::atomic_paddr`] checked), zero-extended.
    pub(super) fn read_phys(&self, bus: &mut Bus, paddr: u64, size: u32) -> Result<u64, BusFault> {
        read_phys(bus, paddr, size)
    }

    /// Every write the hart makes goes through here (`sb`/`sh`/`sw`/`sd`, `sc`, AMOs,
    /// and the page walker's A/D updates), so the bus can keep the LR/SC reservations
    /// right ([`Bus::snoop_store`]): a write to a reserved line breaks that reservation,
    /// whichever hart holds it, and a store to a device breaks this hart's, since it may
    /// start a DMA into RAM (see [`super::Reservation`]). An A/D update counts because
    /// software may be updating that PTE with `lr`/`sc`, and its `sc` must not overwrite
    /// the bit the hardware just set. Only the low `size` bytes of `val` reach the bus
    /// (a device sees the byte an `sb` stored, not the whole register). 8 bytes are
    /// written only to RAM or VRAM.
    pub(super) fn write_phys(
        &mut self,
        bus: &mut Bus,
        paddr: u64,
        size: u32,
        val: u64,
    ) -> Result<(), BusFault> {
        let addr = bus_addr(paddr, size)?;
        if size < 8 {
            let low_bytes = (1u64 << (8 * size)) - 1;
            bus.write(addr, size, (val & low_bytes) as u32)?;
        } else if bus.is_memory(addr, 8) {
            bus.write(addr, 4, val as u32)?;
            bus.write(addr + 4, 4, (val >> 32) as u32)?;
        } else {
            return Err(BusFault);
        }
        bus.snoop_store(self.id as usize, addr, size);
        Ok(())
    }
}
