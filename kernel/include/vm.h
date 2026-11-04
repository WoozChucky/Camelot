/* Virtual memory (spec section 10, "Kernel memory"): page tables, the
 * kernel's address space, and the conversions between kernel pointers and
 * physical addresses.
 *
 * Page tables. A table is one 4 KiB page of PTEs. A PTE with R, W or X set
 * is a leaf; a valid one without them points at the next table down. Grail
 * sets A and D itself (hardware-managed) and has no TLB.
 *
 * - RV32, Sv32: two levels of 1024 4-byte PTEs. A virtual address splits
 *   10/10/12: VPN[1] (bits 31:22) indexes the root, VPN[0] (bits 21:12) a
 *   second-level table, the low 12 bits are the offset in the page. A PTE
 *   holds a 22-bit physical page number in bits 31:10. A leaf in the root
 *   maps a 4 MiB megapage.
 * - RV64, Sv48 or Sv39: four or three levels (PT_LEVELS, chosen at boot,
 *   below) of 512 8-byte PTEs. A virtual address splits 9/9/9/9/12 (Sv48) or
 *   9/9/9/12 (Sv39), and must be canonical: bits 63 down to the top one
 *   used (47 or 38) all equal. A PTE holds a 44-bit physical page number in
 *   bits 53:10. A leaf at level 1 maps a 2 MiB page, at level 2 a 1 GiB
 *   one, at level 3 (Sv48) 512 GiB.
 *
 * The kernel makes superpages only in its own address space (kvminit), for
 * the stretches of RAM and devices a range covers whole. Nothing ever
 * unmaps them, so walk() and unmap_pages() need not handle them: walk()
 * panics on one. */
#ifndef CAMELOT_VM_H
#define CAMELOT_VM_H

#include "memlayout.h"
#include "types.h"

#if __riscv_xlen == 64
typedef u64 pte_t;
#else
typedef u32 pte_t;
#endif
typedef pte_t *pagetable_t; /* one page: PTES_PER_TABLE PTEs */

#define PTE_V (1u << 0) /* valid */
#define PTE_R (1u << 1) /* readable */
#define PTE_W (1u << 2) /* writable */
#define PTE_X (1u << 3) /* executable */
#define PTE_U (1u << 4) /* user-mode accessible */
#define PTE_G (1u << 5) /* global */
#define PTE_A (1u << 6) /* accessed (set by the hardware) */
#define PTE_D (1u << 7) /* dirty (set by the hardware) */

#define PTE_PERM_MASK (PTE_R | PTE_W | PTE_X | PTE_U)
#define PTE_LEAF (PTE_R | PTE_W | PTE_X)

#if __riscv_xlen == 64
#define PTES_PER_TABLE 512
#define VPN_BITS 9
/* Physical address <-> PTE: the PPN is bits 53:10. */
#define PA2PTE(pa) ((pte_t)((u64)(pa) >> 12) << 10)
#define PTE2PA(pte) ((phys_addr_t)((((u64)(pte) >> 10) & 0xfffffffffffull) << 12))
/* VPN[level] of a virtual address. */
#define VPN(va, level) (((uintptr)(va) >> (12 + 9 * (level))) & 0x1ff)
/* The paging mode, chosen once at boot by probing satp (vm.c, setup_vm(),
 * as Linux's set_satp_mode()): Sv48 where the hart has it, Sv39 otherwise.
 * pt_levels is 4 or 3, satp_mode SATP_MODE_SV48 or SATP_MODE_SV39. */
extern u32 pt_levels;
extern uintptr satp_mode;
#define PT_LEVELS pt_levels
/* satp value that turns paging on with root table `pt`, a kernel pointer. */
#define MAKE_SATP(pt) (satp_mode | (pa(pt) >> PGSHIFT))
#else
#define PTES_PER_TABLE 1024
#define VPN_BITS 10
#define PA2PTE(pa) (((u32)(pa) >> 12) << 10)
#define PTE2PA(pte) (((pte) >> 10) << 12)
#define VPN(va, level) (((u32)(va) >> (12 + 10 * (level))) & 0x3ff)
#define PT_LEVELS 2
#define MAKE_SATP(pt) (SATP_SV32 | (pa(pt) >> PGSHIFT))
#endif

