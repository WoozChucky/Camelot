/* Page tables and the kernel address space (include/vm.h).
 *
 * RV32 (Sv32): physical memory is identity mapped in the kernel page table,
 * so a physical address (a page table, a page from kalloc) is also a usable
 * pointer with paging on.
 *
 * RV64 (Sv48, or Sv39 on a hart without Sv48): a higher-half kernel in
 * Linux's layout. The kernel starts with paging off at its physical address
 * (entry.S); setup_vm() builds the early page table that maps the image at
 * both addresses, entry.S turns paging on and jumps to the high ones, and
 * kvminit() builds the final table, which has no identity mapping: from
 * then on a physical address is reached through the direct map, va(), and
 * a kernel pointer handed to the hardware goes through pa().
 *
 * User copies and threads. copyin() and friends find a user page through
 * the page table and then copy through the kernel's mapping of its physical
 * page. A process's threads share its page table, so one thread's sbrk() may
 * unmap and free a page that another, on another hart, is copying to or
 * from: once freed and reused, the copy would write someone else's memory.
 * uvm_lock closes that gap: every user copy checks the range and copies
 * under it, and unmap_pages() takes it once, after clearing the PTEs and
 * before freeing anything, so a copy that saw the old PTEs has finished by
 * then and every later one sees them gone. It is one lock for every
 * process, held for at most a page or so of memcpy, and a leaf of the lock
 * order: nothing else is taken under it. */
#include "vm.h"
#include "kalloc.h"
#include "kprintf.h"
#include "machine.h"
#include "memlayout.h"
#include "mmio.h"
#include "proc.h"
#include "riscv.h"
#include "smp.h"
#include "spinlock.h"
#include "string.h"
#include "sysinfo.h"
#include "trap.h"
#include "uart.h"

#if __riscv_xlen == 64
/* The largest leaf the kernel makes: 1 GiB (level 2), as Linux's direct map
 * uses PUD leaves; it never makes 512 GiB ones. */
#define MAX_LEAF_LEVEL 2
#define SUPERPAGE "superpage"
#else
/* A 4 MiB megapage, in the root. */
#define MAX_LEAF_LEVEL 1
#define SUPERPAGE "megapage"
#endif

pagetable_t kernel_pagetable;

/* Every user copy, against unmap_pages() (above). */
static struct spinlock uvm_lock;

#if __riscv_xlen == 64
/* The vmalloc area's state, under vmalloc_lock, which also keeps two harts
 * from making the same table under its root entry at once (Linux's
 * init_mm.page_table_lock): a leaf of the lock order but for kalloc's.
 * The kernel stacks' (kstack_alloc()):
 * - kstack_state: each slot's, KSTACK_FREE (unmapped, free to take),
 *   KSTACK_MAPPED (its stack is mapped: in use, cached or deferred) or
 *   KSTACK_BUSY (being mapped or unmapped, not to be read);
 * - kstack_cached: each hart's freed stacks, kept mapped for its next
 *   kstack_alloc(), NR_CACHED_STACKS at most, 0 for none (Linux's
 *   cached_stacks);
 * - kstack_deferred: the other freed stacks, each holding the next one's
 *   address in its top word, until kstack_reclaim() unmaps them;
 * - kstack_deepest: the deepest use of a stack that has been unmapped.
 * ioremap_next: where the next device mapping may go. */
static struct spinlock vmalloc_lock;
#define KSTACK_FREE 0
#define KSTACK_MAPPED 1
#define KSTACK_BUSY 2
static u8 kstack_state[KSTACK_SLOTS];
static uintptr kstack_cached[NCPU][NR_CACHED_STACKS];
static volatile uintptr kstack_deferred;
static usize kstack_deepest;
static uintptr ioremap_next;
/* Enough slots for every stack there can be at once: one per thread and
 * per other hart, the harts' caches, and some freed ones waiting to be
 * unmapped (kstack_alloc() returns NULL if they are all taken). */
_Static_assert(KSTACK_SLOTS >= NTHREAD + NCPU + NCPU * NR_CACHED_STACKS + NCPU,
               "a slot for every kernel stack");
#endif

#if __riscv_xlen == 64
u32 pt_levels;
uintptr satp_mode;
uintptr page_offset;
uintptr vmalloc_start;
uintptr fixaddr_top;
u64 direct_map_size;
uintptr va_pa_offset;
uintptr va_kernel_pa_offset;
uintptr user_top;
uintptr early_satp;
uintptr kernel_satp;
/* The early page table and the pool of EARLY_TABLES (vm.h) its lower tables
 * come from, in .bss (entry.S, page aligned). */
extern pte_t early_pg_dir[];
extern pte_t early_tables[];
#endif

