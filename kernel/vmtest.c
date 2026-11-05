/* Self-tests for the page allocator and paging, run by kmain with paging on
 * and before interrupts are enabled. */
#include "vmtest.h"
#include "kalloc.h"
#include "kprintf.h"
#include "memlayout.h"
#include "probe.h"
#include "proc.h"
#include "riscv.h"
#include "vm.h"

/* Nothing lives at these addresses, so the tests can map them freely: on
 * RV32 holes in the memory map (spec section 7), on RV64 the vmalloc area's
 * part for tests (VM_TEST_AREA, vm.h), where the kernel has made no table
 * below the root's yet. The two pages at SPLIT_VA straddle a boundary of
 * what one lowest-level table maps (4 MiB on RV32, 2 MiB on RV64): they use
 * two of them. */
#if __riscv_xlen == 64
#define ALIAS_VA (VM_TEST_AREA + LEVEL_SIZE(1))
#define RANGE_VA (VM_TEST_AREA + 16 * LEVEL_SIZE(1))
#else
#define ALIAS_VA 0x40000000u
#define RANGE_VA 0x50000000u
#endif
#define SPLIT_VA (ALIAS_VA - PGSIZE)

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

/* Whether kernel pointer `p` is a free page's: page aligned, and its
 * physical address in RAM after the kernel. */
static bool is_free_ram(void *p) {
    phys_addr_t a = pa(p);
    return a % PGSIZE == 0 && a >= pa(__kernel_end) && a - ram_base < ram_size;
}

/* Takes every page off the free list, counting them, then puts them all back.
 * The pages are chained through their first word while out. */
static void test_alloc_all(void) {
    u32 before = kfree_pages();
    uintptr *chain = NULL;
    u32 n = 0;
    bool in_range = true;
    for (;;) {
        uintptr *p = kalloc();
        if (p == NULL) {
            break;
        }
        in_range = in_range && is_free_ram(p);
        *p = (uintptr)chain;
        chain = p;
        n++;
    }
    check(n == before && n > 0, "kalloc hands out every free page");
    check(in_range, "kalloc pages are aligned and after the kernel");
    check(kfree_pages() == 0 && kalloc() == NULL, "kalloc returns NULL when out of pages");
    /* Out of memory: no new table, and a mapping that needs one fails. */
    check(pagetable_create() == NULL, "pagetable_create fails when out of pages");
    check(map_pages(kernel_pagetable, ALIAS_VA, pa(chain), PGSIZE, PTE_R | PTE_W) == -1,
          "map_pages fails when it can't allocate a page table");
    for (u32 i = 0; i < n; i++) {
        uintptr *next = (uintptr *)*chain;
        kfree(chain);
        chain = next;
    }
    check(chain == NULL && kfree_pages() == before, "kfree takes every page back");
}

/* Fills a page with all ones (every word a valid-looking PTE) and frees it,
 * so it is the next page kalloc hands out. */
static u32 *dirty_page(void) {
    u32 *p = kalloc();
    if (p != NULL) {
        for (u32 i = 0; i < PGSIZE / 4; i++) {
            p[i] = 0xffffffffu;
        }
        kfree(p);
    }
    return p;
}

static bool is_zero_page(const u32 *p) {
    for (u32 i = 0; i < PGSIZE / 4; i++) {
        if (p[i] != 0) {
            return false;
        }
    }
    return true;
}

/* kzalloc and pagetable_create (which walk uses for second-level tables)
 * clear a page that was dirty when freed. RAM starts out zero on Grail, so
 * only a reused page shows whether they do. */
static void test_zeroing(void) {
    u32 *dirty = dirty_page();
    u32 *p = kzalloc();
    check(p != NULL && p == dirty && is_zero_page(p), "kzalloc clears a dirty page");
    kfree(p);

    dirty = dirty_page();
    pagetable_t pt = pagetable_create();
    check(pt != NULL && (void *)pt == (void *)dirty && is_zero_page((const u32 *)pt),
          "pagetable_create clears a dirty page");
    kfree(pt);
}

/* Maps a page a second time, at ALIAS_VA, and writes through each address.
 * The one kernel mapping made without PTE_KERNEL's A and D, on purpose: it
 * checks that the hart sets them (Grail manages them, spec section 5): no
 * other leaf, kernel or user, is ever made without them. */
