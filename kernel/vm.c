/* Sv32 page tables and the kernel address space. Physical memory is identity
 * mapped in the kernel page table, so a physical address (a page table, a page
 * from kalloc) is also a usable pointer with paging on. */
#include "vm.h"
#include "kalloc.h"
#include "kprintf.h"
#include "memlayout.h"
#include "riscv.h"
#include "sysinfo.h"
#include "trap.h"

#define PTE_LEAF (PTE_R | PTE_W | PTE_X)
#define PTES_PER_TABLE 1024

pagetable_t kernel_pagetable;

pagetable_t pagetable_create(void) {
    return kzalloc();
}

pte_t *walk(pagetable_t pt, u32 va, bool alloc) {
    pte_t *root = &pt[VPN(va, 1)];
    pagetable_t table;
    if (*root & PTE_V) {
        if (*root & PTE_LEAF) {
            panic("walk: unexpected megapage");
        }
        table = (pagetable_t)(uintptr)PTE2PA(*root);
    } else {
        if (!alloc) {
            return NULL;
        }
        table = pagetable_create();
        if (table == NULL) {
            return NULL;
        }
        /* A pointer to the next level: valid, no R/W/X. */
        *root = PA2PTE(table) | PTE_V;
    }
    return &table[VPN(va, 0)];
}

int map_pages(pagetable_t pt, u32 va, u32 pa, u32 size, u32 perm) {
    if (va % PGSIZE != 0 || pa % PGSIZE != 0 || size % PGSIZE != 0 || size == 0) {
        panic("map_pages: unaligned range");
    }
    /* W without R is a reserved encoding; no R/W/X would be a table pointer. */
    if ((perm & PTE_LEAF) == 0 || (perm & ~PTE_PERM_MASK) != 0 || (perm & (PTE_R | PTE_W)) == PTE_W) {
        panic("map_pages: bad permissions");
    }
    /* The last page, computed so that a range ending at 2^32 doesn't wrap. */
    u32 last = va + (size - PGSIZE);
    if (last < va) {
        panic("map_pages: range wraps around");
    }
    for (u32 a = va;; a += PGSIZE, pa += PGSIZE) {
        pte_t *pte = walk(pt, a, true);
        if (pte == NULL) {
            return -1;
        }
        if (*pte & PTE_V) {
            kprintf("map_pages: %p is already mapped\n", (void *)(uintptr)a);
            panic("map_pages: remap");
        }
        *pte = PA2PTE(pa) | perm | PTE_V;
        if (a == last) {
            return 0;
        }
    }
}

void unmap_pages(pagetable_t pt, u32 va, u32 size, bool free_pages) {
    if (va % PGSIZE != 0 || size % PGSIZE != 0 || size == 0) {
        panic("unmap_pages: unaligned range");
    }
    u32 last = va + (size - PGSIZE);
    if (last < va) {
        panic("unmap_pages: range wraps around");
    }
    for (u32 a = va;; a += PGSIZE) {
        pte_t *pte = walk(pt, a, false);
        if (pte == NULL || (*pte & PTE_V) == 0) {
            kprintf("unmap_pages: %p is not mapped\n", (void *)(uintptr)a);
            panic("unmap_pages: not mapped");
        }
        if (free_pages) {
            kfree((void *)(uintptr)PTE2PA(*pte));
        }
        *pte = 0;
        if (a == last) {
            return;
        }
    }
}

void pagetable_free(pagetable_t pt) {
    for (u32 i = 0; i < PTES_PER_TABLE; i++) {
        pte_t pte = pt[i];
        if ((pte & PTE_V) == 0) {
            continue;
        }
        if (pte & PTE_LEAF) {
            panic("pagetable_free: megapage still mapped");
        }
        pagetable_t table = (pagetable_t)(uintptr)PTE2PA(pte);
        for (u32 j = 0; j < PTES_PER_TABLE; j++) {
            if (table[j] & PTE_V) {
                panic("pagetable_free: page still mapped");
            }
        }
        kfree(table);
    }
    kfree(pt);
}

/* Identity-maps [start, start + size) in the kernel page table. */
static void kvmmap(u32 start, u32 size, u32 perm) {
    if (map_pages(kernel_pagetable, start, start, size, perm) != 0) {
        panic("kvmmap: out of memory");
    }
}

void kvminit(void) {
    kernel_pagetable = pagetable_create();
    if (kernel_pagetable == NULL) {
        panic("kvminit: out of memory");
    }

    /* The kernel image, then every free page after it (kinit has checked that
     * the image starts RAM). */
    u32 text = (u32)(uintptr)__text_start;
    u32 rodata = (u32)(uintptr)__rodata_start;
    u32 data = (u32)(uintptr)__data_start;
    kvmmap(text, rodata - text, PTE_R | PTE_X);
    kvmmap(rodata, data - rodata, PTE_R);
    kvmmap(data, ram_size - (data - ram_base), PTE_R | PTE_W);

    /* Every MMIO range the machine has, whole pages, never executable. */
    u32 count = sysinfo_device_count();
    for (u32 i = 0; i < count; i++) {
        struct sysinfo_device dev;
        sysinfo_device(i, &dev);
        if (dev.size == 0) {
            continue;
        }
        u32 start = PGROUNDDOWN(dev.base);
        u32 end = PGROUNDUP(dev.base + dev.size);
        kvmmap(start, end - start, PTE_R | PTE_W);
    }
}

void kvminithart(void) {
    /* Finish the page-table stores before the first translation uses them. */
    sfence_vma();
    w_satp(MAKE_SATP(kernel_pagetable));
    sfence_vma();
}

void vm_print_mapping(u32 va) {
    if (kernel_pagetable == NULL) {
        return;
    }
    pte_t *pte = walk(kernel_pagetable, va, false);
    if (pte == NULL || (*pte & PTE_V) == 0) {
        kprintf("  va 0x%08x is not mapped\n", va);
        return;
    }
    /* Flag letters, bit 7 down to bit 0, '-' where clear. */
    static const char letters[] = "DAGUXWRV";
    char flags[9];
    for (int i = 0; i < 8; i++) {
        flags[i] = (*pte & (1u << (7 - i))) ? letters[i] : '-';
    }
    flags[8] = '\0';
    kprintf("  va 0x%08x maps to pa 0x%08x, pte 0x%08x %s\n", va, PTE2PA(*pte) | (va & (PGSIZE - 1)), *pte,
            flags);
}