#if __riscv_xlen == 64
phys_addr_t kernel_pa(uintptr v) {
    if (v >= KERNEL_LINK_ADDR && v <= (uintptr)__kernel_end) {
        return v - va_kernel_pa_offset;
    }
    if (v >= PAGE_OFFSET && v - PAGE_OFFSET < direct_map_size) {
        return v - va_pa_offset;
    }
    kprintf("pa: %p is neither in the direct map nor in the kernel image\n", (void *)v);
    panic("pa: no physical address");
    return 0; /* not reached: panic() stops the machine */
}
#endif

pagetable_t pagetable_create(void) {
    return kzalloc();
}

/* The PTE that maps `va` at `level` in `pt`, or the superpage leaf above
 * `level` that maps it, with the level of the PTE returned in *at. With
 * `alloc`, the tables down to `level` are created as needed; NULL if that
 * needs a page and none is free, or (without `alloc`) if one is missing. */
static pte_t *lookup(pagetable_t pt, uintptr va, int level, bool alloc, int *at) {
    for (int l = PT_LEVELS - 1;; l--) {
        pte_t *pte = &pt[VPN(va, l)];
        if (l == level || ((*pte & PTE_V) != 0 && (*pte & PTE_LEAF) != 0)) {
            *at = l;
            return pte;
        }
        if (*pte & PTE_V) {
            pt = (pagetable_t)va(PTE2PA(*pte));
        } else {
            if (!alloc) {
                return NULL;
            }
            pagetable_t table = pagetable_create();
            if (table == NULL) {
                return NULL;
            }
            /* A pointer to the next level: valid, no R/W/X. */
            *pte = PA2PTE(pa(table)) | PTE_V;
            pt = table;
        }
    }
}

pte_t *walk(pagetable_t pt, uintptr va, bool alloc) {
    int at;
    pte_t *pte = lookup(pt, va, 0, alloc, &at);
    if (pte != NULL && at != 0) {
        panic("walk: unexpected " SUPERPAGE);
    }
    return pte;
}

int map_pages(pagetable_t pt, uintptr va, phys_addr_t pa, usize size, u32 perm) {
    if (va % PGSIZE != 0 || pa % PGSIZE != 0 || size % PGSIZE != 0 || size == 0) {
        panic("map_pages: unaligned range");
    }
    /* W without R is a reserved encoding; no R/W/X would be a table pointer.
     * A, D and G come with a kernel mapping (PTE_KERNEL), never with U. */
    if ((perm & PTE_LEAF) == 0 || (perm & ~(PTE_PERM_MASK | PTE_KERNEL)) != 0 ||
        (perm & (PTE_R | PTE_W)) == PTE_W || ((perm & PTE_U) != 0 && (perm & PTE_KERNEL) != 0)) {
        panic("map_pages: bad permissions");
    }
    /* The last page, computed so that a range ending at the top of the
     * address space doesn't wrap. */
    uintptr last = va + (size - PGSIZE);
    if (last < va) {
        panic("map_pages: range wraps around");
    }
    for (uintptr a = va;; a += PGSIZE, pa += PGSIZE) {
        pte_t *pte = walk(pt, a, true);
        if (pte == NULL) {
            return -1;
        }
        if (*pte & PTE_V) {
            kprintf("map_pages: %p is already mapped\n", (void *)a);
            panic("map_pages: remap");
        }
        *pte = PA2PTE(pa) | perm | PTE_V;
        if (a == last) {
            return 0;
        }
    }
}

void unmap_pages(pagetable_t pt, uintptr va, usize size, bool free_pages) {
    if (va % PGSIZE != 0 || size % PGSIZE != 0 || size == 0) {
        panic("unmap_pages: unaligned range");
    }
    uintptr last = va + (size - PGSIZE);
    if (last < va) {
        panic("unmap_pages: range wraps around");
    }
    /* Two passes. First the mappings go: a PTE without V translates
     * nothing, but keeps the page's address for the second pass. */
    for (uintptr a = va;; a += PGSIZE) {
        pte_t *pte = walk(pt, a, false);
        if (pte == NULL || (*pte & PTE_V) == 0) {
            kprintf("unmap_pages: %p is not mapped\n", (void *)a);
            panic("unmap_pages: not mapped");
        }
        *pte = *pte & ~(pte_t)PTE_V;
        if (a == last) {
            break;
        }
    }
    /* Then no hart may still hold a translation to one of the pages in its
     * TLB (smp.h), nor be copying through one of them for a system call
     * (uvm_lock, above), before they can be reused. */
    tlb_shootdown(pt);
    acquire(&uvm_lock);
    release(&uvm_lock);
    for (uintptr a = va;; a += PGSIZE) {
        pte_t *pte = walk(pt, a, false);
        if (free_pages) {
            kfree(va(PTE2PA(*pte)));
        }
        *pte = 0;
        if (a == last) {
            return;
        }
    }
}

