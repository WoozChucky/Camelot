/* Physical page allocator (spec section 10, "Kernel memory"). Every free 4 KiB
 * page of RAM, all of it but the kernel image's and the firmware's, is on
 * one free list. RAM's base and size come from sysinfo, so the allocator
 * follows Grail's --ram, and so does the firmware's region on RV64.
 * The pages are handed out as kernel pointers: on RV64 their direct-map
 * addresses (va(), vm.h), on RV32 their physical addresses, which the
 * kernel's identity map makes the same. */
#ifndef CAMELOT_KALLOC_H
#define CAMELOT_KALLOC_H

#include "types.h"

/* RAM as sysinfo reports it (set by kinit), physical. On RV32 ram_base +
 * ram_size is 2^32 with 2 GiB of RAM, which wraps to 0: test
 * `pa - ram_base < ram_size` instead. On RV64 ram_size is what the direct
 * map holds of it. */
extern phys_addr_t ram_base;
extern usize ram_size;
/* The RAM the firmware keeps (sysinfo's SYS_RESERVED_BASE and _SIZE, RV64
 * only), physical: never handed out. A size of 0 without firmware. */
extern phys_addr_t reserved_base;
extern usize reserved_size;

/* Reads RAM's range and the firmware's from sysinfo and frees every page of
 * RAM but the kernel image's and the firmware's. */
void kinit(void);

/* Takes a page off the free list. Its contents are whatever was there last.
 * Returns NULL when no page is free. */
void *kalloc(void);

/* kalloc(), then fills the page with zeros. */
void *kzalloc(void);

/* Puts `page`, a pointer kalloc() returned, back. Panics unless it is a
 * page-aligned address of free RAM (from __kernel_end to the end of RAM). */
void kfree(void *page);

/* Number of pages on the free list. */
u32 kfree_pages(void);

#endif