static void test_alias(void) {
    volatile u32 *page = kalloc();
    volatile u32 *alias = (volatile u32 *)ALIAS_VA;
    check(map_pages(kernel_pagetable, ALIAS_VA, pa(page), PGSIZE, PTE_R | PTE_W) == 0, "map a second mapping");
    sfence_vma();

    alias[0] = 0xc0ffee01u;
    page[PGSIZE / 4 - 1] = 0x5eed1234u;
    check(page[0] == 0xc0ffee01u && alias[PGSIZE / 4 - 1] == 0x5eed1234u,
          "both mappings see the same page");

    /* Grail manages A and D: the stores above set both. */
    pte_t *pte = walk(kernel_pagetable, ALIAS_VA, false);
    check(pte != NULL && PTE2PA(*pte) == pa(page) &&
              (*pte & (PTE_V | PTE_R | PTE_W | PTE_X | PTE_U | PTE_A | PTE_D)) ==
                  (PTE_V | PTE_R | PTE_W | PTE_A | PTE_D),
          "the alias PTE has the page, R+W and A+D");

    /* Unmapped again (the page goes back to the free list), it faults. */
    u32 free_before = kfree_pages();
    unmap_pages(kernel_pagetable, ALIAS_VA, PGSIZE, true);
    sfence_vma();
    check(kfree_pages() == free_before + 1, "unmap_pages frees the page");
    u32 val = 0;
    uintptr stval = 0;
    check(probe_load((const u32 *)ALIAS_VA, &val, &stval) == CAUSE_LOAD_PAGE_FAULT && stval == ALIAS_VA,
          "a load from an unmapped page is a load page fault");
    stval = 0;
    check(probe_store((u32 *)ALIAS_VA, 1, &stval) == CAUSE_STORE_PAGE_FAULT && stval == ALIAS_VA,
          "a store to an unmapped page is a store page fault");
}

/* Kernel text and rodata are read-only; data is writable. */
static void test_protection(void) {
    u32 *text = (u32 *)__text_start;
    u32 *rodata = (u32 *)__rodata_start;
    u32 first = *text;
    u32 val = 0;
    uintptr stval = 0;
    check(probe_store(text, 0, &stval) == CAUSE_STORE_PAGE_FAULT && stval == (uintptr)text,
          "a store to kernel text is a store page fault");
    check(probe_load(text, &val, &stval) == 0 && val == first, "kernel text is readable and unchanged");
    stval = 0;
    check(probe_store(rodata, 0, &stval) == CAUSE_STORE_PAGE_FAULT && stval == (uintptr)rodata,
          "a store to rodata is a store page fault");
    static u32 data_word;
    check(probe_store(&data_word, 0x600du, &stval) == 0 && data_word == 0x600du, "data is writable");
}

/* Builds a separate page table, maps and unmaps in it, and frees it: every
 * page comes back. */
static void test_pagetable(void) {
    u32 before = kfree_pages();
    pagetable_t pt = pagetable_create();
    void *pp = kalloc();
    void *qq = kalloc();
    check(pt != NULL && pp != NULL && qq != NULL, "allocate a page table and two pages");
    phys_addr_t p = pa(pp);
    phys_addr_t q = pa(qq);
    /* Two pages either side of a boundary: two lowest-level tables. */
    check(map_pages(pt, SPLIT_VA, p, PGSIZE, PTE_R | PTE_W) == 0 &&
              map_pages(pt, SPLIT_VA + PGSIZE, q, PGSIZE, PTE_R) == 0,
          "map two pages across a table boundary");
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
    test_zeroing();
    test_alias();
    test_protection();
    test_pagetable();
    kprintf("vm self-test: %d/%d passed\n", passed, passed + failed);
    return failed;
}

#if __riscv_xlen == 64
/* A kernel process that ends at once: what kstack_selftest() runs. */
static void kstack_noop(void) {
}

/* NPROC kernel processes at once, run until all have ended. */
static void kstack_round(void) {
    for (u32 i = 0; i < NPROC; i++) {
        const char *err;
        if (kproc_create("kstack", kstack_noop, &err) == NULL) {
            check(false, "start a kernel process");
        }
    }
    scheduler();
}

/* The pages of every kernel stack in kstack_count()'s `stacks`. */
static u32 stack_pages(u32 stacks) {
    return stacks * (u32)(KSTACK_SIZE / PGSIZE);
}

int kstack_selftest(void) {
    passed = 0;
    failed = 0;
    /* A first round, for the page tables the stacks' slots may need. */
    kstack_round();
    struct kstack_counts n;
    kstack_count(&n);
    u32 before = kfree_pages() + stack_pages(n.cached);
    /* More threads end than this hart caches stacks for: the rest are
     * deferred and unmapped by the scheduler as soon as it holds no lock. */
    for (u32 round = 0; round < 3; round++) {
        kstack_round();
    }
    kstack_count(&n);
    check(n.deferred == 0, "no freed kernel stack is left to unmap");
    check(n.mapped == n.cached && n.cached <= NR_CACHED_STACKS,
          "only this hart's cached kernel stacks stay mapped");
    check(kfree_pages() + stack_pages(n.cached) == before, "every other kernel stack's pages come back");
    kprintf("kstack self-test: %d/%d passed\n", passed, passed + failed);
    return failed;
}
#endif