/* Frees `table`, a table at `level` (above 0), and the tables under it,
 * which must map nothing any more. */
static void table_free(pagetable_t table, int level) {
    for (u32 i = 0; i < PTES_PER_TABLE; i++) {
        pte_t pte = table[i];
        if ((pte & PTE_V) == 0) {
            continue;
        }
        if (pte & PTE_LEAF) {
            panic("pagetable_free: " SUPERPAGE " still mapped");
        }
        pagetable_t next = (pagetable_t)va(PTE2PA(pte));
        if (level == 1) {
            for (u32 j = 0; j < PTES_PER_TABLE; j++) {
                if (next[j] & PTE_V) {
                    panic("pagetable_free: page still mapped");
                }
            }
            kfree(next);
        } else {
            table_free(next, level - 1);
        }
    }
    kfree(table);
}

void pagetable_free(pagetable_t pt) {
    table_free(pt, PT_LEVELS - 1);
}

/* Maps [va, va + size) to the physical range at `pa` in the kernel page
 * table, leaving any page already mapped as it is (two devices may share a
 * page). Where a whole superpage is aligned in both, fits in what is left
 * and is still free in the table, one leaf maps it: one PTE instead of a
 * table of 512 (or 1024): mapping the 64 MiB PLIC page by page would be
 * 16384 PTEs, most of a boot. `size` is a whole number of pages, `perm` a
 * PAGE_KERNEL one. */
static void kvmmap(uintptr va, phys_addr_t pa, usize size, u32 perm) {
    usize off = 0;
    while (off < size) {
        uintptr a = va + off;
        phys_addr_t p = pa + off;
        int level = MAX_LEAF_LEVEL;
        while (level > 0 && (a % LEVEL_SIZE(level) != 0 || p % LEVEL_SIZE(level) != 0 ||
                             size - off < LEVEL_SIZE(level))) {
            level--;
        }
        for (;;) {
            int at;
            pte_t *pte = lookup(kernel_pagetable, a, level, true, &at);
            if (pte == NULL) {
                panic("kvmmap: out of memory");
            }
            if (at > level) {
                /* Inside a superpage mapped already: skip to its end. */
                off += LEVEL_SIZE(at) - a % LEVEL_SIZE(at);
                break;
            }
            if ((*pte & PTE_V) == 0) {
                *pte = PA2PTE(p) | perm | PTE_V;
                off += LEVEL_SIZE(level);
                break;
            }
            if (level == 0 || (*pte & PTE_LEAF) != 0) {
                /* This page (or superpage) is mapped already. */
                off += LEVEL_SIZE(level);
                break;
            }
            /* A table: some of the stretch is mapped, so map the rest
             * with smaller leaves. */
            level--;
        }
    }
}

uintptr ioremap(phys_addr_t p, usize size) {
#if __riscv_xlen == 64
    if (kernel_pagetable == NULL) {
        panic("ioremap: before kvminit");
    }
    /* Whole pages: the registers' offset in the first is kept. */
    phys_addr_t first = PGROUNDDOWN(p);
    if (size == 0 || p + size < p || PGROUNDUP(p + size) < p + size) {
        panic("ioremap: bad range");
    }
    usize len = PGROUNDUP(p + size) - first;
    /* Aligned to the mapping's size, up to the largest leaf (Linux's
     * IOREMAP_MAX_ORDER, a PUD): where the physical address is aligned as
     * well, kvmmap() then uses superpages. */
    usize align = PGSIZE;
    while (align < len && align < LEVEL_SIZE(MAX_LEAF_LEVEL)) {
        align <<= 1;
    }
    acquire(&vmalloc_lock);
    uintptr va = (ioremap_next + align - 1) & ~(align - 1);
    if (va < ioremap_next || va > VM_TEST_AREA || len > VM_TEST_AREA - va) {
        release(&vmalloc_lock);
        panic("ioremap: the vmalloc area is full");
    }
    ioremap_next = va + len;
    kvmmap(va, first, len, PAGE_KERNEL);
    release(&vmalloc_lock);
    /* A new mapping: this hart's TLB may not hold the old, invalid entry
     * (the ISA lets it cache one). The drivers ioremap() as the boot starts
     * them, on hart 0 before the other harts run (smp_boot()), whose first
     * switch to the kernel page table fences. */
    sfence_vma();
    return va + (p - first);
#else
    (void)size;
    return p;
#endif
}

