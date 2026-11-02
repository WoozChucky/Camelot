/* Sv32 virtual memory (spec section 10, "Kernel memory").
 *
 * A virtual address splits 10/10/12: VPN[1] (bits 31:22) indexes the root
 * page table, VPN[0] (bits 21:12) a second-level table, and the low 12 bits
 * are the offset in the 4 KiB page. Each table is one page of 1024 PTEs. A
 * PTE holds a 22-bit physical page number in bits 31:10 and the flags below.
 * A PTE with R, W or X set is a leaf; a valid one without them points at the
 * next table. Grail sets A and D itself (hardware-managed) and has no TLB.
 *
 * The kernel only makes 4 KiB leaves, so walk() and unmap_pages() need not
 * handle 4 MiB megapages (leaves in the root table). */
#ifndef CAMELOT_VM_H
#define CAMELOT_VM_H

#include "types.h"

typedef u32 pte_t;
typedef pte_t *pagetable_t; /* one page: 1024 PTEs */

#define PTE_V (1u << 0) /* valid */
#define PTE_R (1u << 1) /* readable */
#define PTE_W (1u << 2) /* writable */
#define PTE_X (1u << 3) /* executable */
#define PTE_U (1u << 4) /* user-mode accessible */
#define PTE_G (1u << 5) /* global */
#define PTE_A (1u << 6) /* accessed (set by the hardware) */
#define PTE_D (1u << 7) /* dirty (set by the hardware) */

#define PTE_PERM_MASK (PTE_R | PTE_W | PTE_X | PTE_U)

/* Physical address <-> PTE. */
#define PA2PTE(pa) (((u32)(pa) >> 12) << 10)
#define PTE2PA(pte) (((pte) >> 10) << 12)

/* VPN[level] of a virtual address. */
#define VPN(va, level) (((u32)(va) >> (12 + 10 * (level))) & 0x3ff)

/* satp value that turns Sv32 on with root table `pt`. */
#define MAKE_SATP(pt) (SATP_SV32 | ((u32)(uintptr)(pt) >> 12))

/* The kernel's page table, built by kvminit. NULL until then. */
extern pagetable_t kernel_pagetable;

/* A new, empty page table (a zeroed page), or NULL if no page is free. */
pagetable_t pagetable_create(void);

/* The level-0 PTE for `va` in `pt`. With `alloc`, creates the second-level
 * table if it is missing; returns NULL if that needs a page and none is free,
 * or (without `alloc`) if there is no second-level table. */
pte_t *walk(pagetable_t pt, u32 va, bool alloc);

/* Maps the `size` bytes at `va` to the physical range at `pa`, with
 * permissions `perm` (some of PTE_R/W/X/U). `va`, `pa` and `size` must be page
 * aligned and `size` non-zero; mapping a page that is already mapped panics.
 * Returns 0, or -1 if a page table could not be allocated (the pages mapped
 * before that stay mapped). The range may end at 2^32. */
int map_pages(pagetable_t pt, u32 va, u32 pa, u32 size, u32 perm);

/* Removes the mappings of the `size` bytes at `va`, which must all be mapped
 * (else panic). With `free_pages`, also kfree()s the physical pages. The
 * second-level tables stay; pagetable_free() releases them. */
void unmap_pages(pagetable_t pt, u32 va, u32 size, bool free_pages);

/* Frees `pt` and its second-level tables. Every leaf must be unmapped first
 * (else panic): the pages they map belong to someone else. */
void pagetable_free(pagetable_t pt);

/* Builds kernel_pagetable: identity maps of the kernel text (R+X), rodata
 * (R), the rest of RAM (R+W) and every device in the sysinfo table (R+W). */
void kvminit(void);

/* Turns paging on with kernel_pagetable (satp, then sfence.vma). */
void kvminithart(void);

/* For page fault reports: prints how `pt` maps `va`, as
 * "  va 0x... is not mapped" or "  va 0x... maps to pa 0x..., pte 0x... DAGUXWRV". */
void vm_print_mapping(pagetable_t pt, u32 va);

/* The page table satp points at, or NULL while paging is off. */
pagetable_t vm_active_pagetable(void);

/* User address spaces (spec section 10, "Kernel processes"). User space is
 * the low 4 MiB, root entry 0: the program from USER_BASE (page 0 stays
 * unmapped, so a null pointer faults), the stack in the last page below
 * USER_TOP, and an unmapped guard page under it. Every other root entry is a
 * copy of the kernel's, so the kernel's mappings (none with U) are there in
 * every process and a trap from U-mode needs no satp switch. */
#define USER_BASE PGSIZE
#define USER_TOP 0x00400000u
#define USER_STACK_SIZE PGSIZE
#define USER_STACK (USER_TOP - USER_STACK_SIZE)
/* Program segments end at or below this: one guard page under the stack. */
#define USER_LOAD_TOP (USER_STACK - PGSIZE)
/* Root entries that belong to user space (VPN[1] of USER_TOP). */
#define USER_ROOT_ENTRIES 1

/* A page table for a new process: the kernel's root entries from
 * USER_ROOT_ENTRIES on (sharing its second-level tables), nothing in user
 * space. NULL if no page is free. Must be called after kvminit(), and the
 * kernel must not add root entries afterwards. */
pagetable_t uvm_create(void);

/* Frees a table from uvm_create(): every page mapped in user space, the
 * user-space second-level tables and the root. The kernel's tables stay. */
void uvm_free(pagetable_t pt);

/* Whether every byte of [va, va + len) is user memory in `pt` mapped with
 * U and all of `perm` (PTE_R, PTE_W). len 0 is always fine. */
bool uvm_range_ok(pagetable_t pt, u32 va, u32 len, u32 perm);

/* Copies `len` bytes from user address `va` in `pt` to `dst`, through the
 * physical pages: the kernel never dereferences a user address, so
 * sstatus.SUM stays clear. All or nothing: returns -1, having copied
 * nothing, unless uvm_range_ok(pt, va, len, PTE_R); else 0. */
int copyin(pagetable_t pt, void *dst, u32 va, u32 len);

#endif
