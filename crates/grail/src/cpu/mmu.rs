//! Sv32 address translation and the load/store/fetch paths built on it.
//!
//! No TLB yet: every access in a paged mode walks the page table. That is slow
//! but simple, and it means `sfence.vma` has nothing to flush.
//!
//! An RV64 hart's `satp` holds Bare only (its WARL rule, [`Hart::csr_store`]) until
//! #146 adds the Sv39 and Sv48 walks, so it never translates.
//!
//! The bus still takes 32-bit physical addresses and accesses of at most 4 bytes
//! (#147 widens it): a physical address above 4 GiB is an access fault, and an
//! 8-byte access to memory is made as two 4-byte ones, the low word first. Devices
//! refuse 8-byte accesses.

use super::{Hart, Mode};
use crate::bus::{Bus, BusFault};
use crate::trap::{Access, Trap};
use excalibur::csr::*;
use excalibur::Xlen;

const PTE_V: u32 = 1 << 0;
const PTE_R: u32 = 1 << 1;
const PTE_W: u32 = 1 << 2;
const PTE_X: u32 = 1 << 3;
const PTE_U: u32 = 1 << 4;
const PTE_A: u32 = 1 << 6;
const PTE_D: u32 = 1 << 7;

const PAGE_SIZE: u32 = 4096;

/// The leaf of a successful Sv32 page-table walk.
struct Leaf {
    pte: u32,
    /// Where the PTE is, for the A/D update.
    addr: u32,
    /// The physical address the walk translated to.
    paddr: u32,
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

/// Walks the Sv32 page table `satp` points at for `vaddr`, down to its leaf: no
/// permission checks and no A/D update, which are the caller's.
fn walk(bus: &Bus, satp: u32, vaddr: u32) -> Result<Leaf, WalkError> {
    let vpn = [(vaddr >> 12) & 0x3ff, vaddr >> 22];
    let mut table = (satp & 0x3f_ffff).wrapping_mul(PAGE_SIZE);
    let mut level = 1usize;
    let (pte, addr) = loop {
        let pte_addr = table.wrapping_add(vpn[level] * 4);
        // Page tables must live in memory: a PTE address on a device is an access fault.
        let pte = bus.read_mem(pte_addr, 4).map_err(|_| WalkError::AccessFault)?;
        if pte & PTE_V == 0 || (pte & PTE_R == 0 && pte & PTE_W != 0) {
            return Err(WalkError::PageFault);
        }
        if pte & (PTE_R | PTE_X) != 0 {
            break (pte, pte_addr);
        }
        if level == 0 {
            return Err(WalkError::PageFault);
        }
        level -= 1;
        table = (pte >> 10).wrapping_mul(PAGE_SIZE);
    };
    // A 4 MiB superpage must be 4 MiB aligned.
    let ppn = pte >> 10;
    if level == 1 && ppn & 0x3ff != 0 {
        return Err(WalkError::PageFault);
    }
    let paddr = if level == 1 {
        (ppn << 12) & 0xffc0_0000 | (vaddr & 0x003f_ffff)
    } else {
        (ppn << 12) | (vaddr & 0xfff)
    };
    Ok(Leaf { pte, addr, paddr })
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
    // RAM or VRAM ([`Bus::is_memory`]), or RAM or the boot ROM (what `read_mem` reads).
    let ram_or_rom = bus.read_mem(addr, 4).is_ok() && bus.read_mem(addr + 4, 4).is_ok();
    if !bus.is_memory(addr, 8) && !ram_or_rom {
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

    /// Whether `satp` turns paging on: MODE not Bare.
    fn paging(&self) -> bool {
        match self.xlen {
            Xlen::Rv32 => self.csr.satp >> 31 != 0,
            Xlen::Rv64 => self.csr.satp >> 60 != 0,
        }
    }

    /// Virtual to physical. Returns the physical address or the fault to raise.
    pub fn translate(&mut self, bus: &mut Bus, vaddr: u64, access: Access) -> Result<u64, Trap> {
        let mode = if access == Access::Fetch {
            self.mode
        } else {
            self.data_mode()
        };
        if mode == Mode::M || !self.paging() {
            return Ok(vaddr);
        }

        let page_fault = Trap::new(access.page_fault(), vaddr);
        let access_fault = Trap::new(access.access_fault(), vaddr);
        if self.xlen == Xlen::Rv64 {
            // satp's WARL rule keeps an RV64 hart in Bare until #146 (only a debugger
            // writing `csr.satp` directly gets here): no page is mapped.
            return Err(page_fault);
        }
        let leaf =
            walk(bus, self.csr.satp as u32, vaddr as u32).map_err(|e| e.trap(page_fault, access_fault))?;
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

        // Hardware-managed accessed/dirty bits.
        let mut new_pte = pte | PTE_A;
        if access == Access::Store {
            new_pte |= PTE_D;
        }
        if new_pte != pte {
            self.write_phys(bus, leaf.addr.into(), 4, new_pte.into())
                .map_err(|_| access_fault)?;
        }
        Ok(leaf.paddr.into())
    }

    /// A debugger's view of `vaddr`: the physical address the hart's instruction fetches
    /// would use, in its current mode, with the hart's `satp`. Unlike [`Hart::translate`]
    /// it changes nothing (no A/D bits) and checks no permissions, so a debugger can read
    /// a page the hart could not (a U-mode page from S-mode, execute-only code). `None`
    /// when the page is not mapped.
    pub fn debug_translate(&self, bus: &Bus, vaddr: u64) -> Option<u64> {
        if self.mode == Mode::M || !self.paging() {
            return Some(vaddr);
        }
        if self.xlen == Xlen::Rv64 {
            return None;
        }
        walk(bus, self.csr.satp as u32, vaddr as u32)
            .ok()
            .map(|leaf| leaf.paddr.into())
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