void kvminit(void) {
    initlock(&uvm_lock, "uvm");
    kernel_pagetable = pagetable_create();
    if (kernel_pagetable == NULL) {
        panic("kvminit: out of memory");
    }

    /* The kernel image, at its link address (the identity on RV32), in 4 KiB
     * pages so each part gets its permissions. */
    uintptr text = (uintptr)__text_start;
    uintptr rodata = (uintptr)__rodata_start;
    uintptr data = (uintptr)__data_start;
    uintptr end = (uintptr)__kernel_end;
    if (map_pages(kernel_pagetable, text, pa(text), rodata - text, PAGE_KERNEL_READ_EXEC) != 0 ||
        map_pages(kernel_pagetable, rodata, pa(rodata), data - rodata, PAGE_KERNEL_READ) != 0) {
        panic("kvminit: out of memory");
    }
#if __riscv_xlen == 64
    if (map_pages(kernel_pagetable, data, pa(data), end - data, PAGE_KERNEL) != 0) {
        panic("kvminit: out of memory");
    }
    /* All of RAM in the direct map (kinit has cut ram_size to what the
     * direct map holds). */
    kvmmap((uintptr)va(ram_base), ram_base, ram_size, PAGE_KERNEL);
    /* The fixmap, through the early page table's own tables for it, which
     * nothing else uses (as Linux's setup_vm_final() links fixmap_pgd_next
     * into swapper_pg_dir): sysinfo and the UART stay reachable where the
     * boot reached them until their drivers ioremap() them, a panic's report
     * included. */
    u32 fix = VPN(fix_to_virt(FIX_SYSINFO), PT_LEVELS - 1);
    kernel_pagetable[fix] = early_pg_dir[fix];
    /* The devices: their drivers ioremap() them, from the vmalloc area. */
#else
    /* Every free page after the image: kinit has checked that the image
     * starts RAM, and RAM is mapped where it is. */
    (void)end;
    kvmmap(data, data, ram_size - (data - ram_base), PAGE_KERNEL);

    /* Every MMIO range the machine has that S-mode uses, whole pages, never
     * executable: not the CLINT and the test device, which only M-mode
     * touches (machine_vec, at their physical addresses; start.c), as Linux
     * under an SBI never maps them. The table doesn't promise that devices
     * are page aligned, so two of them can share a page: kvmmap() skips
     * pages already mapped. */
    u32 count = sysinfo_device_count();
    for (u32 i = 0; i < count; i++) {
        struct sysinfo_device dev;
        sysinfo_device(i, &dev);
        if (dev.size != 0 && dev.type != DEV_CLINT && dev.type != DEV_TEST) {
            phys_addr_t first = PGROUNDDOWN(dev.base);
            usize size = PGROUNDUP(dev.base + dev.size) - first;
            kvmmap(first, first, size, PAGE_KERNEL);
        }
    }
#endif

#if __riscv_xlen == 64
    /* The vmalloc area's root entry gets its table now, so that what is
     * mapped there later (kernel stacks, devices), whatever the process, is
     * in every process's root table, which copies the kernel's root entries
     * (uvm_create()). */
    int at;
    if (lookup(kernel_pagetable, VMALLOC_START, PT_LEVELS - 2, true, &at) == NULL) {
        panic("kvminit: out of memory");
    }
    initlock(&vmalloc_lock, "vmalloc");
    ioremap_next = IOREMAP_START;
    kernel_satp = MAKE_SATP(kernel_pagetable);
#endif
}

void kvminithart(void) {
    /* Finish the page-table stores before the first translation uses them. */
    sfence_vma();
    mycpu()->pagetable = kernel_pagetable;
    w_satp(MAKE_SATP(kernel_pagetable));
    sfence_vma();
}

pagetable_t vm_active_pagetable(void) {
    uintptr satp = r_satp();
    if ((satp & SATP_MODE_MASK) == 0) {
        return NULL;
    }
    return (pagetable_t)va((satp & SATP_PPN_MASK) << PGSHIFT);
}

void vm_print_mapping(pagetable_t pt, uintptr va) {
    /* The leaf that maps va, a superpage or a page (walk() would panic on
     * a superpage, which the kernel's own address space has). */
    int at;
    pte_t *pte = lookup(pt, va, 0, false, &at);
    if (pte == NULL || (*pte & PTE_V) == 0) {
        kprintf("  va " REG_FMT " is not mapped\n", va);
        return;
    }
    uintptr offset = va % LEVEL_SIZE(at);
    /* Flag letters, bit 7 down to bit 0, '-' where clear. */
    static const char letters[] = "DAGUXWRV";
    char flags[9];
    for (int i = 0; i < 8; i++) {
        flags[i] = (*pte & (1u << (7 - i))) ? letters[i] : '-';
    }
    flags[8] = '\0';
    kprintf("  va " REG_FMT " maps to pa " REG_FMT ", pte " PTE_FMT " %s\n", va, PTE2PA(*pte) | offset, *pte,
            flags);
}

