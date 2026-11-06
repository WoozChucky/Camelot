/* Shared-memory objects: memfd_create()'s memory (shm.h), Linux's shmem
 * files without the filesystem around them. */
#include "shm.h"
#include "kalloc.h"
#include "spinlock.h"
#include "string.h"
#include "syscall.h"
#include "vm.h"

static struct spinlock shm_lock;
static struct shm_object objects[NSHM];

void shm_init(void) {
    initlock(&shm_lock, "shm");
}

/* The pages `size` bytes take, the last one maybe in part. */
static u64 pages_for(u64 size) {
    return (size + PGSIZE - 1) / PGSIZE;
}

struct shm_object *shm_create(bool sealing) {
    acquire(&shm_lock);
    for (u32 i = 0; i < NSHM; i++) {
        struct shm_object *o = &objects[i];
        if (o->ref == 0) {
            memset(o, 0, sizeof *o);
            o->ref = 1;
            o->seals = sealing ? 0 : F_SEAL_SEAL;
            release(&shm_lock);
            return o;
        }
    }
    release(&shm_lock);
    return NULL;
}

void shm_get(struct shm_object *o) {
    acquire(&shm_lock);
    o->ref++;
    release(&shm_lock);
}

void shm_map(struct shm_object *o) {
    acquire(&shm_lock);
    o->ref++;
    o->maps++;
    release(&shm_lock);
}

/* The entry of page `index` in `o`'s table: where its physical address is
 * kept (0 for a page not made yet). With `alloc`, the table's pages on the
 * way are made if they are missing; NULL if they are missing and `alloc`
 * is false, or no page could be had. Called with shm_lock held. */
static phys_addr_t *page_entry(struct shm_object *o, u64 index, bool alloc) {
    if (o->dir == NULL) {
        if (!alloc) {
            return NULL;
        }
        o->dir = kzalloc();
        if (o->dir == NULL) {
            return NULL;
        }
    }
    phys_addr_t *leaf = &o->dir[index / SHM_FANOUT];
    if (*leaf == 0) {
        if (!alloc) {
            return NULL;
        }
        void *page = kzalloc();
        if (page == NULL) {
            return NULL;
        }
        *leaf = pa(page);
    }
    return (phys_addr_t *)va(*leaf) + index % SHM_FANOUT;
}

/* Frees `o`'s pages from page `first` on, and every page of its table that
 * no longer holds one; from 0, the table too. Called with shm_lock held. */
static void free_pages_from(struct shm_object *o, u64 first) {
    if (o->dir == NULL) {
        return;
    }
    for (u32 i = 0; i < SHM_FANOUT; i++) {
        if (o->dir[i] == 0) {
            continue;
        }
        u64 base = (u64)i * SHM_FANOUT;
        if (base + SHM_FANOUT <= first) {
            continue; /* all of it kept */
        }
        phys_addr_t *leaf = va(o->dir[i]);
        bool empty = true;
        for (u32 j = 0; j < SHM_FANOUT; j++) {
            if (leaf[j] == 0) {
                continue;
            }
            if (base + j >= first) {
                kfree(va(leaf[j]));
                leaf[j] = 0;
            } else {
                empty = false;
            }
        }
        if (empty) {
            kfree(leaf);
            o->dir[i] = 0;
        }
    }
    if (first == 0) {
        kfree(o->dir);
        o->dir = NULL;
    }
}

void shm_put(struct shm_object *o) {
    acquire(&shm_lock);
    o->ref--;
    if (o->ref == 0) {
        free_pages_from(o, 0);
    }
    release(&shm_lock);
}

void shm_unmap(struct shm_object *o) {
    acquire(&shm_lock);
    o->ref--;
    o->maps--;
    if (o->ref == 0) {
        free_pages_from(o, 0);
    } else if (o->maps == 0) {
        /* The pages a shrink kept for the mappings go with the last. */
        free_pages_from(o, pages_for(o->size));
    }
    release(&shm_lock);
}

/* Zeroes the bytes of `o` from `from` up to the end of the page holding
 * `to` - 1, in the pages it has: what was past the end before a shrink, or
 * what the end's last page held past it, reads zero again. Called with
 * shm_lock held. */
static void zero_range(struct shm_object *o, u64 from, u64 to) {
    u64 index = from / PGSIZE;
    u64 last = pages_for(to);
    while (index < last) {
        if (o->dir == NULL || o->dir[index / SHM_FANOUT] == 0) {
            /* No page in this part of the table: on to the next part. */
            index = (index / SHM_FANOUT + 1) * SHM_FANOUT;
            continue;
        }
        phys_addr_t *entry = page_entry(o, index, false);
        if (*entry != 0) {
            u64 skip = index == from / PGSIZE ? from % PGSIZE : 0;
            memset((u8 *)va(*entry) + skip, 0, PGSIZE - skip);
        }
        index++;
    }
}

int shm_truncate(struct shm_object *o, u64 size) {
    if (size > SHM_MAX_SIZE) {
        return -EFBIG;
    }
    acquire(&shm_lock);
    u64 old = o->size;
    if ((size < old && (o->seals & F_SEAL_SHRINK)) || (size > old && (o->seals & F_SEAL_GROW))) {
        release(&shm_lock);
        return -EPERM;
    }
    if (size < old) {
        /* The rest of the new last page reads zero, as Linux's truncate
         * zeroes it; the pages past it go, unless a mapping may hold
         * them. */
        if (size % PGSIZE != 0) {
            zero_range(o, size, size + 1);
        }
        if (o->maps == 0) {
            free_pages_from(o, pages_for(size));
        }
    } else if (size > old) {
        /* Pages a shrink kept for a mapping, and the old last page's tail,
         * may hold what was written past the end: the new bytes read
         * zero. */
        zero_range(o, old, size);
    }
    o->size = size;
    release(&shm_lock);
    return 0;
}

int shm_add_seals(struct shm_object *o, u32 seals) {
    acquire(&shm_lock);
    int r = 0;
    if (o->seals & F_SEAL_SEAL) {
        r = -EPERM;
    } else {
        o->seals |= seals;
    }
    release(&shm_lock);
    return r;
}

u32 shm_seals(struct shm_object *o) {
    acquire(&shm_lock);
    u32 seals = o->seals;
    release(&shm_lock);
    return seals;
}

u64 shm_size(struct shm_object *o) {
    acquire(&shm_lock);
    u64 size = o->size;
    release(&shm_lock);
    return size;
}

bool shm_page(struct shm_object *o, u64 index, phys_addr_t *out) {
    acquire(&shm_lock);
    if (index >= pages_for(o->size)) {
        release(&shm_lock);
        return false;
    }
    phys_addr_t *entry = page_entry(o, index, true);
    if (entry != NULL && *entry == 0) {
        void *page = kzalloc();
        if (page != NULL) {
            *entry = pa(page);
        }
    }
    bool ok = entry != NULL && *entry != 0;
    if (ok) {
        *out = *entry;
    }
    release(&shm_lock);
    return ok;
}
