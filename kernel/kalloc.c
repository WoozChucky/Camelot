/* Physical page allocator: a singly linked free list threaded through the
 * free pages themselves (a free page's first word points at the next one), so
 * it needs no memory of its own. kalloc pops the head and kfree pushes, both
 * O(1). Unlike xv6, pages are not filled with junk on free and alloc: at about
 * a thousand stores per page that would dominate the boot on Grail.
 *
 * kmem_lock protects the list and the count. It is a leaf of the lock
 * order: nothing else is taken while it is held, so any code may allocate
 * or free a page whatever locks it holds. */
#include "kalloc.h"
#include "kprintf.h"
#include "memlayout.h"
#include "spinlock.h"
#include "string.h"
#include "sysinfo.h"
#include "trap.h"
#include "vm.h"

struct run {
    struct run *next;
};

phys_addr_t ram_base;
usize ram_size;
phys_addr_t reserved_base;
usize reserved_size;

static struct spinlock kmem_lock;
static struct run *freelist;
static u32 nfree;

/* Whether physical address `a` is in the `size` bytes from `base`: compared
 * as an offset, so that a range ending at 2^32 on RV32 needs no end. */
static bool in_range(phys_addr_t a, phys_addr_t base, usize size) {
    return a - base < size;
}

/* Whether physical address `a` is a page the allocator manages: a page of
 * RAM that is neither the kernel image's nor the firmware's. */
static bool is_free_page(phys_addr_t a) {
    phys_addr_t start = pa(__text_start);
    return a % PGSIZE == 0 && in_range(a, ram_base, ram_size) &&
           !in_range(a, start, pa(__kernel_end) - start) && !in_range(a, reserved_base, reserved_size);
}

void kinit(void) {
    initlock(&kmem_lock, "kmem");
    sysinfo_check();
    ram_base = sysinfo_read_reg(SYS_RAM_BASE);
    ram_size = sysinfo_read_reg(SYS_RAM_SIZE);
#if __riscv_xlen == 64
    /* What the firmware keeps, as Linux leaves a no-map /reserved-memory
     * region alone: the RV64 kernel loads right after it. */
    reserved_base = sysinfo_read_reg(SYS_RESERVED_BASE);
    reserved_size = sysinfo_read_reg(SYS_RESERVED_SIZE);
#endif
    /* Physical addresses: the image is linked high on RV64. */
    phys_addr_t start = pa(__text_start);
    phys_addr_t end = pa(__kernel_end);
    if (!in_range(start, ram_base, ram_size)) {
        panic("kinit: the kernel is not in RAM");
    }
    if (ram_size % PGSIZE != 0 || reserved_size % PGSIZE != 0 || reserved_base % PGSIZE != 0) {
        panic("kinit: the RAM size is not a whole number of pages");
    }
    if (end - ram_base > ram_size) {
        panic("kinit: the kernel is larger than RAM");
    }
    if (in_range(start, reserved_base, reserved_size) || in_range(reserved_base, start, end - start)) {
        panic("kinit: the kernel overlaps the firmware's RAM");
    }
#if __riscv_xlen == 64
    /* The allocator hands out direct-map pointers, so it gets the RAM the
     * direct map holds: in Sv39 128 GiB (vm.h). */
    if (ram_size > direct_map_size) {
        kprintf("ram: %lu MiB above the direct map left out\n", (ram_size - direct_map_size) >> 20);
        ram_size = direct_map_size;
    }
#endif
    /* Every page of RAM but the image's and the firmware's, from the
     * bottom up, so the highest is handed out first. Count pages rather
     * than compare addresses: with 2 GiB of RAM on RV32 the end of RAM is
     * 2^32, which doesn't fit in a u32. One lock for all of them rather
     * than one per kfree(): tens of thousands of pages, and nothing else
     * runs yet. */
    usize pages = ram_size / PGSIZE;
    acquire(&kmem_lock);
    for (usize i = 0; i < pages; i++) {
        phys_addr_t a = ram_base + i * PGSIZE;
        if (!is_free_page(a)) {
            continue;
        }
        struct run *r = va(a);
        r->next = freelist;
        freelist = r;
        nfree++;
    }
    release(&kmem_lock);
}

void *kalloc(void) {
    acquire(&kmem_lock);
    struct run *r = freelist;
    if (r != NULL) {
        freelist = r->next;
        nfree--;
    }
    release(&kmem_lock);
    return r;
}

void *kzalloc(void) {
    void *p = kalloc();
    if (p != NULL) {
        /* Linux's clear_page() on riscv without Zicboz: memset(), with
         * register-wide stores. */
        memset(p, 0, PGSIZE);
    }
    return p;
}

void kfree(void *page) {
    /* A direct-map pointer (on RV32 a physical address): an image address
     * converts to a page of the image, which is no free page either. */
    if (!is_free_page(pa(page))) {
        kprintf("kfree: bad page %p\n", page);
        panic("kfree");
    }
    struct run *r = page;
    acquire(&kmem_lock);
    r->next = freelist;
    freelist = r;
    nfree++;
    release(&kmem_lock);
}

u32 kfree_pages(void) {
    /* One aligned word: the count at some moment, as good as the lock
     * would give once it is let go. */
    return nfree;
}
