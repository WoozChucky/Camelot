/* malloc and free on top of sbrk, after K&R's allocator (The C Programming
 * Language, section 8.7).
 *
 * The heap is a run of blocks. Each starts with a header: its size, in
 * header-sized units counting the header itself, and, while it is free, a link to
 * the next free block. The free blocks form a list in address order, so a
 * block being freed can be merged with a free neighbour on either side.
 * malloc takes the first free block that is big enough (first fit), cutting
 * what it needs off its end; when none is, it asks sbrk for more, at least
 * a page at a time, and frees that into the list. Heap memory never goes
 * back to the kernel.
 *
 * A block of MMAP_THRESHOLD bytes or more gets a mapping of its own instead
 * (mmap, anonymous), as glibc's malloc does, and free gives it back to the
 * kernel at once (munmap): a big buffer used for a while doesn't stay in the
 * heap for good. Its header says so (`next` is MAPPED); a block of the heap
 * has NULL there while it is handed out.
 *
 * The threads of a process share the heap, so malloc and free hold a mutex
 * (mutex.c) while they walk and change the list. It costs one atomic
 * instruction each way when there is only one thread. */
#include "../user.h"

/* A pointer and a size: 16 bytes, which is also the alignment every block
 * gets: the ABI's largest, as glibc's malloc gives. */
struct header {
    struct header *next; /* the next free block, while this one is free */
    size_t units;        /* the block's size, in headers, its own included */
};

#define UNIT sizeof(struct header)
/* The least sbrk is asked for: a page. */
#define GROW_UNITS (4096 / UNIT)
/* The size from which a block gets a mapping of its own: glibc's default
 * (M_MMAP_THRESHOLD, 128 KiB). */
#define MMAP_THRESHOLD (128 * 1024)
#define PAGE_SIZE 4096
/* `next` of a block in a mapping of its own, where no block of the heap
 * can be. */
#define MAPPED ((struct header *)1)

/* The free list: a ring through `base`, a block of size 0 that is never
 * handed out, so the list is never empty. `freep` is where the last search
 * stopped, the next one starts there. */
static struct header base;
static struct header *freep;
static struct mutex heap_lock;

/* free(), with heap_lock held. */
static void free_locked(void *p) {
    struct header *b = (struct header *)p - 1;
    /* The free block f with f < b < f->next, or at an end of the ring. */
    struct header *f = freep;
    while (!(b > f && b < f->next)) {
        if (f >= f->next && (b > f || b < f->next)) {
            break; /* past the highest block, or before the lowest */
        }
        f = f->next;
    }
    /* Merge with the block above, then with the one below. */
    if (b + b->units == f->next) {
        b->units += f->next->units;
        b->next = f->next->next;
    } else {
        b->next = f->next;
    }
    if (f + f->units == b) {
        f->units += b->units;
        f->next = b->next;
    } else {
        f->next = b;
    }
    freep = f;
}

void free(void *p) {
    if (p == NULL) {
        return;
    }
    struct header *b = (struct header *)p - 1;
    if (b->next == MAPPED) {
        munmap(b, b->units * UNIT);
        return;
    }
    mutex_lock(&heap_lock);
    free_locked(p);
    mutex_unlock(&heap_lock);
}

/* Grows the heap by at least `units` and puts the new block on the free
 * list. Returns the list, or NULL when sbrk fails. */
static struct header *more(size_t units) {
    if (units < GROW_UNITS) {
        units = GROW_UNITS;
    }
    /* sbrk takes a long: more than LONG_MAX bytes is more than there is. */
    if (units > (size_t)LONG_MAX / UNIT) {
        return NULL;
    }
    char *got = sbrk((long)(units * UNIT));
    if ((long)got < 0) {
        return NULL;
    }
    /* A program that calls sbrk itself may have left the end of the heap
     * anywhere. Then the block starts at the next multiple of UNIT, so it
     * stays aligned, and sbrk is asked for the bytes that skips, which
     * also leaves the end aligned for the next time, as glibc's malloc
     * corrects a misaligned break (sysmalloc's `correction`). If even
     * those few bytes can't be had, the block is a unit smaller. */
    uintptr_t skip = -(uintptr_t)got & (UNIT - 1);
    if (skip != 0) {
        got += skip;
        if ((long)sbrk((long)skip) < 0) {
            units--;
        }
    }
    struct header *b = (struct header *)got;
    b->units = units;
    free_locked(b + 1);
    return freep;
}

/* A block of `n` bytes in a mapping of its own, whole pages with its
 * header; NULL if the kernel has no room for it. */
static void *map_block(size_t n) {
    size_t bytes = (n + UNIT + PAGE_SIZE - 1) & ~(size_t)(PAGE_SIZE - 1);
    struct header *b = mmap(NULL, bytes, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if ((long)b < 0) {
        return NULL;
    }
    b->next = MAPPED;
    b->units = bytes / UNIT;
    return b + 1;
}

void *malloc(size_t n) {
    /* Nothing for 0 bytes; nor for more than a long holds (sbrk's
     * argument), which also keeps the rounding below from wrapping. */
    if (n == 0 || n > (size_t)LONG_MAX - UNIT) {
        return NULL;
    }
    /* A big block in a mapping; from the heap if the kernel has no room
     * for one, as glibc falls back. */
    if (n >= MMAP_THRESHOLD) {
        void *p = map_block(n);
        if (p != NULL) {
            return p;
        }
    }
    /* The header, and n bytes rounded up to whole units. */
    size_t units = (n + UNIT - 1) / UNIT + 1;
    mutex_lock(&heap_lock);
    if (freep == NULL) {
        base.next = &base;
        base.units = 0;
        freep = &base;
    }
    struct header *prev = freep;
    for (struct header *b = prev->next;; prev = b, b = b->next) {
        if (b->units >= units) {
            if (b->units == units) {
                prev->next = b->next;
            } else {
                /* Hand out the end of the block; the rest stays free. */
                b->units -= units;
                b += b->units;
                b->units = units;
            }
            freep = prev;
            mutex_unlock(&heap_lock);
            b->next = NULL;
            return b + 1;
        }
        if (b == freep) {
            /* Round the whole list: nothing big enough. */
            b = more(units);
            if (b == NULL) {
                mutex_unlock(&heap_lock);
                return NULL;
            }
        }
    }
}

void *calloc(size_t count, size_t size) {
    /* count * size must not wrap around (a size_t). */
    if (size != 0 && count > (size_t)-1 / size) {
        return NULL;
    }
    void *p = malloc(count * size);
    /* A block in a mapping of its own is new zeroed memory: writing zeros
     * would only make the kernel map every page of it now (glibc skips
     * them too). */
    if (p != NULL && ((struct header *)p - 1)->next != MAPPED) {
        memset(p, 0, count * size);
    }
    return p;
}

void *realloc(void *p, size_t n) {
    if (p == NULL) {
        return malloc(n);
    }
    if (n == 0) {
        free(p);
        return NULL;
    }
    size_t have = ((struct header *)p - 1)->units * UNIT - UNIT;
    if (n <= have) {
        return p;
    }
    void *q = malloc(n);
    if (q != NULL) {
        memcpy(q, p, have);
        free(p);
    }
    return q;
}
