/* Physical page allocator (spec section 10, "Kernel memory"). Every free 4 KiB
 * page of RAM, all of it but the kernel image's, the firmware's and the
 * device tree's, is on one free list. RAM's base and size come from sysinfo, so the allocator
 * follows Grail's --ram, and so does the firmware's region.
 * The pages are handed out as kernel pointers: their direct-map addresses
 * (va(), vm.h). */
#ifndef CAMELOT_KALLOC_H
#define CAMELOT_KALLOC_H

#include "types.h"

/* RAM as sysinfo reports it (set by kinit), physical: ram_size is what the
 * direct map holds of it. */
extern phys_addr_t ram_base;
extern usize ram_size;
/* The RAM the firmware keeps (sysinfo's SYS_RESERVED_BASE and _SIZE),
 * physical: never handed out. A size of 0 without firmware. */
extern phys_addr_t reserved_base;
extern usize reserved_size;
/* The pages of the device tree the firmware passed (dtb_early_pa, set by
 * kinit from its header's totalsize), physical: never handed out,
 * as Linux memblock_reserve()s the tree. A size of 0 without one. */
extern phys_addr_t dtb_base;
extern usize dtb_size;
/* The device tree's physical address, the firmware's a1, which _start
 * keeps (entry.S), as Linux's setup_vm() keeps dtb_early_pa. */
extern phys_addr_t dtb_early_pa;

/* A flattened device tree's first word (the Devicetree Specification,
 * chapter 5), big-endian in the tree. */
#define FDT_MAGIC 0xd00dfeedu

/* Reads RAM's range and the firmware's from sysinfo, finds the device
 * tree's pages, and frees every page of RAM but the kernel image's, the
 * firmware's and the device tree's. */
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