/* ---- Kernel stacks ---- */

#if __riscv_xlen == 64
/* The address of slot `i`'s stack, and the slot of a stack. */
#define KSTACK_OF(i) (KSTACK_AREA + (uintptr)(i) * KSTACK_SLOT)
#define SLOT_OF(stack) (((uintptr)(stack) - KSTACK_AREA) / KSTACK_SLOT)
/* A freed stack's link to the next on kstack_deferred: its top word. */
#define NEXT_OF(stack) (*(uintptr *)((uintptr)(stack) + KSTACK_SIZE - sizeof(uintptr)))

/* How much of the stack at [bottom, bottom + size) has been used: the
 * distance from the top to its lowest non-zero word. */
static usize stack_used(uintptr bottom, usize size) {
    for (usize off = 0; off < size; off += sizeof(uintptr)) {
        if (*(volatile uintptr *)(bottom + off) != 0) {
            return size - off;
        }
    }
    return 0;
}

void kstack_reclaim(void) {
    /* Read without the lock first: the scheduler calls this after every
     * switch back, and there is seldom anything to do. */
    if (kstack_deferred == 0) {
        return;
    }
    acquire(&vmalloc_lock);
    uintptr list = kstack_deferred;
    kstack_deferred = 0;
    for (uintptr s = list; s != 0; s = NEXT_OF(s)) {
        kstack_state[SLOT_OF(s)] = KSTACK_BUSY;
    }
    release(&vmalloc_lock);
    /* No spinlock held: unmap_pages() shoots the pages down on every hart,
     * which waits for each of them, before kfree()ing them (as Linux's
     * vfree() of a stack, deferred by vfree_atomic() from where it was freed
     * with locks held). */
    while (list != 0) {
        uintptr s = list;
        list = NEXT_OF(s);
        usize used = stack_used(s, KSTACK_SIZE);
        unmap_pages(kernel_pagetable, s, KSTACK_SIZE, true);
        acquire(&vmalloc_lock);
        if (used > kstack_deepest) {
            kstack_deepest = used;
        }
        kstack_state[SLOT_OF(s)] = KSTACK_FREE;
        release(&vmalloc_lock);
    }
}

void *kstack_alloc(void) {
    /* Stacks freed holding locks are unmapped here, where none is held, so
     * that their memory goes back before more is taken. */
    kstack_reclaim();
    acquire(&vmalloc_lock);
    /* One this hart freed, still mapped: no new pages, no new mapping. */
    uintptr *cached = kstack_cached[cpuid()];
    for (u32 i = 0; i < NR_CACHED_STACKS; i++) {
        if (cached[i] != 0) {
            uintptr stack = cached[i];
            cached[i] = 0;
            release(&vmalloc_lock);
            return (void *)stack;
        }
    }
    /* A free slot. */
    u32 slot = 0;
    while (slot < KSTACK_SLOTS && kstack_state[slot] != KSTACK_FREE) {
        slot++;
    }
    if (slot == KSTACK_SLOTS) {
        release(&vmalloc_lock);
        return NULL;
    }
    uintptr stack = KSTACK_OF(slot);
    /* The stack's pages, zeroed (kstack_max_used() measures from that). The
     * page below the stack, the upper half of the slot under it, stays
     * unmapped: the guard. Under the lock, so that two harts don't race to
     * make the same table. */
    kstack_state[slot] = KSTACK_BUSY;
    usize mapped = 0;
    while (mapped < KSTACK_SIZE) {
        void *page = kzalloc();
        if (page == NULL || map_pages(kernel_pagetable, stack + mapped, pa(page), PGSIZE, PAGE_KERNEL) != 0) {
            if (page != NULL) {
                kfree(page);
            }
            break;
        }
        mapped += PGSIZE;
    }
    release(&vmalloc_lock);
    if (mapped < KSTACK_SIZE) {
        /* Out of memory: the pages go back, and the slot. */
        if (mapped != 0) {
            unmap_pages(kernel_pagetable, stack, mapped, true);
        }
        acquire(&vmalloc_lock);
        kstack_state[slot] = KSTACK_FREE;
        release(&vmalloc_lock);
        return NULL;
    }
    /* The new PTEs before this hart's first access through them: its TLB
     * may hold the slot's old, invalid translation (the ISA lets it cache
     * one, and the slot may have been mapped and unmapped before). Another
     * hart reaches the stack only by switching to a thread on it, or to its
     * page table, which fences (the scheduler, entry.S): as Linux fences
     * locally for a new vmalloc mapping and leaves the other harts to their
     * next fence (new_vmalloc). */
    sfence_vma();
    acquire(&vmalloc_lock);
    kstack_state[slot] = KSTACK_MAPPED;
    release(&vmalloc_lock);
    return (void *)stack;
}

