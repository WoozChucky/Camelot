/* Self-tests for the page allocator and paging, run by kmain with paging on
 * and before interrupts are enabled. */
#include "vmtest.h"
#include "kalloc.h"
#include "kprintf.h"
#include "memlayout.h"
#include "probe.h"
#include "riscv.h"
#include "vm.h"

/* Nothing in the memory map lives at these addresses (spec section 7), so the
 * tests can map them freely. The two pages at SPLIT_VA straddle a 4 MiB
 * boundary: they use two second-level tables. */
#define ALIAS_VA 0x40000000u
#define SPLIT_VA (ALIAS_VA - PGSIZE)
#define RANGE_VA 0x50000000u

static int passed;
static int failed;

static void check(bool ok, const char *what) {
    if (ok) {
        passed++;
    } else {
        failed++;
        kprintf("vm self-test FAILED: %s\n", what);
    }
}

static bool is_free_ram(u32 a) {
    return a % PGSIZE == 0 && a >= (u32)(uintptr)__kernel_end && a - ram_base < ram_size;
}

/* Takes every page off the free list, counting them, then puts them all back.
 * The pages are chained through their first word while out. */
static void test_alloc_all(void) {
    u32 before = kfree_pages();
    u32 *chain = NULL;
    u32 n = 0;
    bool in_range = true;
    for (;;) {
        u32 *p = kalloc();
        if (p == NULL) {
            break;
        }
        in_range = in_range && is_free_ram((u32)(uintptr)p);
        *p = (u32)(uintptr)chain;
        chain = p;
        n++;
    }
    check(n == before && n > 0, "kalloc hands out every free page");
    check(in_range, "kalloc pages are aligned and after the kernel");
    check(kfree_pages() == 0 && kalloc() == NULL, "kalloc returns NULL when out of pages");
    /* Out of memory: no new table, and a mapping that needs one fails. */
    check(pagetable_create() == NULL, "pagetable_create fails when out of pages");
    check(map_pages(kernel_pagetable, ALIAS_VA, (u32)(uintptr)chain, PGSIZE, PTE_R | PTE_W) == -1,
          "map_pages fails when it can't allocate a page table");

    for (u32 i = 0; i < n; i++) {
        u32 *next = (u32 *)(uintptr)*chain;
        kfree(chain);
        chain = next;
    }
    check(chain == NULL && kfree_pages() == before, "kfree takes every page back");
}

/* Maps a page a second time, at ALIAS_VA, and writes through each address. */
static void test_alias(void) {
    volatile u32 *page = kalloc();
    volatile u32 *alias = (volatile u32 *)ALIAS_VA;
    check(map_pages(kernel_pagetable, ALIAS_VA, (u32)(uintptr)page, PGSIZE, PTE_R | PTE_W) == 0,
          "map a second mapping");
    sfence_vma();

    alias[0] = 0xc0ffee01u;
    page[PGSIZE / 4 - 1] = 0x5eed1234u;
    check(page[0] == 0xc0ffee01u && alias[PGSIZE / 4 - 1] == 0x5eed1234u,
          "both mappings see the same page");

    /* Grail manages A and D: the stores above set both. */
    pte_t *pte = walk(kernel_pagetable, ALIAS_VA, false);
    check(pte != NULL && PTE2PA(*pte) == (u32)(uintptr)page &&
              (*pte & (PTE_V | PTE_R | PTE_W | PTE_X | PTE_U | PTE_A | PTE_D)) ==
                  (PTE_V | PTE_R | PTE_W | PTE_A | PTE_D),
          "the alias PTE has the page, R+W and A+D");

    /* Unmapped again (the page goes back to the free list), it faults. */
    u32 free_before = kfree_pages();
    unmap_pages(kernel_pagetable, ALIAS_VA, PGSIZE, true);
    sfence_vma();
    check(kfree_pages() == free_before + 1, "unmap_pages frees the page");
    u32 val = 0;
    u32 stval = 0;
    check(probe_load((const u32 *)ALIAS_VA, &val, &stval) == CAUSE_LOAD_PAGE_FAULT && stval == ALIAS_VA,
          "a load from an unmapped page is a load page fault");
    stval = 0;
    check(probe_store((u32 *)ALIAS_VA, 1, &stval) == CAUSE_STORE_PAGE_FAULT && stval == ALIAS_VA,
          "a store to an unmapped page is a store page fault");
}

