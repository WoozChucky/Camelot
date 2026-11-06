/* Memory mappings: mmap, munmap, mprotect and their page faults (mmap.h),
 * after Linux's mm/mmap.c, mm/mprotect.c and mm/memory.c.
 *
 * How a mapping comes and goes:
 *
 *   mmap      checks its arguments and what backs it (file.c's filemmap()
 *             for a descriptor), finds room for it (from the top of the
 *             mmap region down, or where MAP_FIXED says), and puts a new
 *             area in the process's list. No page is mapped yet (unless
 *             MAP_POPULATE): the first touch faults, and mm_fault() maps
 *             the page then, from the area's backing.
 *   munmap    splits the areas at the range's ends, so that each one is
 *             wholly in or out of it, takes those in out of the list and
 *             their pages out of the page table, shoots the page table
 *             down on every hart running the process (tlb_shootdown(), as
 *             sbrk does), and only then frees the anonymous pages and lets
 *             go of the objects: no hart can still reach them.
 *   mprotect  splits the same way, gives the areas in the range their new
 *             protections and their mapped pages new permissions, and
 *             shoots the page table down, since a permission may have gone.
 *
 * Each step that changes the list holds the process's heap_lock (a sleep
 * lock: one such call at a time, sbrk included) and, for the change
 * itself, its vm_lock (a spinlock: what faults take). So the list can be
 * read under either one. */
#include "mmap.h"
#include "file.h"
#include "kalloc.h"
#include "proc.h"
#include "shm.h"
#include "smp.h"
#include "spinlock.h"
#include "string.h"
#include "syscall.h"
#include "vm.h"

/* The areas no process has, a list through `next` (Linux takes them from
 * a slab cache, vm_area_cachep). */
static struct spinlock area_lock;
static struct vm_area area_table[NAREA];
static struct vm_area *free_areas;

void mmap_init(void) {
    initlock(&area_lock, "area");
    for (u32 i = 0; i < NAREA; i++) {
        area_table[i].next = free_areas;
        free_areas = &area_table[i];
    }
}

/* A free area, zeroed; NULL if none is left. */
static struct vm_area *area_alloc(void) {
    acquire(&area_lock);
    struct vm_area *a = free_areas;
    if (a != NULL) {
        free_areas = a->next;
    }
    release(&area_lock);
    if (a != NULL) {
        memset(a, 0, sizeof *a);
    }
    return a;
}

static void area_free(struct vm_area *a) {
    acquire(&area_lock);
    a->next = free_areas;
    free_areas = a;
    release(&area_lock);
}

/* The PTE permissions for protections `prot`: R for reading (and for
 * writing, which needs it), W, X; 0 for PROT_NONE. */
static u32 prot_perm(u32 prot) {
    u32 perm = 0;
    if (prot & (PROT_READ | PROT_WRITE)) {
        perm |= PTE_R;
    }
    if (prot & PROT_WRITE) {
        perm |= PTE_W;
    }
    if (prot & PROT_EXEC) {
        perm |= PTE_X;
    }
    return perm;
}

/* The PTE of an area's page at `pa` with protections `prot`: a user leaf
 * (with A, and D if writable, as map_pages() makes them), or for
 * PROT_NONE the page kept without V (PTE_PROT_NONE, mmap.h). */
static pte_t area_pte(phys_addr_t pa, u32 prot) {
    u32 perm = prot_perm(prot);
    if (perm == 0) {
        return PA2PTE(pa) | PTE_PROT_NONE;
    }
    return PA2PTE(pa) | perm | PTE_U | PTE_USER_AD(perm) | PTE_V;
}

/* The area of `p` holding user address `va`, or NULL. */
static struct vm_area *area_find(struct proc *p, uintptr va) {
    for (struct vm_area *a = p->areas; a != NULL && a->start <= va; a = a->next) {
        if (va < a->end) {
            return a;
        }
    }
    return NULL;
}

bool mm_overlaps(struct proc *p, uintptr start, uintptr end) {
    for (struct vm_area *a = p->areas; a != NULL && a->start < end; a = a->next) {
        if (a->end > start) {
            return true;
        }
    }
    return false;
}

