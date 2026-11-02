/* The buffer cache (spec section 10, "Kernel disks"), after xv6's bio.c.
 *
 * The NBUF buffers live in .bss, on one doubly linked list ordered by when
 * they were last released, most recent first. A lookup scans from the front
 * for a cached copy; a miss takes the last buffer nobody uses. A buffer
 * released without valid data (its read or write failed) holds nothing worth
 * keeping, so it goes to the back instead, to be reused first. Each buffer
 * has a sleep lock: `locked` plus sleep()/wakeup() on the buffer, held from
 * bread() to brelse(), and kept across the sleep in the disk driver. As in
 * proc.c there are no spinlocks: a process's kernel code is never preempted
 * and runs with interrupts off, and interrupt handlers don't touch the
 * cache. */
#include "bio.h"
#include "proc.h"
#include "trap.h"

static struct buf bufs[NBUF];

/* The list's head: head.next is the most recently released buffer,
 * head.prev the least. Its own data is unused. */
static struct buf head;

u32 bio_hits;
u32 bio_misses;

/* Blocks past this would start at a sector beyond 2^32. */
#define MAX_BLOCKS (0xffffffffu / BLOCK_SECTORS + 1)

/* Takes `b` off the list. */
static void unlink(struct buf *b) {
    b->prev->next = b->next;
    b->next->prev = b->prev;
}

/* Puts `b` at the front of the list. */
static void push_front(struct buf *b) {
    b->next = head.next;
    b->prev = &head;
    head.next->prev = b;
    head.next = b;
}

/* Puts `b` at the back of the list. */
static void push_back(struct buf *b) {
    b->prev = head.prev;
    b->next = &head;
    head.prev->next = b;
    head.prev = b;
}

void binit(void) {
    head.prev = &head;
    head.next = &head;
    for (u32 i = 0; i < NBUF; i++) {
        bufs[i].valid = false;
        bufs[i].locked = false;
        bufs[i].refcnt = 0;
        push_front(&bufs[i]);
    }
}

static void lock(struct buf *b) {
    while (b->locked) {
        sleep(b);
    }
    b->locked = true;
}

/* The buffer for block `blockno` of disk `dev`, locked: the cached one if
 * there is one, else the least recently released buffer nobody uses, taken
 * over for this block (its data not valid). */
static struct buf *bget(u32 dev, u32 blockno) {
    /* A buffer with users holds its block even before its data is valid (the
     * read is under way); one with neither holds nothing. */
    for (struct buf *b = head.next; b != &head; b = b->next) {
        if (b->dev == dev && b->blockno == blockno && (b->valid || b->refcnt > 0)) {
            b->refcnt++;
            lock(b);
            return b;
        }
    }
    for (struct buf *b = head.prev; b != &head; b = b->prev) {
        if (b->refcnt == 0) {
            b->dev = dev;
            b->blockno = blockno;
            b->valid = false;
            b->refcnt = 1;
            /* Unused, so unlocked: this doesn't sleep. */
            lock(b);
            return b;
        }
    }
    panic("bget: no buffers");
    return NULL;
}

struct buf *bread(u32 dev, u32 blockno) {
    struct buf *b = bget(dev, blockno);
    if (b->valid) {
        bio_hits++;
        b->error = BLK_OK;
        return b;
    }
    bio_misses++;
    if (blockno >= MAX_BLOCKS) {
        b->error = BLK_ERR_OUT_OF_RANGE;
    } else {
        b->error = blk_read(dev, blockno * BLOCK_SECTORS, b->data, BLOCK_SECTORS);
    }
    b->valid = b->error == BLK_OK;
    return b;
}

u32 bwrite(struct buf *b) {
    if (!b->locked) {
        panic("bwrite: buffer not locked");
    }
    u32 err = BLK_ERR_OUT_OF_RANGE;
    if (b->blockno < MAX_BLOCKS) {
        err = blk_write(b->dev, b->blockno * BLOCK_SECTORS, b->data, BLOCK_SECTORS);
    }
    /* The disk now holds the data, or (the write failed) maybe not: then the
     * next bread() must read what it does hold. */
    b->valid = err == BLK_OK;
    return err;
}

void brelse(struct buf *b) {
    if (!b->locked) {
        panic("brelse: buffer not locked");
    }
    b->locked = false;
    wakeup(b);
    b->refcnt--;
    if (b->refcnt == 0) {
        unlink(b);
        /* Just used, so it goes to the front; unless it holds nothing, which
         * the next miss should take before evicting a cached block. */
        if (b->valid) {
            push_front(b);
        } else {
            push_back(b);
        }
    }
}

void bpin(struct buf *b) {
    if (!b->locked) {
        panic("bpin: buffer not locked");
    }
    b->refcnt++;
}

void bunpin(struct buf *b) {
    if (!b->locked || b->refcnt < 2) {
        panic("bunpin: buffer not pinned");
    }
    b->refcnt--;
}
