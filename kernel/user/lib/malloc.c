/* malloc and free on top of sbrk, after K&R's allocator (The C Programming
 * Language, section 8.7).
 *
 * The heap is a run of blocks. Each starts with a header: its size, in
 * header-sized units counting the header itself, and, while it is free, a link to
 * the next free block. The free blocks form a list in address order, so a
 * block being freed can be merged with a free neighbour on either side.
 * malloc takes the first free block that is big enough (first fit), cutting
 * what it needs off its end; when none is, it asks sbrk for more, at least
 * a page at a time, and frees that into the list. Memory never goes back to
 * the kernel.
 *
 * The threads of a process share the heap, so malloc and free hold a mutex
 * (mutex.c) while they walk and change the list. It costs one atomic
 * instruction each way when there is only one thread. */
#include "../user.h"

/* A pointer and a size: 8 bytes on RV32, 16 on RV64 (the pointer's 8 and
 * the size's 4, padded to the pointer's alignment), which is also the
 * alignment every block gets: on RV64 the ABI's largest, 16, as glibc's
 * malloc gives. */
struct header {
    struct header *next; /* the next free block, while this one is free */
    uint units;          /* the block's size, in headers, its own included */
};

#define UNIT ((uint)sizeof(struct header))
/* The least sbrk is asked for: a page. */
#define GROW_UNITS (4096 / UNIT)

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
    mutex_lock(&heap_lock);
    free_locked(p);
    mutex_unlock(&heap_lock);
}

/* Grows the heap by at least `units` and puts the new block on the free
 * list. Returns the list, or NULL when sbrk fails. */
static struct header *more(uint units) {
    if (units < GROW_UNITS) {
        units = GROW_UNITS;
    }
    if (units > 0x7fffffffu / UNIT) {
        return NULL;
    }
    char *got = sbrk((long)(units * UNIT));
    if ((long)got < 0) {
        return NULL;
    }
    struct header *b = (struct header *)got;
    b->units = units;
    free_locked(b + 1);
    return freep;
}

void *malloc(size_t n) {
    if (n == 0 || n > 0x7fffffffu - UNIT) {
        return NULL;
    }
    /* The header, and n bytes rounded up to whole units. */
    uint units = (uint)((n + UNIT - 1) / UNIT) + 1;
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
    if (p != NULL) {
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