/* Calls `fn` on the PTE of every page in [start, end) of `pt` that has
 * one (a leaf, or a page kept for PROT_NONE): the pages an area has been
 * given so far. A missing table skips the 2 MiB it would map, so a big
 * area that was hardly touched is quick to go over. */
static void each_pte(pagetable_t pt, uintptr start, uintptr end, void (*fn)(pte_t *pte, void *arg),
                     void *arg) {
    uintptr a = start;
    while (a < end) {
        pte_t *pte = walk(pt, a, false);
        if (pte == NULL) {
            a = (a + LEVEL_SIZE(1)) & ~(LEVEL_SIZE(1) - 1);
            continue;
        }
        if (*pte != 0) {
            fn(pte, arg);
        }
        a += PGSIZE;
    }
}

/* The physical page an area maps at `page`, one of its own: a new zeroed
 * page for anonymous memory, the object's page (made if need be) for a
 * shared-memory object, the device's page for a device. False if there is
 * none: no free page, or past the object's end. */
static bool area_page(struct vm_area *a, uintptr page, phys_addr_t *out) {
    if (a->kind == AREA_ANON) {
        void *mem = kzalloc();
        if (mem == NULL) {
            return false;
        }
        *out = pa(mem);
        return true;
    }
    if (a->kind == AREA_SHM) {
        return shm_page(a->shm, a->pgoff + (page - a->start) / PGSIZE, out);
    }
    *out = a->device + (page - a->start);
    return true;
}

bool mm_fault(pagetable_t pt, uintptr va, u32 access) {
    struct proc *p = myproc();
    if (p == NULL || p->pagetable != pt || va >= USER_TOP) {
        return false;
    }
    uintptr page = PGROUNDDOWN(va);
    bool ok = false;
    acquire(&p->vm_lock);
    struct vm_area *a = area_find(p, va);
    u32 perm = a != NULL ? prot_perm(a->prot) : 0;
    if (perm != 0 && (perm & access) == access) {
        pte_t *pte = walk(pt, page, false);
        if (pte != NULL && (*pte & PTE_V) != 0) {
            /* Another thread mapped it first: fine if the access is
             * allowed, as it is unless the protections changed since. */
            ok = (*pte & access) == access;
        } else if (pte == NULL || *pte == 0) {
            phys_addr_t pa;
            if (area_page(a, page, &pa)) {
                if (map_pages(pt, page, pa, PGSIZE, perm | PTE_U) == 0) {
                    ok = true;
                } else if (a->kind == AREA_ANON) {
                    kfree(va(pa));
                }
            }
        }
    }
    release(&p->vm_lock);
    return ok;
}

/* Splits `a` in two at `at`, strictly inside it, with `b`, a free area:
 * `a` keeps [start, at), `b` takes [at, end) and comes after it in the
 * list, with the same backing from there on (a shared-memory object then
 * has one more mapping, b's). Called holding the process's vm_lock (or on
 * a list no fault can see yet). */
static void area_split(struct vm_area *a, uintptr at, struct vm_area *b) {
    *b = *a;
    b->start = at;
    b->pgoff = a->pgoff + (at - a->start) / PGSIZE;
    b->device = a->device + (at - a->start);
    a->end = at;
    a->next = b;
    if (b->kind == AREA_SHM) {
        shm_map(b->shm);
    }
}

/* How many splits [start, end) needs among `p`'s areas: one for an area
 * that starts before `start` and goes on past it, one for an area that
 * starts before `end` and goes on past it (the same area twice, if the
 * range is inside it). */
static u32 splits_needed(struct proc *p, uintptr start, uintptr end) {
    u32 n = 0;
    for (struct vm_area *a = p->areas; a != NULL && a->start < end; a = a->next) {
        if (a->start < start && a->end > start) {
            n++;
        }
        if (a->start < end && a->end > end) {
            n++;
        }
    }
    return n;
}

/* Takes `n` free areas for the splits to come, in spare[0..n), all or
 * none: false if there aren't that many. */
static bool take_spares(struct vm_area **spare, u32 n) {
    for (u32 i = 0; i < n; i++) {
        spare[i] = area_alloc();
        if (spare[i] == NULL) {
            for (u32 j = 0; j < i; j++) {
                area_free(spare[j]);
            }
            return false;
        }
    }
    return true;
}

