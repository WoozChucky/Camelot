/* Self-test of the disk driver and the buffer cache (spec section 10,
 * "Kernel disks"), run at boot when a disk is attached. Disk requests sleep,
 * so the tests run in kernel processes, on disk 0:
 *
 * - three readers at once: a and c want block 0, b block 1. a's read is in
 *   flight when b asks for the disk, so b waits for it; c finds block 0
 *   locked by a, waits for a to release it and gets it from the cache. Two
 *   disk reads in all, and c sees what a saw.
 * - the scratch block, the disk's last whole block: read it (a miss), write
 *   a pattern, read it again (a hit), check the disk itself holds the
 *   pattern by reading past the cache, then write the old contents back and
 *   flush. On a read-only disk the write must fail with BLK_ERR_READ_ONLY
 *   and change nothing. On a disk holding a filesystem (disk 0 is the
 *   root), the last block may be one of its files', and the machine may
 *   stop between the two writes: there the "pattern" is the block's own
 *   contents, so the disk never holds anything else in it.
 * - errors: a block past the end is out of range and isn't cached; a buffer
 *   outside RAM is refused.
 * - LRU (when the disk is big enough): after blocks 0 to NBUF - 1, then
 *   block 0 again, block NBUF replaces block 1, the least recently used,
 *   and blocks 0 and NBUF are both cached. */
#include "blktest.h"
#include "bio.h"
#include "blk.h"
#include "fs.h"
#include "kprintf.h"
#include "proc.h"
#include "string.h"

/* The boot ROM: not RAM, so the device refuses it as a buffer. */
#define NOT_RAM 0x00001000

static int passed;
static int failed;

/* Whole blocks on disk 0. */
static u32 nblocks;

/* The scratch block before the test; a block read straight from the disk;
 * block 0 as reader a got it. */
static u8 saved[BSIZE];
static u8 raw[BSIZE];
static u8 first[BSIZE];

static void check(bool ok, const char *what) {
    if (ok) {
        passed++;
    } else {
        failed++;
        kprintf("blk self-test FAILED: %s\n", what);
    }
}

/* The test pattern: byte i is i * 7 + seed. */
static void fill(u8 *p, u32 seed) {
    for (u32 i = 0; i < BSIZE; i++) {
        p[i] = (u8)(i * 7 + seed);
    }
}

/* What the scratch block is written with: the pattern, or on a disk with
 * a filesystem the block's own contents (`saved`). */
static u8 pattern[BSIZE];

/* Whether disk 0 holds a Camelot filesystem: its magic at the start of
 * block 1 (docs/camelot-fs.md), read past the cache. */
static bool holds_filesystem(void) {
    u32 magic;
    if (blk_read(0, SUPERBLOCK_NO * BLOCK_SECTORS, raw, BLOCK_SECTORS) != BLK_OK) {
        return false;
    }
    memcpy(&magic, raw, sizeof magic);
    return magic == FS_MAGIC;
}

/* ---- Three readers at once ---- */

static void reader_a(void) {
    struct buf *b = bread(0, 0);
    check(b->error == BLK_OK && b->valid, "reader a: block 0");
    memcpy(first, b->data, BSIZE);
    brelse(b);
}

static void reader_b(void) {
    struct buf *b = bread(0, 1);
    check(b->error == BLK_OK && b->valid, "reader b: block 1");
    brelse(b);
}

static void reader_c(void) {
    struct buf *b = bread(0, 0);
    check(b->error == BLK_OK && b->valid && memcmp(b->data, first, BSIZE) == 0,
          "reader c: block 0 as reader a got it");
    brelse(b);
}

/* ---- One process, step by step ---- */

