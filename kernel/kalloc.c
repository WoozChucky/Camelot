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
phys_addr_t dtb_base;
usize dtb_size;

static struct spinlock kmem_lock;
static struct run *freelist;
static u32 nfree;

/* Whether physical address `a` is in the `size` bytes from `base`: compared
 * as an offset, so that a range ending at the top of the address space
 * needs no end. */
static bool in_range(phys_addr_t a, phys_addr_t base, usize size) {
    return a - base < size;
}

/* Whether physical address `a` is a page the allocator manages: a page of
 * RAM that is neither the kernel image's, the firmware's nor the device
 * tree's. */
static bool is_free_page(phys_addr_t a) {
    phys_addr_t start = pa(__text_start);
    return a % PGSIZE == 0 && in_range(a, ram_base, ram_size) &&
           !in_range(a, start, pa(__kernel_end) - start) && !in_range(a, reserved_base, reserved_size) &&
           !in_range(a, dtb_base, dtb_size);
}

/* A big-endian 32-bit word of the device tree: every number in one is
 * big-endian, whatever the machine's byte order. */
static u32 fdt32(const u8 *p) {
    return (u32)p[0] << 24 | (u32)p[1] << 16 | (u32)p[2] << 8 | (u32)p[3];
}

/* Keeps the device tree the firmware passed in a1 (dtb_early_pa, entry.S)
 * out of the free list, its whole pages, as Linux's setup_bootmem()
 * memblock_reserve()s dtb_early_pa for fdt_totalsize() bytes (rather than
 * early_init_fdt_reserve_self(), whose __pa() a fixmap pointer defeats):
 * the tree is still there, intact, for whoever reads it. Only a tree in
 * the RAM the direct map holds, whose header (the magic, then totalsize)
 * says it is one; and only RAM's part of it. Camelot's firmware puts it at
 * the last 2 MiB boundary that leaves it room before the end of RAM, as
 * QEMU does, OpenSBI's fw_jump at 0x8220_0000 (spec section 7,
 * "Firmware"). */
static void reserve_dtb(void) {
    phys_addr_t dtb = dtb_early_pa;
    if (dtb % 4 != 0 || !in_range(dtb, ram_base, ram_size) || ram_size - (dtb - ram_base) < 8 ||
        in_range(dtb, reserved_base, reserved_size)) {
        return;
    }
    const u8 *header = va(dtb);
    if (fdt32(header) != FDT_MAGIC) {
        return;
    }
    u64 end = dtb + fdt32(header + 4);
    if (end - ram_base > ram_size) {
        end = ram_base + ram_size;
    }
    dtb_base = PGROUNDDOWN(dtb);
    dtb_size = PGROUNDUP(end) - dtb_base;
}

void kinit(void) {
    initlock(&kmem_lock, "kmem");
    sysinfo_check();
    ram_base = sysinfo_read_reg(SYS_RAM_BASE);
    ram_size = sysinfo_read_reg(SYS_RAM_SIZE);
    /* What the firmware keeps, as Linux leaves a no-map /reserved-memory
     * region alone: the kernel loads right after it. */
    reserved_base = sysinfo_read_reg(SYS_RESERVED_BASE);
    reserved_size = sysinfo_read_reg(SYS_RESERVED_SIZE);
    /* Physical addresses: the image is linked high. */
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
    /* The allocator hands out direct-map pointers, so it gets the RAM the
     * direct map holds: in Sv39 128 GiB (vm.h). */
    if (ram_size > direct_map_size) {
        kprintf("ram: %lu MiB above the direct map left out\n", (ram_size - direct_map_size) >> 20);
        ram_size = direct_map_size;
    }
    reserve_dtb();
    /* Every page of RAM but the image's, the firmware's and the device
     * tree's, from the bottom up, so the highest is handed out first. One
     * lock for all of them rather than one per kfree(): tens of thousands of
     * pages, and nothing else runs yet. */
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
    /* A direct-map pointer: an image address converts to a page of the
     * image, which is no free page either. */
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