/* Splits `p`'s areas at `start` and `end` with the spares
 * (splits_needed() of them), so that every area is wholly inside [start,
 * end) or wholly outside. Called holding p->vm_lock. */
static void split_at_ends(struct proc *p, uintptr start, uintptr end, struct vm_area **spare) {
    u32 used = 0;
    for (struct vm_area *a = p->areas; a != NULL && a->start < end; a = a->next) {
        if (a->start < start && a->end > start) {
            area_split(a, start, spare[used++]);
            continue; /* the next one is the part from `start` */
        }
        if (a->start < end && a->end > end) {
            area_split(a, end, spare[used++]);
        }
    }
}

/* For each_pte(): a mapped page made unreachable, its address kept for
 * release_pte() once no hart can be using it. */
static void invalidate_pte(pte_t *pte, void *arg) {
    (void)arg;
    *pte &= ~(pte_t)PTE_V;
}

/* For each_pte(): an area's page given back, an anonymous one freed (arg
 * is the area), and its PTE cleared. */
static void release_pte(pte_t *pte, void *arg) {
    struct vm_area *a = arg;
    if (a->kind == AREA_ANON) {
        kfree(va(PTE2PA(*pte)));
    }
    *pte = 0;
}

/* Frees the areas of list `gone`, which no fault can find any more, with
 * the pages they had in `pt`: freed if anonymous, a shared-memory object's
 * reference dropped, a device's pages left alone. */
static void release_areas(pagetable_t pt, struct vm_area *gone) {
    while (gone != NULL) {
        struct vm_area *a = gone;
        gone = a->next;
        each_pte(pt, a->start, a->end, release_pte, a);
        if (a->kind == AREA_SHM) {
            shm_unmap(a->shm);
        }
        area_free(a);
    }
}

void mm_release(pagetable_t pt, struct vm_area *areas) {
    release_areas(pt, areas);
}

/* Unmaps [start, end) (page aligned) from the current process: every area
 * or part of one there goes, with its pages, after a shootdown. 0, or
 * -ENOMEM (nothing changed) if an area has to be split and no free one is
 * left for it. Called holding p->heap_lock. */
static int unmap_range(struct proc *p, uintptr start, uintptr end) {
    if (!mm_overlaps(p, start, end)) {
        return 0;
    }
    struct vm_area *spare[2];
    u32 n = splits_needed(p, start, end);
    if (!take_spares(spare, n)) {
        return -ENOMEM;
    }
    acquire(&p->vm_lock);
    split_at_ends(p, start, end, spare);
    /* Out of the list, the areas in the range, kept in order on `gone`;
     * their pages unreachable, but still held. */
    struct vm_area *gone = NULL;
    struct vm_area **gone_end = &gone;
    struct vm_area **link = &p->areas;
    while (*link != NULL && (*link)->start < end) {
        struct vm_area *a = *link;
        if (a->start < start) {
            link = &a->next;
            continue;
        }
        *link = a->next;
        a->next = NULL;
        *gone_end = a;
        gone_end = &a->next;
        each_pte(p->pagetable, a->start, a->end, invalidate_pte, NULL);
    }
    release(&p->vm_lock);
    /* No hart may still translate to the pages, nor be copying through
     * them for a system call, before they are freed (unmap_pages()). */
    tlb_shootdown(p->pagetable);
    uvm_sync();
    release_areas(p->pagetable, gone);
    return 0;
}

/* Whether two neighbouring areas, `a` just below `b`, can be one: the same
 * protections and flags, and backing that goes on from one into the
 * other (Linux's vma_merge()). */
static bool can_merge(struct vm_area *a, struct vm_area *b) {
    if (a->end != b->start || a->kind != b->kind || a->prot != b->prot || a->flags != b->flags ||
        a->may_write != b->may_write) {
        return false;
    }
    if (a->kind == AREA_SHM) {
        return a->shm == b->shm && a->pgoff + (a->end - a->start) / PGSIZE == b->pgoff;
    }
    if (a->kind == AREA_DEVICE) {
        return a->device + (a->end - a->start) == b->device;
    }
    return true;
}

/* Merges every pair of neighbouring areas of `p` that can be one, from
 * `start` to `end`: after mmap and mprotect, so that a program that maps
 * piece by piece, or changes the protections back, doesn't use up the
 * table. Called holding p->vm_lock. */
