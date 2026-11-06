/* Shared-memory objects (spec section 10, "Kernel memory mappings";
 * docs/camelot-syscalls.md, "Memory mappings"): what memfd_create() makes,
 * Linux's anonymous shmem files. An object is a size (ftruncate) and the
 * pages holding its bytes, which it owns: every process that maps it
 * (MAP_SHARED, mmap.h) maps the same pages, so each sees the others'
 * writes, on any hart. This is how a GUI client and the display server
 * share a window's pixels.
 *
 * Its pages are made on demand, zeroed, the first time a mapping touches
 * them, so a big object costs memory only for what is used. They are kept
 * in a two-level table: a page of SHM_FANOUT pointers to pages of
 * SHM_FANOUT physical addresses, one per page of the object, like a page
 * table (or Linux's xarray), which bounds an object at SHM_MAX_SIZE.
 *
 * An object lives while anything holds a reference to it: each open file
 * (FD_SHM, file.h) and each area mapping it (mmap.h). The last one frees
 * it with its pages.
 *
 * Locking: shm_lock (shm.c) guards every object: its references, its size,
 * its mappings and its page table. Taken after a process's vm_lock (a
 * fault maps an object's page under both) and before kmem's. */
#ifndef CAMELOT_SHM_H
#define CAMELOT_SHM_H

#include "memlayout.h"
#include "types.h"

/* Objects, every process's together. memfd_create() fails with -ENFILE
 * when none is free. */
#define NSHM 32

/* memfd_create's flags (Linux's values): the descriptor closes on exec;
 * sealing allowed, which is accepted, though Camelot has no seals. And the
 * longest name it takes, without the NUL (Linux's MFD_NAME_MAX_LEN). */
#define MFD_CLOEXEC 0x1
#define MFD_ALLOW_SEALING 0x2
#define MFD_NAME_MAX 249

/* Entries in each page of an object's table: a page of 8-byte entries. */
#define SHM_FANOUT 512
/* The largest object: SHM_FANOUT * SHM_FANOUT pages, 1 GiB. */
#define SHM_MAX_SIZE ((u64)SHM_FANOUT * SHM_FANOUT * PGSIZE)

struct shm_object {
    /* References: open files and areas. 0: a free slot. */
    u32 ref;
    /* Of those, the areas: while it has one, a shrink keeps the pages past
     * the new end (shm_truncate()). */
    u32 maps;
    /* The size in bytes, which ftruncate() sets: a mapping's pages past it
     * can't be touched. */
    u64 size;
    /* The table of pages: NULL until the first page is made; else a page
     * of SHM_FANOUT physical addresses of the second-level pages, 0 for
     * none, each holding SHM_FANOUT physical addresses of the object's
     * pages, 0 for one not made yet. */
    phys_addr_t *dir;
};

/* Sets up shm_lock. kmain calls it at boot. */
void shm_init(void);

/* A new object, empty (size 0), with one reference, the caller's; NULL if
 * every slot is taken. */
struct shm_object *shm_create(void);

/* One more reference to `o`, an area's: shm_map() counts it among the
 * mappings too. */
void shm_get(struct shm_object *o);
void shm_map(struct shm_object *o);

/* Drops a reference, an area's for shm_unmap(). The last one frees the
 * object and its pages. */
void shm_put(struct shm_object *o);
void shm_unmap(struct shm_object *o);

/* ftruncate(): sets the size to `size` bytes. A shrink zeroes the rest of
 * the last page and frees the pages wholly past the new end, unless the
 * object is mapped: those stay until its last mapping goes, mapped where
 * they were touched already, and read zero again if it grows back over
 * them. 0, or -EFBIG past SHM_MAX_SIZE. */
int shm_truncate(struct shm_object *o, u64 size);

/* The size, for fstat(). */
u64 shm_size(struct shm_object *o);

/* The physical address of page `index` of `o`, made (zeroed) if it wasn't
 * yet: what a fault maps. 0, -EFAULT if the page is past the object's size
 * (Linux's SIGBUS), or -ENOMEM if no page could be had. Takes only
 * shm_lock and kmem's. */
long shm_page(struct shm_object *o, u64 index, phys_addr_t *pa);

#endif