/* The bytes a leaf at `level` maps: 4 KiB at level 0, then PTES_PER_TABLE
 * times as much per level. */
#define LEVEL_SIZE(level) ((uintptr)PGSIZE << (VPN_BITS * (level)))

/* ---- The kernel's address space ----
 *
 * RV32: the kernel's mappings are the identity, so a physical address is a
 * kernel pointer and pa()/va() change nothing.
 *
 * RV64: the upper half of the address space is the kernel's, laid out as
 * Linux's riscv64 kernel lays it out (Documentation/arch/riscv/vm-layout.rst;
 * root entries 256-511 in Sv48, which every process's root table copies,
 * #155). From the top:
 *
 *   KERNEL_LINK_ADDR  0xffff_ffff_8000_0000  the kernel image (kernel.lds.S),
 *                                            the top 2 GiB, in both modes
 *   PAGE_OFFSET       Sv48 0xffff_af80_0000_0000 (64 TiB)
 *                     Sv39 0xffff_ffd6_0000_0000 (128 GiB)
 *                     the direct map: all of RAM, its base at PAGE_OFFSET,
 *                     with 1 GiB and 2 MiB leaves where alignment allows
 *   VMALLOC_START     Sv48 0xffff_8f80_0000_0000 (32 TiB, to PAGE_OFFSET)
 *                     Sv39 0xffff_ffc6_0000_0000 (64 GiB)
 *                     the vmalloc/ioremap area: first the devices, each at
 *                     VMALLOC_START + its physical address (ioremap(): every
 *                     device of the machine is below IO_WINDOW_SIZE, 4 GiB),
 *                     then the kernel stacks (kstack_alloc(), from
 *                     KSTACK_AREA)
 *
 * Linux's ioremap() finds free room in the vmalloc area for each call;
 * Camelot's maps each device at a fixed distance from its physical address,
 * since its devices sit in the bottom 4 GiB and are all mapped at boot.
 *
 * The values that depend on the mode are variables, set at boot, as Linux's
 * kernel_map and PAGE_OFFSET are: the image's link address is the same in
 * both modes, so the kernel is linked once. */
#if __riscv_xlen == 64
#define KERNEL_LINK_ADDR 0xffffffff80000000ull
#define PAGE_OFFSET_SV48 0xffffaf8000000000ull
#define PAGE_OFFSET_SV39 0xffffffd600000000ull
#define VMALLOC_START_SV48 0xffff8f8000000000ull
#define VMALLOC_START_SV39 0xffffffc600000000ull
/* The devices' window at the start of the vmalloc area. */
#define IO_WINDOW_SIZE 0x100000000ull
/* The direct map's room: from PAGE_OFFSET up to the next region (the KASAN
 * shadow in Linux's tables, unused here), 64 TiB in Sv48, 128 GiB in Sv39.
 * RAM beyond it is left out (kinit says how much). */
#define DIRECT_MAP_SIZE_SV48 0x400000000000ull
#define DIRECT_MAP_SIZE_SV39 0x2000000000ull

/* Set by setup_vm() before paging goes on, read-only afterwards. */
extern uintptr page_offset;   /* PAGE_OFFSET */
extern uintptr vmalloc_start; /* VMALLOC_START */
extern u64 direct_map_size;   /* the most of RAM the direct map holds */
/* A direct-map pointer minus its physical address: PAGE_OFFSET minus RAM's
 * base (Linux's kernel_map.va_pa_offset). */