void kstack_free(void *stack) {
    acquire(&vmalloc_lock);
    /* Cached for this hart's next thread, as Linux keeps NR_CACHED_STACKS
     * per CPU; any other waits for kstack_reclaim(), since unmapping needs a
     * shootdown, which can't wait for other harts holding a lock. */
    uintptr *cached = kstack_cached[cpuid()];
    for (u32 i = 0; i < NR_CACHED_STACKS; i++) {
        if (cached[i] == 0) {
            cached[i] = (uintptr)stack;
            release(&vmalloc_lock);
            return;
        }
    }
    NEXT_OF(stack) = kstack_deferred;
    kstack_deferred = (uintptr)stack;
    release(&vmalloc_lock);
}

void kstack_count(struct kstack_counts *n) {
    acquire(&vmalloc_lock);
    n->mapped = 0;
    for (u32 i = 0; i < KSTACK_SLOTS; i++) {
        if (kstack_state[i] != KSTACK_FREE) {
            n->mapped++;
        }
    }
    n->cached = 0;
    for (u32 h = 0; h < NCPU; h++) {
        for (u32 i = 0; i < NR_CACHED_STACKS; i++) {
            if (kstack_cached[h][i] != 0) {
                n->cached++;
            }
        }
    }
    n->deferred = 0;
    for (uintptr s = kstack_deferred; s != 0; s = NEXT_OF(s)) {
        n->deferred++;
    }
    release(&vmalloc_lock);
}

usize kstack_max_used(void) {
    /* Hart 0's boot stack (entry.S zeroes it), every stack mapped now, in
     * use, cached or waiting, and those unmapped before. */
    usize most = stack_used((uintptr)__stack_bottom, KSTACK_SIZE);
    acquire(&vmalloc_lock);
    if (kstack_deepest > most) {
        most = kstack_deepest;
    }
    for (u32 i = 0; i < KSTACK_SLOTS; i++) {
        if (kstack_state[i] == KSTACK_MAPPED) {
            usize used = stack_used(KSTACK_OF(i), KSTACK_SIZE);
            if (used > most) {
                most = used;
            }
        }
    }
    release(&vmalloc_lock);
    return most;
}
#else
void *kstack_alloc(void) {
    return kalloc();
}

void kstack_free(void *stack) {
    kfree(stack);
}

void kstack_reclaim(void) {
}

void kstack_count(struct kstack_counts *n) {
    n->mapped = 0;
    n->cached = 0;
    n->deferred = 0;
}

usize kstack_max_used(void) {
    return 0;
}
#endif

/* ---- The early page table (RV64), with paging off ---- */

#if __riscv_xlen == 64
static u32 early_used;

/* Everything below runs with paging off, at the image's physical address:
 * a pointer to the kernel's own data (a pc-relative address) is its physical
 * address, and nothing here may use the high ones (no pointer stored in
 * initialised data, no string from a table). */

/* Prints `msg` on the UART at its physical address and stops the machine:
 * a panic before there is a kprintf. */
static void early_panic(const char *msg) {
    for (const char *s = "panic: "; *s; s++) {
        uart_putc_phys(*s);
    }
    for (; *msg; msg++) {
        uart_putc_phys(*msg);
    }
    uart_putc_phys('\n');
    machine_halt(PANIC_EXIT_CODE);
}

static pagetable_t early_table(void) {
    if (early_used == EARLY_TABLES) {
        early_panic("setup_vm: out of early page tables");
    }
    pagetable_t t = &early_tables[early_used * PTES_PER_TABLE];
    early_used++;
    for (u32 i = 0; i < PTES_PER_TABLE; i++) {
        t[i] = 0;
    }
    return t;
}

/* Maps [va, va + size) to `pa` in the early page table, `size` a whole
 * number of pages: each stretch with the largest leaf, up to level `max`,
 * that both addresses are aligned to and the rest of the range covers, as
 * kvmmap() does, so nothing past the range is mapped. */
static void early_map(uintptr va, phys_addr_t pa, u64 size, int max, u32 perm) {
    u64 off = 0;
    while (off < size) {
        uintptr a = va + off;
        phys_addr_t p = pa + off;
        int level = max;
        while (level > 0 && (a % LEVEL_SIZE(level) != 0 || p % LEVEL_SIZE(level) != 0 ||
                             size - off < LEVEL_SIZE(level))) {
            level--;
        }
        pagetable_t t = early_pg_dir;
        for (int l = (int)pt_levels - 1; l > level; l--) {
            pte_t *pte = &t[VPN(a, l)];
            if ((*pte & PTE_V) == 0) {
                *pte = PA2PTE((uintptr)early_table()) | PTE_V;
            }
            t = (pagetable_t)PTE2PA(*pte);
        }
        t[VPN(a, level)] = PA2PTE(p) | perm | PTE_V;
        off += LEVEL_SIZE(level);
    }
}