/* Kernel text and rodata are read-only; data is writable. */
static void test_protection(void) {
    u32 *text = (u32 *)(uintptr)__text_start;
    u32 *rodata = (u32 *)(uintptr)__rodata_start;
    u32 first = *text;
    u32 val = 0;
    u32 stval = 0;
    check(probe_store(text, 0, &stval) == CAUSE_STORE_PAGE_FAULT && stval == (u32)(uintptr)text,
          "a store to kernel text is a store page fault");
    check(probe_load(text, &val, &stval) == 0 && val == first, "kernel text is readable and unchanged");
    stval = 0;
    check(probe_store(rodata, 0, &stval) == CAUSE_STORE_PAGE_FAULT && stval == (u32)(uintptr)rodata,
          "a store to rodata is a store page fault");
    static u32 data_word;
    check(probe_store(&data_word, 0x600du, &stval) == 0 && data_word == 0x600du, "data is writable");
}

/* Builds a separate page table, maps and unmaps in it, and frees it: every
 * page comes back. */
static void test_pagetable(void) {
    u32 before = kfree_pages();
    pagetable_t pt = pagetable_create();
    u32 p = (u32)(uintptr)kalloc();
    u32 q = (u32)(uintptr)kalloc();
    check(pt != NULL && p != 0 && q != 0, "allocate a page table and two pages");

    /* Two pages either side of a 4 MiB boundary: two second-level tables. */
    check(map_pages(pt, SPLIT_VA, p, PGSIZE, PTE_R | PTE_W) == 0 &&
              map_pages(pt, SPLIT_VA + PGSIZE, q, PGSIZE, PTE_R) == 0,
          "map two pages across a 4 MiB boundary");
    pte_t *a = walk(pt, SPLIT_VA, false);
    pte_t *b = walk(pt, SPLIT_VA + PGSIZE, false);
    check(a != NULL && *a == (PA2PTE(p) | PTE_R | PTE_W | PTE_V) && b != NULL &&
              *b == (PA2PTE(q) | PTE_R | PTE_V),
          "walk finds both PTEs");
    check(walk(pt, RANGE_VA, false) == NULL, "walk without alloc doesn't create tables");

    /* A multi-page range maps consecutive physical pages. */
    check(map_pages(pt, RANGE_VA, p, 3 * PGSIZE, PTE_R | PTE_X) == 0, "map a three-page range");
    bool consecutive = true;
    for (u32 i = 0; i < 3; i++) {
        pte_t *pte = walk(pt, RANGE_VA + i * PGSIZE, false);
        consecutive = consecutive && pte != NULL && *pte == (PA2PTE(p + i * PGSIZE) | PTE_R | PTE_X | PTE_V);
    }
    check(consecutive, "the range maps consecutive pages");
    unmap_pages(pt, RANGE_VA, 3 * PGSIZE, false);
    check(*walk(pt, RANGE_VA + PGSIZE, false) == 0, "unmap_pages clears the PTEs");

    unmap_pages(pt, SPLIT_VA, 2 * PGSIZE, true);
    pagetable_free(pt);
    check(kfree_pages() == before, "pagetable_free returns every page");
}

int vm_selftest(void) {
    test_alloc_all();
    test_alias();
    test_protection();
    test_pagetable();
    kprintf("vm self-test: %d/%d passed\n", passed, passed + failed);
    return failed;
}
