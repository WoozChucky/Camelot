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
/* The kernel stacks (kstack_alloc()). kstack_next: the next slot never
 * handed out, from KSTACK_AREA up; kstack_cache: the stacks given back, each
 * holding the next one's address in its top word. Under kstack_lock, which
 * is a leaf of the lock order but for kalloc's. */
static struct spinlock kstack_lock;
static uintptr kstack_next;
static uintptr kstack_cache;
#endif

#if __riscv_xlen == 64
u32 pt_levels;
uintptr satp_mode;
uintptr page_offset;
uintptr vmalloc_start;
u64 direct_map_size;
uintptr va_pa_offset;
uintptr va_kernel_pa_offset;
uintptr user_top;
uintptr early_satp;
uintptr kernel_satp;
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
    /* W without R is a reserved encoding; no R/W/X would be a table pointer. */
    if ((perm & PTE_LEAF) == 0 || (perm & ~PTE_PERM_MASK) != 0 || (perm & (PTE_R | PTE_W)) == PTE_W) {
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
 * 16384 PTEs, most of a boot. `size` is a whole number of pages. */
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
    if (p >= IO_WINDOW_SIZE || size > IO_WINDOW_SIZE - p) {
        panic("ioremap: a device outside the devices' window");
    }
    return VMALLOC_START + p;
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
    if (map_pages(kernel_pagetable, text, pa(text), rodata - text, PTE_R | PTE_X) != 0 ||
        map_pages(kernel_pagetable, rodata, pa(rodata), data - rodata, PTE_R) != 0) {
        panic("kvminit: out of memory");
    }
#if __riscv_xlen == 64
    if (map_pages(kernel_pagetable, data, pa(data), end - data, PTE_R | PTE_W) != 0) {
        panic("kvminit: out of memory");
    }
    /* All of RAM in the direct map (kinit has cut ram_size to what the
     * direct map holds). */
    kvmmap((uintptr)va(ram_base), ram_base, ram_size, PTE_R | PTE_W);
#else
    /* Every free page after the image: kinit has checked that the image
     * starts RAM, and RAM is mapped where it is. */
    (void)end;
    kvmmap(data, data, ram_size - (data - ram_base), PTE_R | PTE_W);
#endif

    /* Every MMIO range the machine has, whole pages, never executable. The
     * table doesn't promise that devices are page aligned, so two of them can
     * share a page: kvmmap() skips pages already mapped. */
    u32 count = sysinfo_device_count();
    for (u32 i = 0; i < count; i++) {
        struct sysinfo_device dev;
        sysinfo_device(i, &dev);
        if (dev.size != 0) {
            phys_addr_t first = PGROUNDDOWN(dev.base);
            usize size = PGROUNDUP(dev.base + dev.size) - first;
            kvmmap(ioremap(first, size), first, size, PTE_R | PTE_W);
        }
    }

#if __riscv_xlen == 64
    /* The kernel stacks' part of the vmalloc area gets its table under the
     * root now, so that the stacks mapped later, whatever the process, are
     * in every process's root table, which copies the kernel's root entries
     * (uvm_create()). */
    int at;
    if (lookup(kernel_pagetable, KSTACK_AREA, PT_LEVELS - 2, true, &at) == NULL) {
        panic("kvminit: out of memory");
    }
    initlock(&kstack_lock, "kstack");
    kstack_next = KSTACK_AREA;
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
void *kstack_alloc(void) {
    acquire(&kstack_lock);
    if (kstack_cache != 0) {
        uintptr stack = kstack_cache;
        kstack_cache = *(uintptr *)(stack + KSTACK_SIZE - sizeof(uintptr));
        release(&kstack_lock);
        return (void *)stack;
    }
    /* A new slot. They stay inside the root entry kvminit() made a table
     * for, below the self-tests' part of it. */
    uintptr stack = kstack_next;
    if (stack + KSTACK_SLOT > VM_TEST_AREA) {
        release(&kstack_lock);
        return NULL;
    }
    kstack_next = stack + KSTACK_SLOT;
    /* The stack's pages, zeroed (kstack_max_used() measures from that). The
     * page below the stack, the upper half of the slot under it, stays
     * unmapped: the guard. Under the lock, so that two harts don't race to
     * make the same table. */
    usize mapped = 0;
    while (mapped < KSTACK_SIZE) {
        void *page = kzalloc();
        if (page == NULL || map_pages(kernel_pagetable, stack + mapped, pa(page), PGSIZE, PTE_R | PTE_W) != 0) {
            if (page != NULL) {
                kfree(page);
            }
            break;
        }
        mapped += PGSIZE;
    }
    release(&kstack_lock);
    if (mapped < KSTACK_SIZE) {
        /* Out of memory: the pages go back (the slot is not used again). */
        if (mapped != 0) {
            unmap_pages(kernel_pagetable, stack, mapped, true);
        }
        return NULL;
    }
    return (void *)stack;
}

void kstack_free(void *stack) {
    acquire(&kstack_lock);
    *(uintptr *)((uintptr)stack + KSTACK_SIZE - sizeof(uintptr)) = kstack_cache;
    kstack_cache = (uintptr)stack;
    release(&kstack_lock);
}

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

usize kstack_max_used(void) {
    /* Hart 0's boot stack (entry.S zeroes it), then every slot handed out:
     * all are mapped, in use or cached. */
    usize most = stack_used((uintptr)__stack_bottom, KSTACK_SIZE);
    acquire(&kstack_lock);
    for (uintptr s = KSTACK_AREA; s < kstack_next; s += KSTACK_SLOT) {
        usize used = stack_used(s, KSTACK_SIZE);
        if (used > most) {
            most = used;
        }
    }
    release(&kstack_lock);
    return most;
}
#else
void *kstack_alloc(void) {
    return kalloc();
}

void kstack_free(void *stack) {
    kfree(stack);
}

usize kstack_max_used(void) {
    return 0;
}
#endif

/* ---- The early page table (RV64), with paging off ---- */

#if __riscv_xlen == 64
/* The early page table and the pool of EARLY_TABLES (vm.h) its lower tables
 * come from, in .bss (entry.S, page aligned). */
extern pte_t early_pg_dir[];
extern pte_t early_tables[];
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

/* Maps [va, va + size) to `pa` in the early page table with leaves at
 * `level`; size is rounded up to whole leaves. */
static void early_map(uintptr va, phys_addr_t pa, u64 size, int level, u32 perm) {
    for (u64 off = 0; off < size; off += LEVEL_SIZE(level)) {
        pagetable_t t = early_pg_dir;
        for (int l = (int)pt_levels - 1; l > level; l--) {
            pte_t *pte = &t[VPN(va + off, l)];
            if ((*pte & PTE_V) == 0) {
                *pte = PA2PTE((uintptr)early_table()) | PTE_V;
            }
            t = (pagetable_t)PTE2PA(*pte);
        }
        t[VPN(va + off, level)] = PA2PTE(pa + off) | perm | PTE_V;
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
        direct_map_size = DIRECT_MAP_SIZE_SV48;
        user_top = 1ul << 47;
    } else {
        satp_mode = SATP_MODE_SV39;
        page_offset = PAGE_OFFSET_SV39;
        vmalloc_start = VMALLOC_START_SV39;
        direct_map_size = DIRECT_MAP_SIZE_SV39;
        user_top = 1ul << 38;
    }
    va_pa_offset = page_offset - base;
    early_used = 0;
    for (u32 i = 0; i < PTES_PER_TABLE; i++) {
        early_pg_dir[i] = 0;
    }
    /* The image at both addresses, in 2 MiB leaves, R+W+X: entry.S runs at
     * the physical one until it jumps to the link one. */
    u64 image = (uintptr)__kernel_end - load;
    early_map(load, load, image, 1, PTE_R | PTE_W | PTE_X);
    early_map(KERNEL_LINK_ADDR, load, image, 1, PTE_R | PTE_W | PTE_X);
    /* RAM in the direct map, in 1 GiB leaves, so kinit() and kvminit() can
     * reach every page before the final table exists (it maps RAM exactly;
     * these may run past its end, but nothing goes there). */
    early_map(page_offset, base, size < direct_map_size ? size : direct_map_size, 2, PTE_R | PTE_W);
    /* The devices' window, in 1 GiB leaves: every device is in the first
     * 2 GiB, below RAM (spec section 7). */
    early_map(vmalloc_start, 0, 0x80000000ull, 2, PTE_R | PTE_W);
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
