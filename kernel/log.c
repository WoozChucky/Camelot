/* The filesystem's write-ahead log (docs/camelot-fs.md, "The log"; spec
 * section 10, "Kernel filesystem"), after xv6's log.c.
 *
 * A transaction is the changes of one or more operations (begin_op() to
 * end_op()). Operations of several processes may share one: the log counts
 * those in progress (`outstanding`), and the last to end commits for all of
 * them (group commit). While a commit runs, or while the operations in
 * progress could fill the log if each wrote MAXOPBLOCKS blocks, a new
 * operation waits in begin_op().
 *
 * The changed blocks wait in the buffer cache, pinned, until the commit:
 *
 *   1. copy each to its log slot          (write_slots)
 *   2. flush
 *   3. write the header with n > 0        (the commit point)
 *   4. flush
 *   5. write each block home              (install)
 *   6. flush
 *   7. write the header with n = 0
 *   8. flush
 *
 * The flushes make each step durable before the next starts, so a disk that
 * reorders writes can't put the header before its slots, the homes before
 * the header, or the next transaction's slots before the empty header.
 * Grail's disks write in order anyway; flushing costs a request each.
 *
 * Locking: log.lock, a spinlock, protects the fields; begin_op() sleeps on
 * the log with it. The commit itself runs without it (it sleeps on the
 * disk), with `committing` set so nobody else touches the transaction
 * meanwhile: a new operation waits in begin_op(), and log_write() and
 * end_op() can't happen, every operation having ended. A disk error while
 * committing panics: the log on the disk is then still good, and the next
 * mount replays or drops the transaction as a crash would. */
#include "log.h"
#include "kprintf.h"
#include "proc.h"
#include "spinlock.h"
#include "string.h"
#include "trap.h"

/* The header as the kernel keeps it: the home blocks of the transaction
 * being built, in slot order. */
struct logheader {
    u32 n;
    u32 block[LOGSIZE];
};

static struct {
    struct spinlock lock;
    u32 dev;
    u32 start; /* the header block; slot i is start + 1 + i */
    u32 slots; /* how many blocks a transaction may hold */
    u32 outstanding;
    bool committing;
    struct logheader lh;
} log;

u32 log_recovered;
u32 log_commits;
u32 log_room_waits;
u32 log_commit_waits;

/* The replay's list of home blocks: up to LOG_MAX - 1, whatever LOGSIZE is,
 * since the disk may come from a writer with a bigger log. */
static u32 replay_blocks[LOG_MAX - 1];

static void flush(void) {
    if (blk_flush(log.dev) != BLK_OK) {
        panic("log: flush failed");
    }
}

static void write_or_panic(struct buf *b) {
    if (bwrite(b) != BLK_OK) {
        panic("log: disk write failed");
    }
}

/* Writes the header: `n` and the first n of `blocks`, zeros after. Writing
 * n > 0 commits; writing 0 empties the log. */
static void write_header(u32 n, const u32 *blocks) {
    struct buf *b = fs_bread(log.dev, log.start);
    u32 *h = (u32 *)b->data;
    memset(b->data, 0, BSIZE);
    h[0] = n;
    for (u32 i = 0; i < n; i++) {
        h[1 + i] = blocks[i];
    }
    write_or_panic(b);
    brelse(b);
}

/* Step 1: copies each block of the transaction, pinned in the cache, to its
 * slot. */
static void write_slots(void) {
    for (u32 i = 0; i < log.lh.n; i++) {
        struct buf *to = fs_bread(log.dev, log.start + 1 + i);
        struct buf *from = fs_bread(log.dev, log.lh.block[i]);
        memmove(to->data, from->data, BSIZE);
        write_or_panic(to);
        brelse(from);
        brelse(to);
    }
}

/* Step 5: writes each block home from the cache, where it is pinned, and
 * unpins it. */
static void install(void) {
    for (u32 i = 0; i < log.lh.n; i++) {
        struct buf *b = fs_bread(log.dev, log.lh.block[i]);
        write_or_panic(b);
        bunpin(b);
        brelse(b);
    }
}

/* Returns whether there was anything to commit. */
static bool commit(void) {
    if (log.lh.n == 0) {
        return false;
    }
    write_slots();
    flush();
    write_header(log.lh.n, log.lh.block);
    flush();
    install();
    flush();
    log.lh.n = 0;
    write_header(0, log.lh.block);
    flush();
    return true;
}

/* The mount's replay: installs `n` slots from the disk (nothing is in the
 * cache yet), then empties the log. */
