/* Physical page allocator (spec section 10, "Kernel memory"). Every free 4 KiB
 * page of RAM from __kernel_end to the end of RAM is on one free list. RAM's
 * base and size come from sysinfo, so the allocator follows Grail's --ram. */
#ifndef CAMELOT_KALLOC_H
#define CAMELOT_KALLOC_H

#include "types.h"

/* RAM as sysinfo reports it (set by kinit). ram_base + ram_size is 2^32 with
 * 2 GiB of RAM, which wraps to 0: test `pa - ram_base < ram_size` instead. */
extern u32 ram_base;
extern u32 ram_size;

/* Reads the RAM range from sysinfo and frees every page after the kernel. */
void kinit(void);

/* Takes a page off the free list. Its contents are whatever was there last.
 * Returns NULL when no page is free. */
void *kalloc(void);

/* kalloc(), then fills the page with zeros. */
void *kzalloc(void);

/* Puts the page at `pa` back. Panics unless it is a page-aligned address of
 * free RAM (from __kernel_end to the end of RAM). */
void kfree(void *pa);

/* Number of pages on the free list. */
u32 kfree_pages(void);

#endif
