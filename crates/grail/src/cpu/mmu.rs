//! Sv32 address translation and the load/store/fetch paths built on it.
//!
//! No TLB yet: every access in a paged mode walks the page table. That is slow
//! but simple, and it means `sfence.vma` has nothing to flush.

use super::{Cpu, Mode};
use crate::bus::BusFault;
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

impl Cpu {
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
    pub fn translate(&mut self, vaddr: u32, access: Access) -> Result<u32, Trap> {
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
        let vpn = [(vaddr >> 12) & 0x3ff, vaddr >> 22];
        let mut table = (satp & 0x3f_ffff).wrapping_mul(PAGE_SIZE);
        let mut level = 1usize;

        let (pte, pte_addr) = loop {
            let pte_addr = table.wrapping_add(vpn[level] * 4);
            // Page tables must live in memory: a PTE address on a device is an access fault.
            let pte = self.bus.read_mem(pte_addr, 4).map_err(|_| access_fault)?;
            if pte & PTE_V == 0 || (pte & PTE_R == 0 && pte & PTE_W != 0) {
                return Err(page_fault);
            }
            if pte & (PTE_R | PTE_X) != 0 {
                break (pte, pte_addr);
            }
            if level == 0 {
                return Err(page_fault);
            }
            level -= 1;
            table = (pte >> 10).wrapping_mul(PAGE_SIZE);
        };

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
        // A 4 MiB superpage must be 4 MiB aligned.
        let ppn = pte >> 10;
        if level == 1 && ppn & 0x3ff != 0 {
            return Err(page_fault);
        }

        // Hardware-managed accessed/dirty bits.
        let mut new_pte = pte | PTE_A;
        if access == Access::Store {
            new_pte |= PTE_D;
        }
        if new_pte != pte {
            self.write_phys(pte_addr, 4, new_pte).map_err(|_| access_fault)?;
        }

        Ok(if level == 1 {
            (ppn << 12) & 0xffc0_0000 | (vaddr & 0x003f_ffff)
        } else {
            (ppn << 12) | (vaddr & 0xfff)
        })
    }

    pub fn fetch(&mut self, pc: u32) -> Result<u32, Trap> {
        let paddr = self.translate(pc, Access::Fetch)?;
        // Only memory is executable: fetching from a device is an access fault.
        self.bus
            .read_mem(paddr, 4)
            .map_err(|_| Trap::new(Access::Fetch.access_fault(), pc))
    }

    /// Loads `size` bytes. Misaligned accesses trap (spec section 4).
    pub fn load(&mut self, vaddr: u32, size: u32) -> Result<u32, Trap> {
        if vaddr & (size - 1) != 0 {
            return Err(Trap::new(Access::Load.misaligned(), vaddr));
        }
        let paddr = self.translate(vaddr, Access::Load)?;
        self.bus
            .read(paddr, size)
            .map_err(|_| Trap::new(Access::Load.access_fault(), vaddr))
    }

    pub fn store(&mut self, vaddr: u32, size: u32, val: u32) -> Result<(), Trap> {
        if vaddr & (size - 1) != 0 {
            return Err(Trap::new(Access::Store.misaligned(), vaddr));
        }
        let paddr = self.translate(vaddr, Access::Store)?;
        self.write_phys(paddr, size, val)
            .map_err(|_| Trap::new(Access::Store.access_fault(), vaddr))
    }

    /// Every write the hart makes goes through here (`sb`/`sh`/`sw`, `sc.w`,
    /// AMOs, and the page walker's A/D updates), so the LR/SC reservation sees
    /// it: a write to the reserved word breaks the reservation, and so does any
    /// store to a device, which may start a DMA into RAM (see
    /// [`super::Reservation`]). An A/D update counts because software may be
    /// updating that PTE with `lr.w`/`sc.w`, and its `sc.w` must not overwrite
    /// the bit the hardware just set.
    pub(super) fn write_phys(&mut self, paddr: u32, size: u32, val: u32) -> Result<(), BusFault> {
        self.bus.write(paddr, size, val)?;
        if self.reservation.held().is_some() {
            if self.bus.is_memory(paddr, size) {
                self.reservation.snoop(paddr, size);
            } else {
                self.reservation.clear();
            }
        }
        Ok(())
    }
}