extern uintptr va_pa_offset;
/* A kernel-image pointer minus its physical address: KERNEL_LINK_ADDR minus
 * where the boot ROM or the bootloader loaded the image (Linux's
 * kernel_map.va_kernel_pa_offset). entry.S adds it to jump to the high
 * addresses. */
extern uintptr va_kernel_pa_offset;
#define PAGE_OFFSET page_offset
#define VMALLOC_START vmalloc_start

/* The physical address of kernel pointer `v`: a direct-map pointer, or one
 * into the kernel image (its code, data and boot stack), as Linux's __pa().
 * Page tables, satp and the devices' DMA registers take physical addresses. */
static inline phys_addr_t kernel_pa(uintptr v) {
    if (v >= KERNEL_LINK_ADDR) {
        return v - va_kernel_pa_offset;
    }
    return v - va_pa_offset;
}
#define pa(v) kernel_pa((uintptr)(v))
/* The kernel's pointer to physical address `p` of RAM: its direct-map
 * address, as Linux's __va(). */
#define va(p) ((void *)((uintptr)(p) + va_pa_offset))
#else
#define pa(v) ((phys_addr_t)(uintptr)(v))
#define va(p) ((void *)(uintptr)(p))
#endif

/* The kernel's address of the device registers at physical address `p`
 * (`size` bytes, all mapped by kvminit(), which maps every device in the
 * sysinfo table): VMALLOC_START + p on RV64, p itself on RV32. Panics if the
 * range is outside the devices' window. After paging is on. */
uintptr ioremap(phys_addr_t p, usize size);

/* ---- Kernel stacks ----
 *
 * RV64: KSTACK_SIZE, 16 KiB per kernel thread (Linux riscv64's THREAD_SIZE),
 * in the vmalloc area with an unmapped guard page below (Linux's VMAP_STACK).
 * Each stack is the lower half of a KSTACK_SLOT of twice its size, aligned
 * to that, so a stack pointer below the stack has bit KSTACK_SHIFT set:
 * kernel_vec tests that bit, and a trap whose frame would land in the guard
 * page is reported as a kernel stack overflow (kstack_overflow()) instead of
 * faulting again and again. The trap frame of a process's thread sits at the
 * top of its stack.
 *
 * RV32: one page from kalloc, without a guard (the kernel's mappings are the
 * identity there, and RV32 goes in #159). */
#if __riscv_xlen == 64
#define KSTACK_SHIFT 14
#define KSTACK_SIZE (1ul << KSTACK_SHIFT)
#define KSTACK_SLOT (2 * KSTACK_SIZE)
/* Where the stacks start: after the devices' window. They take what one
 * root entry of the vmalloc area holds (kvminit() makes that entry, before
 * the first process copies the kernel's root entries). */
#define KSTACK_AREA (VMALLOC_START + IO_WINDOW_SIZE)
/* The second half of that root entry's room: for the paging self-tests
 * (vmtest.c, smp.c), which map and unmap pages there. The stacks end below
 * it: 256 GiB of them in Sv48, 512 MiB (16384 stacks) in Sv39. */
#define VM_TEST_AREA (KSTACK_AREA + LEVEL_SIZE(PT_LEVELS - 1) / 2)
/* Each hart's overflow stack, for the report of an overflow: kernel_vec
 * puts the frame at overflow_stacks + (hart + 1) * OVERFLOW_STACK_SIZE. */
#define OVERFLOW_STACK_SIZE 4096
extern u8 overflow_stacks[];
#else
#define KSTACK_SIZE PGSIZE
#endif

/* A kernel stack: returns its lowest address (the stack is the KSTACK_SIZE
 * bytes from there; its top is that plus KSTACK_SIZE), or NULL if no memory
 * is left. Must be called with no spinlock held (it maps pages). */
void *kstack_alloc(void);

/* Gives back a stack from kstack_alloc(), which nothing may run on any
 * more. May be called with spinlocks held: on RV64 the stack stays mapped,
 * cached for the next kstack_alloc() (as Linux caches freed thread stacks),
 * since unmapping it would need a TLB shootdown. */
