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
#include "sysinfo.h"
#include "trap.h"

struct run {
    struct run *next;
};

u32 ram_base;
u32 ram_size;

static struct spinlock kmem_lock;
static struct run *freelist;
static u32 nfree;

/* Whether `pa` is a page the allocator manages. */
static bool is_free_page(u32 a) {
    return a % PGSIZE == 0 && a >= (u32)(uintptr)__kernel_end && a - ram_base < ram_size;
}

void kinit(void) {
    initlock(&kmem_lock, "kmem");
    sysinfo_check();
    ram_base = sysinfo_read(SYS_RAM_BASE);
    ram_size = sysinfo_read(SYS_RAM_SIZE);
    u32 start = (u32)(uintptr)__text_start;
    u32 end = (u32)(uintptr)__kernel_end;
    if (start != ram_base) {
        panic("kinit: the kernel is not at the start of RAM");
    }
    if (ram_size % PGSIZE != 0) {
        panic("kinit: the RAM size is not a whole number of pages");
    }
    if (end - ram_base > ram_size) {
        panic("kinit: the kernel is larger than RAM");
    }
    /* Count pages rather than compare addresses: with 2 GiB of RAM the end
     * of RAM is 2^32, which doesn't fit in a u32. */
    u32 pages = (ram_size - (end - ram_base)) / PGSIZE;
    /* One lock for all of them rather than one per kfree(): tens of
     * thousands of pages, and nothing else runs yet. */
    acquire(&kmem_lock);
    for (u32 i = 0; i < pages; i++) {
        struct run *r = (struct run *)(uintptr)(end + i * PGSIZE);
        r->next = freelist;
        freelist = r;
    }
    nfree += pages;
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
    u32 *p = kalloc();
    if (p != NULL) {
        for (u32 i = 0; i < PGSIZE / 4; i++) {
            p[i] = 0;
        }
    }
    return p;
}

void kfree(void *pa) {
    if (!is_free_page((u32)(uintptr)pa)) {
        kprintf("kfree: bad page %p\n", pa);
        panic("kfree");
    }
    struct run *r = pa;
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
