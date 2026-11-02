/* The buffer cache (spec section 10, "Kernel disks"), after xv6's bio.c: a
 * fixed pool of block buffers between the disks and their users (the
 * filesystem, later). A block is BSIZE bytes, two sectors; block n of a disk
 * is its sectors 2n and 2n + 1.
 *
 *   struct buf *b = bread(dev, blockno);   locked, holding the block
 *   if (b->error == BLK_OK) { ... read or change b->data, bwrite(b) ... }
 *   brelse(b);
 *
 * A buffer is locked by whoever bread() returned it to, until brelse(): a
 * process asking for a block that is locked sleeps until it is released, so
 * there is only ever one copy of a block and one user changing it. Released
 * buffers stay cached, and when a block that isn't cached needs a buffer,
 * the least recently released one is reused. Writes go to the disk at once
 * (bwrite). The filesystem's log changes buffers without writing them until
 * its transaction commits (log.h): it pins them (bpin) so they stay cached
 * until then, and only then does the cache hold data the disk doesn't. */
#ifndef CAMELOT_BIO_H
#define CAMELOT_BIO_H

#include "blk.h"
#include "types.h"

#define BSIZE 1024
#define BLOCK_SECTORS (BSIZE / BLK_SECTOR_SIZE)

/* Buffers in the cache, as in xv6. bread() panics if every one is in use. */
#define NBUF 30

struct buf {
    /* `data` holds the block as it is on the disk (or as a transaction
     * changed it, while the buffer is pinned). */
    bool valid;
    /* Held by the process bread() returned it to, until brelse(). */
    bool locked;
    /* After bread(): BLK_OK, or the BLK_ERR_ code of the failed read (the
     * data is then not valid). */
    u32 error;
    u32 dev;
    u32 blockno;
    /* Users: the holder, the processes waiting for the lock, and one more
     * while the log has it pinned. A buffer with users is never reused for
     * another block. */
    u32 refcnt;
    /* The LRU list, most recently released first. */
    struct buf *prev;
    struct buf *next;
    u8 data[BSIZE];
};

/* bread() calls served from the cache, and those that read the disk. */
extern u32 bio_hits;
extern u32 bio_misses;

/* Puts every buffer on the LRU list, empty. Call once at boot. */
void binit(void);

/* Returns the buffer for block `blockno` of disk `dev`, locked, reading the
 * block from the disk unless it is cached. If the read fails, b->error says
 * why and the data is not valid; the buffer must still be released. Must be
 * called by a process (it may sleep). */
struct buf *bread(u32 dev, u32 blockno);

/* Writes b->data to its block on the disk. `b` must be locked (from bread).
 * Returns BLK_OK or the BLK_ERR_ code; after a failed write the cached copy
 * is dropped, so the next bread() reads what the disk holds. */
u32 bwrite(struct buf *b);

/* Unlocks `b`, which must be locked, and gives it back to the cache. */
void brelse(struct buf *b);

/* Pins `b` in the cache (an extra user, so it is never reused for another
 * block), and unpins it. For the log, which holds a transaction's changed
 * blocks in the cache until they are written. `b` must be locked. */
void bpin(struct buf *b);
void bunpin(struct buf *b);

#endif