static void merge_areas(struct proc *p, uintptr start, uintptr end) {
    struct vm_area *a = p->areas;
    while (a != NULL && a->next != NULL && a->start <= end) {
        struct vm_area *b = a->next;
        if (b->end >= start && can_merge(a, b)) {
            a->end = b->end;
            a->next = b->next;
            if (b->kind == AREA_SHM) {
                /* Not the object's last reference: `a` holds one too. */
                shm_unmap(b->shm);
            }
            area_free(b);
        } else {
            a = b;
        }
    }
}

/* Lets go of what a mapping that won't be made holds: the reference to
 * its shared-memory object. */
static void drop_backing(struct vm_area *want) {
    if (want->kind == AREA_SHM) {
        shm_unmap(want->shm);
    }
}

/* The highest free range of `size` bytes in [lo, hi) between `p`'s areas,
 * Linux's top-down search (unmapped_area_topdown()): its start, or 0 if
 * there is none. */
static uintptr find_room(struct proc *p, uintptr lo, uintptr hi, usize size) {
    uintptr found = 0;
    uintptr gap_start = lo;
    for (struct vm_area *a = p->areas;; a = a->next) {
        uintptr gap_end = a != NULL && a->start < hi ? a->start : hi;
        /* Gaps come in address order, so the last that fits is the
         * highest. */
        if (gap_end > gap_start && gap_end - gap_start >= size) {
            found = gap_end - size;
        }
        if (a == NULL || a->start >= hi) {
            return found;
        }
        if (a->end > gap_start) {
            gap_start = a->end;
        }
    }
}

long mm_mmap(uintptr addr, usize len, u32 prot, u32 flags, int fd, off_t offset) {
    struct proc *p = myproc();
    u32 type = flags & MAP_TYPE;
    if (len == 0 || (prot & ~(u32)(PROT_READ | PROT_WRITE | PROT_EXEC)) != 0 ||
        (type != MAP_SHARED && type != MAP_PRIVATE)) {
        return -EINVAL;
    }
    bool fixed = (flags & (MAP_FIXED | MAP_FIXED_NOREPLACE)) != 0;
    if (fixed && addr % PGSIZE != 0) {
        return -EINVAL;
    }
    /* More than user space holds never fits; past that, rounding up to
     * whole pages can't wrap. */
    if (len > USER_TOP) {
        return -ENOMEM;
    }
    usize size = PGROUNDUP(len);

    struct vm_area want;
    memset(&want, 0, sizeof want);
    want.prot = prot;
    want.flags = type;
    want.may_write = true;
    if (flags & MAP_ANONYMOUS) {
        /* The descriptor and the offset are ignored, as Linux ignores
         * them. MAP_SHARED anonymous memory would be shared with the
         * children fork() makes; Camelot has no fork (spawn starts a new
         * program), so it is the process's own either way. */
        want.kind = AREA_ANON;
        /* More than the whole of RAM can never be had: as sbrk(), fail
         * at once (Linux's overcommit guess). */
        if (size / PGSIZE > ram_size / PGSIZE) {
            return -ENOMEM;
        }
    } else {
        if (offset < 0 || offset % PGSIZE != 0) {
            return -EINVAL;
        }
        struct file *f = fd_get((u32)fd);
        if (f == NULL) {
            return -EBADF;
        }
        int r = filemmap(f, type, prot, (u64)offset, size, &want);
        fileclose(f);
        if (r != 0) {
            return r;
        }
    }

    /* From here on, `want` may hold a reference to a shared-memory object
     * (filemmap() took it while the descriptor kept the object alive),
     * which the new area takes over, or drop_backing() lets go of. */
    struct vm_area *a = area_alloc();
    if (a == NULL) {
        drop_backing(&want);
        return -ENOMEM;
    }
    acquiresleep(&p->heap_lock);
    /* The mmap region: from the heap's end, which can't grow into an area
     * (sbrk), to the guard page under the stack's limit. */
    uintptr lo = PGROUNDUP(p->brk);
    uintptr hi = USER_LOAD_TOP;
    uintptr start = 0;
    long err = 0;
    if (fixed) {
        if (addr < lo || addr > hi || size > hi - addr) {
            err = -ENOMEM;
        } else if (mm_overlaps(p, addr, addr + size)) {
            if (flags & MAP_FIXED_NOREPLACE) {
                err = -EEXIST;
            } else {
                err = unmap_range(p, addr, addr + size);
            }
        }
        start = addr;
    } else {
        /* A hint is taken if the range is free; else the highest room. */
        uintptr hint = PGROUNDUP(addr);
        if (hint >= lo && hint <= hi && size <= hi - hint && !mm_overlaps(p, hint, hint + size)) {
            start = hint;
        } else {
            start = find_room(p, lo, hi, size);
            if (start == 0) {
                err = -ENOMEM;
            }
        }
    }
    if (err != 0) {
        releasesleep(&p->heap_lock);
        area_free(a);
        drop_backing(&want);
        return err;
    }
    *a = want;
    a->start = start;
    a->end = start + size;
    acquire(&p->vm_lock);
    struct vm_area **link = &p->areas;
    while (*link != NULL && (*link)->start < a->start) {
        link = &(*link)->next;
    }
    a->next = *link;
    *link = a;
    merge_areas(p, start, start + size);
    release(&p->vm_lock);
    releasesleep(&p->heap_lock);
    if (flags & MAP_POPULATE) {
        /* Every page now, as a touch of each would map it; one that can't
         * be had is left to fault later. */
        for (uintptr page = start; page < start + size; page += PGSIZE) {
            mm_fault(p->pagetable, page, 0);
        }
    }
    return (long)start;
}