void kstack_free(void *stack);

/* The deepest use of any kernel stack so far, in bytes: every stack is
 * zeroed when it is first made, and the lowest non-zero word of each is how
 * far down it has been used (Linux's CONFIG_DEBUG_STACK_USAGE). RV64 only
 * keeps track; RV32 returns 0. */
usize kstack_max_used(void);

/* ---- Page tables ---- */

/* The kernel's page table, built by kvminit. NULL until then. */
extern pagetable_t kernel_pagetable;

/* A new, empty page table (a zeroed page), or NULL if no page is free. */
pagetable_t pagetable_create(void);

/* The level-0 PTE for `va` in `pt`. With `alloc`, creates the tables on the
 * way down if they are missing; returns NULL if that needs a page and none is
 * free, or (without `alloc`) if a table is missing. Panics on a superpage on
 * the way. */
pte_t *walk(pagetable_t pt, uintptr va, bool alloc);

/* Maps the `size` bytes at `va` to the physical range at `pa`, with
 * permissions `perm` (some of PTE_R/W/X/U). `va`, `pa` and `size` must be page
 * aligned and `size` non-zero; mapping a page that is already mapped panics.
 * Returns 0, or -1 if a page table could not be allocated (the pages mapped
 * before that stay mapped). The range may end at the top of the address
 * space. */
int map_pages(pagetable_t pt, uintptr va, phys_addr_t pa, usize size, u32 perm);

/* Removes the mappings of the `size` bytes at `va`, which must all be mapped
 * (else panic). With `free_pages`, also kfree()s the physical pages. The
 * tables stay; pagetable_free() releases them. In between, a TLB shootdown
 * (smp.h) makes sure no hart still translates to the pages: so the caller
 * holds no spinlock, unless no other hart can be using `pt`. */
void unmap_pages(pagetable_t pt, uintptr va, usize size, bool free_pages);

/* Frees `pt` and every table under it. Every leaf must be unmapped first
 * (else panic): the pages they map belong to someone else. */
void pagetable_free(pagetable_t pt);

#if __riscv_xlen == 64
/* With paging off, on hart 0, before anything else in S-mode (entry.S):
 * probes the paging mode, reads RAM from sysinfo and builds the early page
 * table, which maps the kernel image at its physical address and at its
 * link address, RAM in the direct map and the devices' window, with
 * superpages. Leaves early_satp, which entry.S writes to satp on every hart
 * before it jumps to the high addresses. */
void setup_vm(void);
extern uintptr early_satp;
/* satp for kernel_pagetable, set by kvminit(): what the other harts switch
 * to in entry.S before they use their stacks, which only it maps. */
extern uintptr kernel_satp;
#endif

/* Builds kernel_pagetable: the kernel image (text R+X, rodata R, the rest
 * R+W), all of RAM (R+W) and every device in the sysinfo table (R+W). On
 * RV32 every one at its physical address; on RV64 in the kernel half, the
 * image at its link address, RAM in the direct map and the devices in their
 * window, with no identity mapping left. */
void kvminit(void);

/* Turns paging on with kernel_pagetable (satp, then sfence.vma). On RV64 this
 * drops the early page table, and with it the identity mapping of the image. */
void kvminithart(void);

/* For page fault reports: prints how `pt` maps `va`, as
 * "  va 0x... is not mapped" or "  va 0x... maps to pa 0x..., pte 0x... DAGUXWRV". */
void vm_print_mapping(pagetable_t pt, uintptr va);

/* The page table satp points at, or NULL while paging is off. */
pagetable_t vm_active_pagetable(void);

