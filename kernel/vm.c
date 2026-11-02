/* Sv32 page tables and the kernel address space. Physical memory is identity
 * mapped in the kernel page table, so a physical address (a page table, a page
 * from kalloc) is also a usable pointer with paging on. */
#include "vm.h"
#include "kalloc.h"
#include "kprintf.h"
#include "memlayout.h"
#include "riscv.h"
#include "smp.h"
#include "string.h"
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
    /* Two passes. First the mappings go: a PTE without V translates
     * nothing, but keeps the page's address for the second pass. */
    for (u32 a = va;; a += PGSIZE) {
        pte_t *pte = walk(pt, a, false);
        if (pte == NULL || (*pte & PTE_V) == 0) {
            kprintf("unmap_pages: %p is not mapped\n", (void *)(uintptr)a);
            panic("unmap_pages: not mapped");
        }
        *pte = *pte & ~PTE_V;
        if (a == last) {
            break;
        }
    }
    /* Then no hart may still hold a translation to one of the pages in its
     * TLB (smp.h), before they can be reused. */
    tlb_shootdown(pt);
    for (u32 a = va;; a += PGSIZE) {
        pte_t *pte = walk(pt, a, false);
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

    /* Every MMIO range the machine has, whole pages, never executable. The
     * table doesn't promise that devices are page aligned, so two of them can
     * share a page: map page by page, skipping pages already mapped. */
    u32 count = sysinfo_device_count();
    for (u32 i = 0; i < count; i++) {
        struct sysinfo_device dev;
        sysinfo_device(i, &dev);
        if (dev.size == 0) {
            continue;
        }
        /* Page numbers, so a device ending at 2^32 doesn't wrap. */
        u32 first = dev.base / PGSIZE;
        u32 last = (dev.base + (dev.size - 1)) / PGSIZE;
        for (u32 page = first;; page++) {
            u32 a = page * PGSIZE;
            pte_t *pte = walk(kernel_pagetable, a, false);
            if (pte == NULL || (*pte & PTE_V) == 0) {
                kvmmap(a, PGSIZE, PTE_R | PTE_W);
            }
            if (page == last) {
                break;
            }
        }
    }
}

void kvminithart(void) {
    /* Finish the page-table stores before the first translation uses them. */
    sfence_vma();
    mycpu()->pagetable = kernel_pagetable;
    w_satp(MAKE_SATP(kernel_pagetable));
    sfence_vma();
}

pagetable_t vm_active_pagetable(void) {
    u32 satp = r_satp();
    if ((satp & SATP_SV32) == 0) {
        return NULL;
    }
    return (pagetable_t)(uintptr)((satp & SATP_PPN_MASK) << PGSHIFT);
}

void vm_print_mapping(pagetable_t pt, u32 va) {
    pte_t *pte = walk(pt, va, false);
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

pagetable_t uvm_create(void) {
    pagetable_t pt = pagetable_create();
    if (pt == NULL) {
        return NULL;
    }
    /* The root entries, not the tables they point to: a process shares the
     * kernel's second-level tables, so it never copies or frees them. */
    for (u32 i = USER_ROOT_ENTRIES; i < PTES_PER_TABLE; i++) {
        pt[i] = kernel_pagetable[i];
    }
    return pt;
}

void uvm_free(pagetable_t pt) {
    for (u32 i = 0; i < USER_ROOT_ENTRIES; i++) {
        if ((pt[i] & PTE_V) == 0) {
            continue;
        }
        pagetable_t table = (pagetable_t)(uintptr)PTE2PA(pt[i]);
        for (u32 j = 0; j < PTES_PER_TABLE; j++) {
            if (table[j] & PTE_V) {
                kfree((void *)(uintptr)PTE2PA(table[j]));
                table[j] = 0;
            }
        }
    }
    /* Forget the kernel's tables, so that pagetable_free() frees only the
     * root and the user-space tables. */
    for (u32 i = USER_ROOT_ENTRIES; i < PTES_PER_TABLE; i++) {
        pt[i] = 0;
    }
    pagetable_free(pt);
}

bool uvm_range_ok(pagetable_t pt, u32 va, u32 len, u32 perm) {
    if (len == 0) {
        return true;
    }
    /* Inside user space, so va + len can't wrap around either. */
    if (va >= USER_TOP || len > USER_TOP - va) {
        return false;
    }
    u32 want = perm | PTE_U | PTE_V;
    for (u32 a = PGROUNDDOWN(va); a < va + len; a += PGSIZE) {
        pte_t *pte = walk(pt, a, false);
        if (pte == NULL || (*pte & want) != want) {
            return false;
        }
    }
    return true;
}

/* The kernel's pointer to user address `va` in `pt`, which must be mapped:
 * RAM is identity mapped in the kernel's half, so the user page's physical
 * address is a usable pointer. */
static u8 *user_ptr(pagetable_t pt, u32 va) {
    return (u8 *)(uintptr)(PTE2PA(*walk(pt, va, false)) + va % PGSIZE);
}

/* The bytes from `va` to the end of its page, at most `len`. */
static u32 page_chunk(u32 va, u32 len) {
    u32 n = PGSIZE - va % PGSIZE;
    return n < len ? n : len;
}

int copyin(pagetable_t pt, void *dst, u32 va, u32 len) {
    if (!uvm_range_ok(pt, va, len, PTE_R)) {
        return -1;
    }
    u8 *out = dst;
    while (len > 0) {
        u32 n = page_chunk(va, len);
        memcpy(out, user_ptr(pt, va), n);
        out += n;
        va += n;
        len -= n;
    }
    return 0;
}

int copyout(pagetable_t pt, u32 va, const void *src, u32 len) {
    if (!uvm_range_ok(pt, va, len, PTE_W)) {
        return -1;
    }
    const u8 *in = src;
    while (len > 0) {
        u32 n = page_chunk(va, len);
        memcpy(user_ptr(pt, va), in, n);
        in += n;
        va += n;
        len -= n;
    }
    return 0;
}

int copyinstr(pagetable_t pt, char *dst, u32 va, u32 max) {
    /* Page by page: the string's length isn't known up front, and the page
     * after its NUL needn't be mapped. */
    u32 len = 0;
    while (len < max) {
        u32 n = page_chunk(va, max - len);
        if (!uvm_range_ok(pt, va, n, PTE_R)) {
            dst[len] = 0;
            return -1;
        }
        const u8 *src = user_ptr(pt, va);
        for (u32 i = 0; i < n; i++) {
            dst[len] = (char)src[i];
            if (src[i] == 0) {
                return (int)len;
            }
            len++;
        }
        va += n;
    }
    dst[max - 1] = 0;
    return -2;
}
