/* Memory mappings (spec section 10, "Kernel memory mappings";
 * docs/camelot-syscalls.md, "Memory mappings"): mmap, munmap and mprotect,
 * as Linux's mm/mmap.c and mm/mprotect.c do them, for what Camelot needs.
 *
 * A process's mappings are its areas, Linux's VMAs: each a page-aligned
 * range of user addresses with its protections and what backs it. The
 * program's segments, the heap and the stack are not areas: they are the
 * address space proc.c builds and grows, and munmap and mprotect leave
 * them alone. The areas live between the heap's end and the stack's guard
 * page (USER_LOAD_TOP, vm.h), placed from the top down, as Linux's
 * mmap_base is below the stack.
 *
 * Three kinds of backing:
 *
 *   AREA_ANON    anonymous memory (MAP_ANONYMOUS): a zeroed page is mapped
 *                on the first touch (demand paging, like the stack), and
 *                freed with the mapping
 *   AREA_SHM     a shared-memory object (memfd_create, shm.h): its pages,
 *                which it owns, mapped on the first touch, so that every
 *                process mapping it sees the same memory
 *   AREA_DEVICE  a device's memory (a driver's mmap hook, file.h): physical
 *                pages that are not RAM's, mapped on the first touch like
 *                the others, and never freed
 *
 * A page of an area is in one of three states in the process's page table:
 * no PTE (never touched), a valid leaf with the area's permissions, or,
 * while the area is PROT_NONE, the page kept without V (PTE_PROT_NONE, its
 * address still in the PTE), as Linux's riscv _PAGE_PROT_NONE keeps a page
 * that mprotect() took every access from.
 *
 * Locking: the process's heap_lock (proc.h), a sleep lock, makes mmap,
 * munmap, mprotect, sbrk and the stack's growth one at a time: each may
 * change the page table and shoot it down. Its vm_lock, a spinlock, guards
 * the area list and every page the kernel maps for the process, so that a
 * fault (from U-mode, or a system call's copy into an area, uvm_range_ok())
 * can map a page with nothing but spinlocks: the copies run under the
 * pipes', the console's and the network's spinlocks. Lock order: vm_lock,
 * then shm_lock (shm.h), then kmem's. */
#ifndef CAMELOT_MMAP_H
#define CAMELOT_MMAP_H

#include "types.h"
#include "vm.h"

/* mmap's and mprotect's protections (Linux's values). PROT_WRITE implies
 * PROT_READ: a RISC-V page can't be writable and not readable. */
#define PROT_NONE 0x0
#define PROT_READ 0x1
#define PROT_WRITE 0x2
#define PROT_EXEC 0x4
/* Accepted by mprotect and meaning nothing (Linux's, for atomics, which
 * every RISC-V page allows). mmap ignores every other bit, as Linux's
 * does; mprotect refuses them. */
#define PROT_SEM 0x8

/* mmap's flags (Linux's values). One of MAP_SHARED, MAP_PRIVATE and
 * MAP_SHARED_VALIDATE, in MAP_TYPE's bits; flags Camelot doesn't know are
 * ignored, as Linux ignores them, but for MAP_SHARED_VALIDATE. */
#define MAP_SHARED 0x01
#define MAP_PRIVATE 0x02
#define MAP_SHARED_VALIDATE 0x03 /* MAP_SHARED, unknown flags -EOPNOTSUPP */
#define MAP_TYPE 0x0f
#define MAP_FIXED 0x10           /* exactly at addr, replacing what is there */
#define MAP_ANONYMOUS 0x20       /* no file: zeroed memory */
#define MAP_NORESERVE 0x4000     /* accepted; nothing is reserved anyway */
#define MAP_POPULATE 0x8000      /* map every page now, not on first touch */
#define MAP_FIXED_NOREPLACE 0x100000 /* exactly at addr, -EEXIST if taken */
/* The flags MAP_SHARED_VALIDATE takes for a descriptor: Linux's
 * LEGACY_MAP_MASK (the types; MAP_FIXED, MAP_ANONYMOUS, MAP_GROWSDOWN
 * 0x100, MAP_DENYWRITE 0x800, MAP_EXECUTABLE 0x1000, MAP_LOCKED 0x2000,
 * MAP_NORESERVE, MAP_POPULATE, MAP_NONBLOCK 0x10000, MAP_STACK 0x20000,
 * MAP_HUGETLB 0x40000, MAP_UNINITIALIZED 0x4000000) and
 * MAP_FIXED_NOREPLACE. The ones Camelot doesn't know it ignores here too. */