/* User address spaces (spec section 10, "Kernel processes").
 *
 * RV32: user space is the low 192 MiB, root entries 0 to 47, up to the PLIC,
 * the lowest device the kernel uses (the test device and the CLINT below it
 * are M-mode's):
 *
 *   USER_TOP          0x0c000000
 *                     the stack, grown down on demand (proc_grow_stack),
 *                     at most USER_STACK_MAX
 *   USER_STACK_LIMIT  0x0b800000
 *                     an unmapped guard page
 *   USER_LOAD_TOP     0x0b7ff000
 *                     the heap (sbrk), up from the program
 *                     the program's segments, from USER_BASE
 *   USER_BASE         0x00001000
 *                     page 0, never mapped, so a null pointer faults
 *
 * RV64: user space is the lower half of the address space, root entries 0
 * to 255 (Linux's TASK_SIZE: 128 TiB in Sv48, 256 GiB in Sv39), with the
 * same order of stack, guard page, heap and program. The process side is
 * #155's.
 *
 * Every other root entry is a copy of the kernel's, so the kernel's mappings
 * (none with U) are there in every process and a trap from U-mode needs no
 * satp switch. */
#define USER_BASE PGSIZE
#if __riscv_xlen == 64
/* The top of user space for the paging mode: 2^47 (Sv48) or 2^38 (Sv39),
 * set by setup_vm(). */
extern uintptr user_top;
#define USER_TOP user_top
#define USER_ROOT_ENTRIES (PTES_PER_TABLE / 2)
#else
#define USER_TOP 0x0c000000u
/* Root entries that belong to user space (VPN[1] of USER_TOP). */
#define USER_ROOT_ENTRIES (USER_TOP >> 22)
#endif
/* How far the stack may grow: Linux's default limit. */
#define USER_STACK_MAX (8u << 20)
/* The stack's lowest address: it never grows below this. */
#define USER_STACK_LIMIT (USER_TOP - USER_STACK_MAX)
/* The stack's first page, mapped when the program starts, with its
 * arguments at the top. */
#define USER_STACK (USER_TOP - PGSIZE)
/* Program segments and the heap end at or below this: one guard page under
 * the stack's limit. */
#define USER_LOAD_TOP (USER_STACK_LIMIT - PGSIZE)

/* A page table for a new process: the kernel's root entries from
 * USER_ROOT_ENTRIES on (sharing its tables), nothing in user space. NULL if
 * no page is free. Must be called after kvminit(), and the kernel must not
 * add root entries afterwards. */
pagetable_t uvm_create(void);

/* Frees a table from uvm_create(): every page mapped in user space, the
 * user-space tables and the root. The kernel's tables stay. */
void uvm_free(pagetable_t pt);

/* Whether every byte of [va, va + len) is user memory in `pt` mapped with
 * U and all of `perm` (PTE_R, PTE_W). len 0 is always fine. */
bool uvm_range_ok(pagetable_t pt, uintptr va, usize len, u32 perm);

/* Copies `len` bytes from user address `va` in `pt` to `dst`, through the
 * kernel's mapping of the physical pages (the direct map): the kernel never
 * dereferences a user address, so sstatus.SUM stays clear. All or nothing:
 * returns -1, having copied nothing, unless uvm_range_ok(pt, va, len,
 * PTE_R); else 0. */
int copyin(pagetable_t pt, void *dst, uintptr va, usize len);

/* Copies `len` bytes from `src` to user address `va` in `pt`, through the
 * physical pages, like copyin(). All or nothing: returns -1, having written
 * nothing, unless uvm_range_ok(pt, va, len, PTE_W); else 0. */
int copyout(pagetable_t pt, uintptr va, const void *src, usize len);

/* Copies the NUL-terminated string at user address `va` in `pt` to `dst`,
 * which holds `max` bytes (at least 1), NUL included. Returns its length; -1
 * if a byte up to its NUL is not readable user memory; -2 if it has no NUL in
 * its first `max` bytes. `dst` holds a string (maybe cut short) either way. */
int copyinstr(pagetable_t pt, char *dst, uintptr va, u32 max);

#endif