static void scratch_test(void) {
    u32 scratch = nblocks - 1;
    u32 sector = scratch * BLOCK_SECTORS;
    bool read_only = blk_read_only(0);

    bool fs = holds_filesystem();

    /* Block 0 came from the disk as it is. */
    check(blk_read(0, 0, raw, BLOCK_SECTORS) == BLK_OK && memcmp(raw, first, BSIZE) == 0,
          "block 0 in the cache matches the disk");

    /* On a two-block disk reader b left the scratch block cached. */
    struct buf *b = bread(0, scratch);
    check(b->error == BLK_OK && b->valid, "first read of the scratch block");
    u32 hits = bio_hits;
    u32 misses = bio_misses;
    memcpy(saved, b->data, BSIZE);
    if (fs) {
        memcpy(pattern, saved, BSIZE);
    } else {
        fill(pattern, 3);
    }
    memcpy(b->data, pattern, BSIZE);
    u32 err = bwrite(b);
    brelse(b);

    b = bread(0, scratch);
    if (read_only) {
        check(err == BLK_ERR_READ_ONLY, "a write to a read-only disk fails");
        /* The refused data left the cache: this read went to the disk. */
        check(b->valid && bio_misses == misses + 1 && memcmp(b->data, saved, BSIZE) == 0,
              "after a refused write, the block is read again");
    } else {
        check(err == BLK_OK, "write the scratch block");
        check(b->valid && bio_hits == hits + 1 && memcmp(b->data, pattern, BSIZE) == 0,
              "read it back from the cache");
    }
    brelse(b);

    /* Past the cache, what the disk holds. */
    err = blk_read(0, sector, raw, BLOCK_SECTORS);
    if (read_only) {
        check(err == BLK_OK && memcmp(raw, saved, BSIZE) == 0, "the read-only disk is unchanged");
        return;
    }
    check(err == BLK_OK && memcmp(raw, pattern, BSIZE) == 0, "the disk holds the written block");

    /* Put the old contents back. */
    b = bread(0, scratch);
    memcpy(b->data, saved, BSIZE);
    err = bwrite(b);
    brelse(b);
    check(err == BLK_OK && blk_read(0, sector, raw, BLOCK_SECTORS) == BLK_OK && memcmp(raw, saved, BSIZE) == 0,
          "restore the scratch block");
    check(blk_flush(0) == BLK_OK, "flush");
}

static void error_test(void) {
    u32 misses = bio_misses;
    struct buf *b = bread(0, nblocks);
    check(b->error == BLK_ERR_OUT_OF_RANGE && !b->valid, "a block past the end is out of range");
    brelse(b);
    b = bread(0, nblocks);
    check(b->error == BLK_ERR_OUT_OF_RANGE && bio_misses == misses + 2, "a failed read isn't cached");
    brelse(b);
    check(blk_read(0, 0, (void *)NOT_RAM, 1) == BLK_ERR_BAD_DMA, "a buffer outside RAM is refused");
}

static void lru_test(void) {
    if (nblocks < NBUF + 1) {
        return;
    }
    /* Blocks 0 to NBUF - 1 fill every buffer, block 0 the least recently
     * used. Reading block 0 again leaves block 1 the least recent, so block
     * NBUF takes block 1's buffer. (Taking the most recent one instead would
     * drop block 0.) */
    for (u32 i = 0; i < NBUF; i++) {
        brelse(bread(0, i));
    }
    brelse(bread(0, 0));
    brelse(bread(0, NBUF));
    u32 hits = bio_hits;
    u32 misses = bio_misses;
    brelse(bread(0, 0));
    brelse(bread(0, NBUF));
    check(bio_hits == hits + 2 && bio_misses == misses, "the blocks used last are cached");
    brelse(bread(0, 1));
    check(bio_misses == misses + 1, "the least recently used block was reused");
}

static void step_by_step(void) {
    scratch_test();
    error_test();
    lru_test();
}

static void start(const char *name, void (*fn)(void)) {
    const char *err;
    if (kproc_create(name, fn, &err) == NULL) {
        failed++;
        kprintf("blk self-test FAILED: cannot start %s: %s\n", name, err);
    }
}

int blk_selftest(void) {
    if (blk_count() == 0) {
        return 0;
    }
    nblocks = blk_capacity(0) / BLOCK_SECTORS;
    /* Reader b wants block 1, and the scratch block (the last) must not be
     * block 0, which the readers check against. */
    if (nblocks < 2) {
        kprintf("blk self-test: disk 0 is too small, skipped\n");
        return 0;
    }

    u32 hits = bio_hits;
    u32 misses = bio_misses;
    start("blkread-a", reader_a);
    start("blkread-b", reader_b);
    start("blkread-c", reader_c);
    scheduler();
    check(bio_misses == misses + 2 && bio_hits == hits + 1, "three readers, two disk reads");

    start("blktest", step_by_step);
    scheduler();

    kprintf("blk self-test: %d/%d passed\n", passed, passed + failed);
    return failed;
}
