//! The pages the kernel has not lost, read from a machine's RAM: what the
//! tests that check every page comes back compare (Grail's network tests
//! through tests/common, and xtask's tests/smp.rs, which includes this
//! file).
//!
//! A kernel stack is not a page of free RAM (as it was on RV32, until #159)
//! but KSTACK_SIZE (16 KiB, 4 pages) mapped in the vmalloc area (kernel/vm.c's
//! kstack_alloc()). A freed stack stays mapped: in its hart's cache
//! (`kstack_cached`), as Linux keeps NR_CACHED_STACKS freed thread stacks
//! per CPU, or on the list of those waiting for kstack_reclaim() to unmap
//! them (`kstack_deferred`). So `nfree` alone need not come back after a
//! process ends; the free pages and those of every mapped stack do. And a
//! stack no one freed would keep that total the same too, mapped but in use
//! by no thread: so the stacks in use, those mapped but neither cached nor
//! deferred (what kstack_count() reports), must come back as well.
//!
//! Those counts agree only between the kernel's changes to a stack's state.
//! kstack_reclaim() takes the deferred stacks off their list, marks their
//! slots KSTACK_BUSY and then unmaps each one, kfree()ing its pages before
//! it marks the slot KSTACK_FREE: in between, the pages are in `nfree` and
//! still counted as the slot's, 4 too many (kstack_alloc() is the same the
//! other way round). The shell's prompt is no such moment: the hart that
//! ran a process's last thread reclaims its stack after switching away from
//! it, which on several harts may be after the shell, on another, has
//! printed the prompt. So [`pages`] first lets the machine run until every
//! hart idles in wfi, as Linux's accounting of a freed task stack settles
//! once the work that vfree()s it has run.

use grail::Machine;

/// Where the kernel image is linked, and where it is loaded: after the
/// firmware's 2 MiB.
const KERNEL_LINK_ADDR: u64 = 0xffff_ffff_8000_0000;
const LOAD_ADDR: u64 = 0x8020_0000;
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

/// Steps for every hart to finish what it is doing and idle ([`settle`]):
/// a few unmappings with their shootdowns, far fewer than this.
const SETTLE_MAX_STEPS: u64 = 10_000_000;

/// Steps `m` until every hart waits in wfi: the kernel's idle loop, the only
/// place it waits while it runs (its other wfi is a halt). Then no hart is
/// between two changes to a stack's state, nor to anything else.
fn settle(m: &mut Machine) {
    for _ in 0..SETTLE_MAX_STEPS {
        if m.cpu.harts().iter().all(|h| h.waiting) {
            return;
        }
        m.cpu.step();
    }
    panic!("the harts never all idled");
}

/// The RV64 kernel `kernel` (its ELF file)'s pages in `m`'s RAM, once its
/// harts have settled ([`settle`]): `m` runs on until they all idle.
pub fn pages(m: &mut Machine, kernel: &[u8]) -> Pages {
    settle(m);
    let m = &*m;
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
    // kstack_state: a byte per slot, KSTACK_FREE (0) for an unmapped one,
    // KSTACK_BUSY (2) for one being mapped or unmapped, which no slot is
    // once the harts have settled.
    let (state, slots) = symbol("kstack_state");
    let busy: Vec<u64> = (0..slots).filter(|&i| read(state + i, 1) == 2).collect();
    assert!(
        busy.is_empty(),
        "kernel stack slots {busy:?} busy with every hart idle"
    );
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
