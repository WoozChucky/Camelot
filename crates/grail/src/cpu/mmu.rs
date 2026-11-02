//! Sv32 address translation and the load/store/fetch paths built on it.
//!
//! No TLB yet: every access in a paged mode walks the page table. That is slow
//! but simple, and it means `sfence.vma` has nothing to flush.

use super::{Hart, Mode};
use crate::bus::{Bus, BusFault};
use crate::trap::{Access, Trap};
use excalibur::csr::*;

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

    /// Virtual to physical. Returns the physical address or the fault to raise.
    pub fn translate(&mut self, bus: &mut Bus, vaddr: u32, access: Access) -> Result<u32, Trap> {
        let mode = if access == Access::Fetch {
            self.mode
        } else {
            self.data_mode()
        };
        let satp = self.csr.satp;
        if mode == Mode::M || satp >> 31 == 0 {
            return Ok(vaddr);
        }

        let page_fault = Trap::new(access.page_fault(), vaddr);
        let access_fault = Trap::new(access.access_fault(), vaddr);
        let leaf = walk(bus, satp, vaddr).map_err(|e| e.trap(page_fault, access_fault))?;
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
            self.write_phys(bus, leaf.addr, 4, new_pte)
                .map_err(|_| access_fault)?;
        }
        Ok(leaf.paddr)
    }

    /// A debugger's view of `vaddr`: the physical address the hart's instruction fetches
    /// would use, in its current mode, with the hart's `satp`. Unlike [`Hart::translate`]
    /// it changes nothing (no A/D bits) and checks no permissions, so a debugger can read
    /// a page the hart could not (a U-mode page from S-mode, execute-only code). `None`
    /// when the page is not mapped.
    pub fn debug_translate(&self, bus: &Bus, vaddr: u32) -> Option<u32> {
        if self.mode == Mode::M || self.csr.satp >> 31 == 0 {
            return Some(vaddr);
        }
        walk(bus, self.csr.satp, vaddr).ok().map(|leaf| leaf.paddr)
    }

    pub fn fetch(&mut self, bus: &mut Bus, pc: u32) -> Result<u32, Trap> {
        let paddr = self.translate(bus, pc, Access::Fetch)?;
        // Only memory is executable: fetching from a device is an access fault.
        bus.read_mem(paddr, 4)
            .map_err(|_| Trap::new(Access::Fetch.access_fault(), pc))
    }

    /// Loads `size` bytes. Misaligned accesses trap (spec section 4).
    pub fn load(&mut self, bus: &mut Bus, vaddr: u32, size: u32) -> Result<u32, Trap> {
        if vaddr & (size - 1) != 0 {
            return Err(Trap::new(Access::Load.misaligned(), vaddr));
        }
        let paddr = self.translate(bus, vaddr, Access::Load)?;
        let v = bus
            .read(paddr, size)
            .map_err(|_| Trap::new(Access::Load.access_fault(), vaddr))?;
        bus.watches.note(self.id, vaddr, size, true, false);
        Ok(v)
    }

    pub fn store(&mut self, bus: &mut Bus, vaddr: u32, size: u32, val: u32) -> Result<(), Trap> {
        if vaddr & (size - 1) != 0 {
            return Err(Trap::new(Access::Store.misaligned(), vaddr));
        }
        let paddr = self.translate(bus, vaddr, Access::Store)?;
        self.write_phys(bus, paddr, size, val)
            .map_err(|_| Trap::new(Access::Store.access_fault(), vaddr))?;
        bus.watches.note(self.id, vaddr, size, false, true);
        Ok(())
    }

    /// Every write the hart makes goes through here (`sb`/`sh`/`sw`, `sc.w`,
    /// AMOs, and the page walker's A/D updates), so the bus can keep the LR/SC
    /// reservations right ([`Bus::snoop_store`]): a write to a reserved word breaks
    /// that reservation, whichever hart holds it, and a store to a device breaks this
    /// hart's, since it may start a DMA into RAM (see [`super::Reservation`]). An A/D
    /// update counts because software may be updating that PTE with `lr.w`/`sc.w`,
    /// and its `sc.w` must not overwrite the bit the hardware just set.
    pub(super) fn write_phys(
        &mut self,
        bus: &mut Bus,
        paddr: u32,
        size: u32,
        val: u32,
    ) -> Result<(), BusFault> {
        bus.write(paddr, size, val)?;
        bus.snoop_store(self.id as usize, paddr, size);
        Ok(())
    }
}