/* The range [addr, addr + len) as whole pages, for munmap and mprotect:
 * false unless `addr` is page aligned and the range is in user space. */
static bool user_pages(uintptr addr, usize len, uintptr *end) {
    if (addr % PGSIZE != 0 || addr >= USER_TOP || len > USER_TOP - addr) {
        return false;
    }
    *end = PGROUNDUP(addr + len);
    return true;
}

long mm_munmap(uintptr addr, usize len) {
    uintptr end;
    if (len == 0 || !user_pages(addr, len, &end)) {
        return -EINVAL;
    }
    struct proc *p = myproc();
    acquiresleep(&p->heap_lock);
    int r = unmap_range(p, addr, end);
    releasesleep(&p->heap_lock);
    return r;
}

/* For each_pte(): a page's PTE with the protections in *arg. */
static void protect_pte(pte_t *pte, void *arg) {
    *pte = area_pte(PTE2PA(*pte), *(u32 *)arg);
}

long mm_mprotect(uintptr addr, usize len, u32 prot) {
    if (addr % PGSIZE != 0 || (prot & ~(u32)(PROT_READ | PROT_WRITE | PROT_EXEC)) != 0) {
        return -EINVAL;
    }
    if (len == 0) {
        return 0;
    }
    uintptr end;
    if (!user_pages(addr, len, &end)) {
        return -ENOMEM;
    }
    struct proc *p = myproc();
    acquiresleep(&p->heap_lock);
    /* Every page of the range must be in an area, with no hole: else
     * -ENOMEM, as Linux says for memory that isn't mapped. */
    uintptr covered = addr;
    bool denied = false;
    for (struct vm_area *a = p->areas; a != NULL && a->start < end; a = a->next) {
        if (a->end <= covered) {
            continue;
        }
        if (a->start > covered) {
            break;
        }
        covered = a->end;
        if ((prot & PROT_WRITE) != 0 && !a->may_write) {
            denied = true;
        }
    }
    long r = 0;
    struct vm_area *spare[2];
    if (covered < end) {
        r = -ENOMEM;
    } else if (denied) {
        r = -EACCES;
    } else if (!take_spares(spare, splits_needed(p, addr, end))) {
        r = -ENOMEM;
    }
    if (r != 0) {
        releasesleep(&p->heap_lock);
        return r;
    }
    acquire(&p->vm_lock);
    split_at_ends(p, addr, end, spare);
    for (struct vm_area *a = p->areas; a != NULL && a->start < end; a = a->next) {
        if (a->start >= addr) {
            a->prot = prot;
            each_pte(p->pagetable, a->start, a->end, protect_pte, &prot);
        }
    }
    merge_areas(p, addr, end);
    release(&p->vm_lock);
    /* A permission may have gone: no hart may keep it in its TLB. */
    tlb_shootdown(p->pagetable);
    releasesleep(&p->heap_lock);
    return 0;
}