#define MAP_VALIDATE_FLAGS                                                                            \
    (MAP_SHARED | MAP_PRIVATE | MAP_FIXED | MAP_ANONYMOUS | 0x100 | 0x800 | 0x1000 | 0x2000 |         \
     MAP_NORESERVE | MAP_POPULATE | 0x10000 | 0x20000 | 0x40000 | 0x4000000 | MAP_FIXED_NOREPLACE)

/* The kernel's areas, every process's together (Linux bounds a process's
 * with vm.max_map_count): mmap() fails with -ENOMEM when none is free. */
#define NAREA 128

/* What backs an area. */
#define AREA_ANON 0
#define AREA_SHM 1
#define AREA_DEVICE 2

/* A PTE of an area's page while the area is PROT_NONE: the page's address
 * kept, V clear, and bit 8, one of the two bits RISC-V leaves to software
 * (RSW), set to say so. */
#define PTE_PROT_NONE (1u << 8)

struct shm_object;

/* One mapping: Linux's struct vm_area_struct. */
struct vm_area {
    uintptr start; /* page aligned */
    uintptr end;   /* one past the last byte, page aligned */
    u32 prot;      /* PROT_ */
    u32 flags;     /* MAP_SHARED or MAP_PRIVATE */
    u32 kind;      /* AREA_ */
    /* Whether mprotect() may add PROT_WRITE: false for a shared mapping of
     * a descriptor not open for writing (Linux's VM_MAYWRITE). */
    bool may_write;
    /* AREA_SHM: the object, with a reference the area holds, and the
     * object's page at `start` (Linux's vm_pgoff). */
    struct shm_object *shm;
    u64 pgoff;
    /* AREA_DEVICE: the physical address mapped at `start`. */
    phys_addr_t device;
    /* The next area up: a process's are a list in address order. */
    struct vm_area *next;
};

struct proc;

/* Sets up the area table's lock. kmain calls it at boot. */
void mmap_init(void);

/* mmap(addr, len, prot, flags, fd, offset) for the current process: the
 * address of the new mapping, or minus an error (docs/camelot-syscalls.md,
 * "Memory mappings", for each). */
long mm_mmap(uintptr addr, usize len, u32 prot, u32 flags, int fd, off_t offset);

/* munmap(addr, len): 0, or -EINVAL, -ENOMEM. */
long mm_munmap(uintptr addr, usize len);

/* mprotect(addr, len, prot): 0, or -EINVAL, -ENOMEM, -EACCES. */
long mm_mprotect(uintptr addr, usize len, u32 prot);

/* Maps the page holding user address `va` in `pt`, if it is a page of one
 * of the current process's areas not mapped yet whose protections allow
 * `access` (PTE_R, PTE_W or PTE_X): a page fault from U-mode (trap.c) and
 * a system call's check of a user range (uvm_range_ok(), vm.c), since the
 * kernel never faults on user memory, alike. True if the page is mapped
 * now (or was already, by another thread); false if the access is not
 * allowed there, or no page could be had. Takes only spinlocks, so it may
 * be called holding any spinlock but vm_lock's and kmem's. */
bool mm_fault(pagetable_t pt, uintptr va, u32 access);

/* Whether [start, end) overlaps one of `p`'s areas: sbrk() checks that the
 * heap doesn't grow into one. Called holding p->heap_lock. */
bool mm_overlaps(struct proc *p, uintptr start, uintptr end);

/* Takes every area of `areas`, a process's list, out of page table `pt`:
 * anonymous pages freed, shared-memory objects' references dropped,
 * device pages left alone, every PTE cleared, the areas freed. For a page
 * table that no hart uses any more (an ended process's, an old program's
 * after exec), before uvm_free(): which frees every page still mapped, and
 * so must not see the pages that aren't the process's own. Takes only
 * spinlocks. */
void mm_release(pagetable_t pt, struct vm_area *areas);

#endif