static void replay(u32 n) {
    for (u32 i = 0; i < n; i++) {
        struct buf *from = fs_bread(log.dev, log.start + 1 + i);
        struct buf *to = fs_bread(log.dev, replay_blocks[i]);
        memmove(to->data, from->data, BSIZE);
        write_or_panic(to);
        brelse(to);
        brelse(from);
    }
    flush();
    write_header(0, replay_blocks);
    flush();
}

int log_init(u32 dev, const struct superblock *sb, const char **why) {
    /* At the mount, before any operation: nothing else uses the log yet. */
    initlock(&log.lock, "log");
    log.dev = dev;
    log.start = sb->logstart;
    log.slots = sb->nlog - 1 < LOGSIZE ? sb->nlog - 1 : LOGSIZE;
    log.outstanding = 0;
    log.committing = false;
    log.lh.n = 0;
    log_recovered = 0;
    log_commits = 0;

    /* The header's rules: n fits the log, the homes are distinct and none
     * is the boot block, the superblock or the log. */
    struct buf *b = fs_bread(dev, log.start);
    const u32 *h = (const u32 *)b->data;
    u32 n = h[0];
    if (n > sb->nlog - 1) {
        brelse(b);
        *why = "the log header counts more blocks than the log has";
        return -EINVAL;
    }
    for (u32 i = 0; i < n; i++) {
        u32 home = h[1 + i];
        if (home < sb->inodestart || home >= sb->size) {
            brelse(b);
            *why = "the log header names a block outside the filesystem";
            return -EINVAL;
        }
        for (u32 j = 0; j < i; j++) {
            if (replay_blocks[j] == home) {
                brelse(b);
                *why = "the log header names a block twice";
                return -EINVAL;
            }
        }
        replay_blocks[i] = home;
    }
    brelse(b);
    if (n == 0) {
        return 0;
    }
    if (blk_read_only(dev)) {
        *why = "the disk is read-only and its log holds a transaction to replay";
        return -EROFS;
    }
    replay(n);
    log_recovered = n;
    return 0;
}

void begin_op(void) {
    acquire(&log.lock);
    for (;;) {
        if (log.committing) {
            log_commit_waits++;
            sleep(&log, &log.lock);
        } else if (log.lh.n + (log.outstanding + 1) * MAXOPBLOCKS > log.slots && log.outstanding > 0) {
            /* This operation might fill the log: wait for the transaction
             * to commit, or for an operation in it to end without having
             * used all of its room. (Alone, an operation always starts: a
             * writable mount has at least MAXOPBLOCKS slots, see
             * fs_mount(), and a read-only one writes nothing.) */
            log_room_waits++;
            sleep(&log, &log.lock);
        } else {
            log.outstanding++;
            release(&log.lock);
            return;
        }
    }
}

bool log_committing(void) {
    acquire(&log.lock);
    bool committing = log.committing;
    release(&log.lock);
    return committing;
}

void end_op(void) {
    acquire(&log.lock);
    if (log.outstanding == 0) {
        panic("end_op: no operation in progress");
    }
    if (log.committing) {
        panic("end_op: committing");
    }
    log.outstanding--;
    if (log.outstanding > 0) {
        /* begin_op() may be waiting for the room this one reserved. */
        wakeup(&log);
        release(&log.lock);
        return;
    }
    /* The last operation: commit, without the lock (the commit sleeps for
     * the disk) but with `committing` set, so nobody starts an operation
     * meanwhile. */
    log.committing = true;
    release(&log.lock);
    bool committed = commit();
    acquire(&log.lock);
    log.committing = false;
    if (committed) {
        log_commits++;
    }
    wakeup(&log);
    release(&log.lock);
}

void log_write(struct buf *b) {
    acquire(&log.lock);
    if (log.outstanding == 0) {
        panic("log_write outside a transaction");
    }
    if (fs_readonly) {
        panic("log_write: read-only filesystem");
    }
    if (b->dev != log.dev) {
        panic("log_write: another disk");
    }
    for (u32 i = 0; i < log.lh.n; i++) {
        if (log.lh.block[i] == b->blockno) {
            release(&log.lock);
            return; /* absorbed: already in this transaction */
        }
    }
    if (log.lh.n >= log.slots) {
        panic("log_write: transaction too big");
    }
    log.lh.block[log.lh.n++] = b->blockno;
    /* bcache_lock inside log.lock: the log comes first in the lock order. */
    bpin(b);
    release(&log.lock);
}
