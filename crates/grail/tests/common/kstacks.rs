//! The pages the RV64 kernel has not lost, read from a machine's RAM: what
//! the tests that check every page comes back compare (Grail's network
//! tests through tests/common, and xtask's tests/smp_rv64.rs, which
//! includes this file).
//!
//! On RV64 a kernel stack is not a page of free RAM, as on RV32, but
//! KSTACK_SIZE (16 KiB, 4 pages) mapped in the vmalloc area (kernel/vm.c's
//! kstack_alloc()). A freed stack stays mapped: in its hart's cache
//! (`kstack_cached`), as Linux keeps NR_CACHED_STACKS freed thread stacks
//! per CPU, or on the list of those waiting for kstack_reclaim() to unmap
//! them (`kstack_deferred`). So `nfree` alone need not come back after a
//! process ends; the free pages and those of every mapped stack do. And a
//! stack no one freed would keep that total the same too, mapped but in use
//! by no thread: so the stacks in use, those mapped but neither cached nor
//! deferred (what kstack_count() reports), must come back as well.

use grail::Machine;

/// Where the RV64 kernel image is linked, and where it is loaded.
const KERNEL_LINK_ADDR: u64 = 0xffff_ffff_8000_0000;
const LOAD_ADDR: u64 = 0x8000_0000;
const PGSIZE: u64 = 4096;
/// A kernel stack's size (vm.h's KSTACK_SIZE): 4 pages.
const KSTACK_SIZE: u64 = 4 * PGSIZE;

/// The kernel's pages, as [`pages`] counts them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pages {
    /// The free pages (`nfree`, kernel/kalloc.c) and those of every mapped
    /// kernel stack: cached, deferred or in use.
    pub total: u64,
    /// The kernel stacks in use: mapped, neither cached nor deferred.
    pub stacks_in_use: u64,
}

/// The RV64 kernel `kernel` (its ELF file)'s pages in `m`'s RAM. Read at a
/// quiet moment (the shell waiting at its prompt), when no hart is
/// changing the stacks' state.
pub fn pages(m: &Machine, kernel: &[u8]) -> Pages {
    let symbols = elf::Elf::parse(kernel).unwrap().symbols().unwrap();
    let symbol = |name: &str| {
        let s = symbols
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("no symbol {name}"));
        (s.value - KERNEL_LINK_ADDR + LOAD_ADDR, s.size)
    };
    let read = |pa: u64, size: u32| m.cpu.bus.read_mem(pa, size).unwrap();

    let free = read(symbol("nfree").0, 4);
    // kstack_state: a byte per slot, KSTACK_FREE (0) for an unmapped one.
    let (state, slots) = symbol("kstack_state");
    let mapped = (0..slots).filter(|&i| read(state + i, 1) != 0).count() as u64;
    // kstack_cached[NCPU][NR_CACHED_STACKS]: a stack's address, or 0.
    let (cached_at, size) = symbol("kstack_cached");
    let cached = (0..size / 8).filter(|&i| read(cached_at + 8 * i, 8) != 0).count() as u64;
    // kstack_deferred: a list of stacks linked through each one's top word
    // (vm.c's NEXT_OF), at vmalloc addresses: read through the kernel's
    // page table.
    let satp = read(symbol("kernel_satp").0, 8);
    let mut deferred = 0;
    let mut s = read(symbol("kstack_deferred").0, 8);
    while s != 0 {
        deferred += 1;
        assert!(deferred <= slots, "kstack_deferred loops");
        s = read(translate(m, satp, s + KSTACK_SIZE - 8), 8);
    }
    assert!(
        cached + deferred <= mapped,
        "{cached} cached and {deferred} deferred of {mapped} mapped"
    );
    let stacks_in_use = mapped - cached - deferred;
    Pages {
        total: free + KSTACK_SIZE / PGSIZE * (cached + deferred + stacks_in_use),
        stacks_in_use,
    }
}

/// The physical address of kernel address `va` in the page table `satp`
/// names (Sv39 or Sv48, as the kernel chose at boot).
fn translate(m: &Machine, satp: u64, va: u64) -> u64 {
    let levels = match satp >> 60 {
        8 => 3,
        9 => 4,
        mode => panic!("satp mode {mode}"),
    };
    let mut table = (satp & ((1 << 44) - 1)) * PGSIZE;
    for level in (0..levels).rev() {
        let index = (va >> (12 + 9 * level)) & 511;
        let pte = m.cpu.bus.read_mem(table + 8 * index, 8).unwrap();
        assert!(pte & 1 != 0, "{va:#x} is not mapped");
        let pa = (pte >> 10 & ((1 << 44) - 1)) * PGSIZE;
        if pte & 0b1010 != 0 {
            // A leaf (R or X set), maybe a superpage.
            let offset = va & ((PGSIZE << (9 * level)) - 1);
            return pa + offset;
        }
        table = pa;
    }
    panic!("{va:#x}: no leaf");
}
