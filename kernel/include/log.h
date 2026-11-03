/* The filesystem's write-ahead log (docs/camelot-fs.md, "The log"), after
 * xv6's log.c. A filesystem operation that changes the disk is bracketed:
 *
 *   begin_op();
 *   b = fs_bread(dev, blockno);
 *   ... change b->data ...
 *   log_write(b);       instead of bwrite(b)
 *   brelse(b);
 *   ...
 *   end_op();
 *
 * log_write() only records the block in the transaction and pins its buffer
 * in the cache; nothing reaches the disk until the transaction commits,
 * which end_op() does when the last operation in it ends: the blocks are
 * copied to the log, the log header is written (the commit point), then the
 * blocks go to their homes and the header is cleared. A crash before the
 * header write loses the transaction; one after it is completed by the
 * replay at the next mount. */
#ifndef CAMELOT_LOG_H
#define CAMELOT_LOG_H

#include "bio.h"
#include "fs.h"
#include "types.h"

/* The most blocks one operation may change, and the most a transaction
 * holds (the log's slots, if it has fewer). MAXOPBLOCKS is the worst case
 * of an operation, counted in docs/camelot-fs.md ("Limits on
 * transactions"): a write's chunk, when adding a block splits the extent
 * tree at every level. LOGSIZE pinned buffers must leave enough of the
 * NBUF for everyone else. */
#define MAXOPBLOCKS 22
#define LOGSIZE 44 /* room for two operations at their worst */

/* Blocks the mount's replay installed (0 if the log was empty). */
extern u32 log_recovered;

/* Transactions committed since boot. */
extern u32 log_commits;

/* For the self-test: how often begin_op() has gone to sleep because the
 * operations in progress could fill the log, and because a commit was
 * under way; and whether one is now (its writer sleeping on the disk). */
extern u32 log_room_waits;
extern u32 log_commit_waits;
bool log_committing(void);

/* For the self-test: the distinct blocks the transaction being built
 * holds so far (an operation's share, when it runs alone). */
u32 log_blocks(void);

/* Sets the log up for the filesystem `sb` on disk `dev` and replays a
 * committed transaction if the header holds one. Returns 0, -EINVAL with
 * *why for a header that breaks the format's rules, or -EROFS if there is
 * one to replay on a read-only disk. Called by fs_mount(). */
int log_init(u32 dev, const struct superblock *sb, const char **why);

/* Starts an operation: waits while a commit is under way or the log could
 * not hold MAXOPBLOCKS more blocks. */
void begin_op(void);

/* Ends an operation; the last one in a transaction commits it. */
void end_op(void);

/* Records buffer `b` (locked, changed) as part of the current transaction,
 * once however often it is called, and pins it in the cache until the
 * commit has written it home. Panics outside begin_op()/end_op(), when the
 * transaction is full, and on a read-only disk. */
void log_write(struct buf *b);

#endif