/* Builds the early page table for `levels` (4: Sv48, 3: Sv39), the image
 * loaded at `load`, and the layout's variables for that mode. */
static void early_build(u32 levels, phys_addr_t load, phys_addr_t base, u64 size) {
    pt_levels = levels;
    if (levels == 4) {
        satp_mode = SATP_MODE_SV48;
        page_offset = PAGE_OFFSET_SV48;
        vmalloc_start = VMALLOC_START_SV48;
        fixaddr_top = FIXADDR_TOP_SV48;
        direct_map_size = DIRECT_MAP_SIZE_SV48;
        user_top = 1ul << 47;
    } else {
        satp_mode = SATP_MODE_SV39;
        page_offset = PAGE_OFFSET_SV39;
        vmalloc_start = VMALLOC_START_SV39;
        fixaddr_top = FIXADDR_TOP_SV39;
        direct_map_size = DIRECT_MAP_SIZE_SV39;
        user_top = 1ul << 38;
    }
    va_pa_offset = page_offset - base;
    early_used = 0;
    for (u32 i = 0; i < PTES_PER_TABLE; i++) {
        early_pg_dir[i] = 0;
    }
    /* The image at both addresses, in 2 MiB leaves (the last one rounded up,
     * as Linux maps the kernel in early_pg_dir), R+W+X: entry.S runs at the
     * physical one until it jumps to the link one. */
    u64 image = ((uintptr)__kernel_end - load + LEVEL_SIZE(1) - 1) & ~(LEVEL_SIZE(1) - 1);
    early_map(load, load, image, 1, PAGE_KERNEL_EXEC);
    early_map(KERNEL_LINK_ADDR, load, image, 1, PAGE_KERNEL_EXEC);
    /* All of RAM in the direct map, so kinit() and kvminit() can reach
     * every page before the final table exists: exactly RAM (what the
     * direct map holds of it, whole pages), 1 GiB leaves, then 2 MiB and
     * 4 KiB ones for its end. */
    u64 ram = (size < direct_map_size ? size : direct_map_size) & ~(PGSIZE - 1);
    early_map(page_offset, base, ram, 2, PAGE_KERNEL);
    /* The fixmap: the devices used before ioremap() works, a page each. */
    early_map(fix_to_virt(FIX_SYSINFO), SYSINFO_BASE, PGSIZE, 0, PAGE_KERNEL);
    early_map(fix_to_virt(FIX_UART), UART_BASE, PGSIZE, 0, PAGE_KERNEL);
    early_satp = satp_mode | ((uintptr)early_pg_dir >> PGSHIFT);
}

void setup_vm(void) {
    /* Where the image was loaded: _start's physical address, since this
     * runs there. */
    phys_addr_t load = (uintptr)__text_start;
    va_kernel_pa_offset = KERNEL_LINK_ADDR - load;
    if (load % LEVEL_SIZE(1) != 0) {
        early_panic("setup_vm: the kernel is not loaded at a 2 MiB boundary");
    }
    if (mmio_read32(SYSINFO_BASE + SYS_MAGIC) != SYSINFO_MAGIC ||
        mmio_read32(SYSINFO_BASE + SYS_VERSION) != SYSINFO_VERSION) {
        early_panic("setup_vm: no sysinfo block of version 2 at 0x30002000");
    }
    phys_addr_t base = mmio_read64(SYSINFO_BASE + SYS_RAM_BASE);
    u64 size = mmio_read64(SYSINFO_BASE + SYS_RAM_SIZE);
    if (base % LEVEL_SIZE(2) != 0) {
        early_panic("setup_vm: RAM does not start at a 1 GiB boundary");
    }

    /* Sv48 if the hart has it. satp is WARL: a write with a MODE the hart
     * doesn't implement changes nothing, so reading it back says. The write
     * turns translation on at once, so it is made with a table that maps
     * this code and its stack where they are (the image at its physical
     * address), and turned off right after (Linux's set_satp_mode()). */
    early_build(4, load, base, size);
    w_satp(early_satp);
    sfence_vma();
    uintptr got = r_satp();
    w_satp(0);
    sfence_vma();
    if ((got & SATP_MODE_MASK) != SATP_MODE_SV48) {
        early_build(3, load, base, size);
    }
    /* From here on, until their drivers ioremap() them, sysinfo and the
     * UART are reached through the fixmap: as soon as paging is on, a
     * panic's report gets out (the variables are written here at their
     * physical addresses, the same memory). */
    sysinfo_regs = fix_to_virt(FIX_SYSINFO);
    uart_regs = fix_to_virt(FIX_UART);
}
#endif

/* ---- User address spaces ---- */

pagetable_t uvm_create(void) {
    pagetable_t pt = pagetable_create();
    if (pt == NULL) {
        return NULL;
    }
    /* The root entries, not the tables they point to: a process shares the
     * kernel's tables, so it never copies or frees them. */
    for (u32 i = USER_ROOT_ENTRIES; i < PTES_PER_TABLE; i++) {
        pt[i] = kernel_pagetable[i];
    }
    return pt;
}

/* Frees every page mapped under `table`, a user-space table at `level`. */
static void uvm_free_pages(pagetable_t table, int level) {
    for (u32 j = 0; j < PTES_PER_TABLE; j++) {
        if ((table[j] & PTE_V) == 0) {
            continue;
        }
        if (level == 0) {
            kfree(va(PTE2PA(table[j])));
            table[j] = 0;
        } else {
            uvm_free_pages((pagetable_t)va(PTE2PA(table[j])), level - 1);
        }
    }
}

void uvm_free(pagetable_t pt) {
    for (u32 i = 0; i < USER_ROOT_ENTRIES; i++) {
        if (pt[i] & PTE_V) {
            uvm_free_pages((pagetable_t)va(PTE2PA(pt[i])), PT_LEVELS - 2);
        }
    }
    /* Forget the kernel's tables, so that pagetable_free() frees only the
     * root and the user-space tables. */
    for (u32 i = USER_ROOT_ENTRIES; i < PTES_PER_TABLE; i++) {
        pt[i] = 0;
    }
    pagetable_free(pt);
}

bool uvm_range_ok(pagetable_t pt, uintptr va, usize len, u32 perm) {
    if (len == 0) {
        return true;
    }
    /* Inside user space, so va + len can't wrap around either. */
    if (va >= USER_TOP || len > USER_TOP - va) {
        return false;
    }
    u32 want = perm | PTE_U | PTE_V;
    for (uintptr a = PGROUNDDOWN(va); a < va + len; a += PGSIZE) {
        pte_t *pte = walk(pt, a, false);
        if (pte == NULL || (*pte & want) != want) {
            return false;
        }
    }
    return true;
}

/* The kernel's pointer to user address `va` in `pt`, which must be mapped:
 * the user page's physical address, through the direct map (on RV32 the
 * kernel's identity map). */
static u8 *user_ptr(pagetable_t pt, uintptr va) {
    return (u8 *)va(PTE2PA(*walk(pt, va, false))) + va % PGSIZE;
}

/* The bytes from `va` to the end of its page, at most `len`. */
static usize page_chunk(uintptr va, usize len) {
    usize n = PGSIZE - va % PGSIZE;
    return n < len ? n : len;
}

int copyin(pagetable_t pt, void *dst, uintptr va, usize len) {
    acquire(&uvm_lock);
    if (!uvm_range_ok(pt, va, len, PTE_R)) {
        release(&uvm_lock);
        return -1;
    }
    u8 *out = dst;
    while (len > 0) {
        usize n = page_chunk(va, len);
        memcpy(out, user_ptr(pt, va), n);
        out += n;
        va += n;
        len -= n;
    }
    release(&uvm_lock);
    return 0;
}

int copyout(pagetable_t pt, uintptr va, const void *src, usize len) {
    acquire(&uvm_lock);
    if (!uvm_range_ok(pt, va, len, PTE_W)) {
        release(&uvm_lock);
        return -1;
    }
    const u8 *in = src;
    while (len > 0) {
        usize n = page_chunk(va, len);
        memcpy(user_ptr(pt, va), in, n);
        in += n;
        va += n;
        len -= n;
    }
    release(&uvm_lock);
    return 0;
}

int copyinstr(pagetable_t pt, char *dst, uintptr va, u32 max) {
    /* Page by page: the string's length isn't known up front, and the page
     * after its NUL needn't be mapped. */
    u32 len = 0;
    acquire(&uvm_lock);
    while (len < max) {
        usize n = page_chunk(va, max - len);
        if (!uvm_range_ok(pt, va, n, PTE_R)) {
            dst[len] = 0;
            release(&uvm_lock);
            return -1;
        }
        const u8 *src = user_ptr(pt, va);
        for (usize i = 0; i < n; i++) {
            dst[len] = (char)src[i];
            if (src[i] == 0) {
                release(&uvm_lock);
                return (int)len;
            }
            len++;
        }
        va += n;
    }
    release(&uvm_lock);
    dst[max - 1] = 0;
    return -2;
}
