/* The filesystem (docs/camelot-fs.md; spec section 10, "Kernel
 * filesystem"), after xv6's fs.c: mounting, the block allocator, inodes,
 * file contents, directories and paths, and the create/unlink/link
 * operations the file system calls (sysfile.c) use. The log is in log.c.
 *
 * Locking, as in xv6:
 *
 * - an in-memory inode is shared by everyone using its file: `ref` counts
 *   them, under itable_lock, a spinlock that also covers which inode each
 *   slot holds; and `lock` is a sleep lock held while its fields are read
 *   or changed (ilock to iunlock). Holding a reference keeps the inode in
 *   the cache; it says nothing about its fields;
 * - blocks are locked by the buffer cache, from bread to brelse;
 * - lookups lock one directory at a time, going down the path, so they
 *   never wait for each other in a cycle (`..` is looked up with the child
 *   unlocked first).
 *
 * Every function that may write runs inside a transaction (log.h), and
 * changes blocks with log_write() rather than bwrite(). */
#include "fs.h"
#include "kprintf.h"
#include "log.h"
#include "proc.h"
#include "string.h"
#include "trap.h"

bool fs_mounted;
bool fs_readonly;
bool fs_log_too_small;
u32 fs_truncations;
u32 fs_orphans;
u32 fs_dev;
struct superblock fs_sb;

static struct inode itable[NINODE];
static struct spinlock itable_lock;

void fs_init(void) {
    initlock(&itable_lock, "itable");
    for (u32 i = 0; i < NINODE; i++) {
        initsleeplock(&itable[i].lock, "inode");
        initsleeplock(&itable[i].wlock, "inode write");
    }
}

/* iget() panics when the cache is full, so it must hold every inode that
 * can be in use at once: one per open file (an FD_INODE file, or a device
 * opened by its path, keeps its inode until closed), one per process's
 * working directory, and the few each thread holds for a moment inside a
 * call, at most INODES_PER_CALL (link() keeps `old` while walking `new`'s
 * path, which holds a directory and the entry found in it); every thread of
 * a process may be in a call at once. A user can then run out of open
 * files (-ENFILE), never out of inodes. */
#define INODES_PER_CALL 3
_Static_assert(NINODE >= NFILE + NPROC + NTHREAD * INODES_PER_CALL, "the inode cache holds every user");

#define MIN(a, b) ((a) < (b) ? (a) : (b))

struct buf *fs_bread(u32 dev, u32 blockno) {
    struct buf *b = bread(dev, blockno);
    if (b->error != BLK_OK) {
        kprintf("fs: block %u of disk %u: error %u\n", blockno, dev, b->error);
        panic("fs: disk read failed");
    }
    return b;
}

/* ceil(a / b). */
static u32 div_ceil(u32 a, u32 b) {
    return a / b + (a % b != 0);
}

/* Checks the superblock's rules (docs/camelot-fs.md, "Superblock"); NULL if
 * they hold, else what is wrong. */
static const char *check_superblock(const struct superblock *sb, u32 disk_sectors) {
    if (sb->version != FS_VERSION) {
        return "unknown version";
    }
    if (sb->nlog < LOG_MIN || sb->nlog > LOG_MAX) {
        return "bad log size";
    }
    if (sb->ninodes < INODES_MIN || sb->ninodes > INODES_MAX) {
        return "bad inode count";
    }
    if (sb->size > 0x80000000u) {
        return "bigger than a filesystem can be";
    }
    if (sb->logstart != LOGSTART || sb->inodestart != sb->logstart + sb->nlog ||
        sb->bmapstart != sb->inodestart + div_ceil(sb->ninodes, IPB) ||
        sb->datastart != sb->bmapstart + div_ceil(sb->size, BPB) || sb->datastart >= sb->size ||
        sb->nblocks != sb->size - sb->datastart) {
        return "the regions are misplaced";
    }
    if (sb->size > disk_sectors / BLOCK_SECTORS) {
        return "bigger than the disk";
    }
    return NULL;
}

/* Finishes the truncations a crash interrupted: files with
 * INODE_TRUNCATING set and links, whose remaining blocks itrunc() frees as
 * it would have (docs/camelot-fs.md, "Truncation"), in transactions of its
 * own. An orphan being freed is left to free_orphans(), which frees it
 * whole. Returns how many it finished. */
static u32 finish_truncations(void) {
    u32 n = 0;
    for (u32 inum = ROOTINO; inum < fs_sb.ninodes; inum++) {
        struct buf *b = fs_bread(fs_dev, IBLOCK(inum, fs_sb));
        const struct dinode *dip = (const struct dinode *)b->data + inum % IPB;
        bool truncating = dip->type != T_FREE && dip->nlink != 0 && (dip->flags & INODE_TRUNCATING) != 0;
        brelse(b);
        if (!truncating) {
            continue;
        }
        struct inode *ip = iget(fs_dev, inum);
        acquiresleep(&ip->wlock);
        begin_op();
        ilock(ip);
        itrunc(ip);
        iunlock(ip);
        end_op();
        releasesleep(&ip->wlock);
        begin_op();
        iput(ip);
        end_op();
        n++;
    }
    return n;
}

/* Frees the inodes a crash left allocated with no links. fs_unlink() takes
 * a file's last link in one transaction, and the last iput() frees it in
 * another, when whoever still used it (an open file, a working directory)
 * lets go; a crash
 * in between leaves it on the disk, unnamed (docs/camelot-fs.md, "Link
 * count"). Each is freed by iput() as it would have been, in transactions of
 * its own. The root is never one: if its count is 0 the disk is broken,
 * and freeing it would make that worse. Returns how many were freed. */
static u32 free_orphans(void) {
    u32 n = 0;
    for (u32 inum = ROOTINO + 1; inum < fs_sb.ninodes; inum++) {
        struct buf *b = fs_bread(fs_dev, IBLOCK(inum, fs_sb));
        const struct dinode *dip = (const struct dinode *)b->data + inum % IPB;
        bool orphan = dip->type != T_FREE && dip->nlink == 0;
        brelse(b);
        if (!orphan) {
            continue;
        }
        begin_op();
        struct inode *ip = iget(fs_dev, inum);
        ilock(ip); /* reads it in: valid, with no links */
        iunlock(ip);
        iput(ip); /* the last reference: frees its blocks and the inode */
        end_op();
        n++;
    }
    return n;
}

int fs_mount(u32 dev, const char **why) {
    if (fs_mounted) {
        panic("fs_mount: already mounted");
    }
    /* Too small to hold a superblock: certainly no filesystem. */
    if (blk_capacity(dev) < (SUPERBLOCK_NO + 1) * BLOCK_SECTORS) {
        *why = "no filesystem";
        return -ENOENT;
    }
    struct buf *b = bread(dev, SUPERBLOCK_NO);
    if (b->error != BLK_OK) {
        brelse(b);
        *why = "cannot read the superblock";
        return -EIO;
    }
    memcpy(&fs_sb, b->data, sizeof fs_sb);
    bool padded = true;
    for (u32 i = sizeof fs_sb; i < BSIZE; i++) {
        padded = padded && b->data[i] == 0;
    }
    brelse(b);
    if (fs_sb.magic != FS_MAGIC) {
        *why = "no filesystem";
        return -ENOENT;
    }
    const char *bad = check_superblock(&fs_sb, blk_capacity(dev));
    if (bad == NULL && !padded) {
        bad = "the superblock's bytes past its fields are not zero";
    }
    if (bad != NULL) {
        *why = bad;
        return -EINVAL;
    }
    fs_dev = dev;
    fs_readonly = blk_read_only(dev);
    int err = log_init(dev, &fs_sb, why);
    if (err < 0) {
        return err;
    }
    /* Straight from the inode block: ilock() would panic on a free inode. */
    b = fs_bread(dev, IBLOCK(ROOTINO, fs_sb));
    bool is_dir = ((const struct dinode *)b->data + ROOTINO % IPB)->type == T_DIR;
    brelse(b);
    if (!is_dir) {
        *why = "the root is not a directory";
        return -EINVAL;
    }
    /* An operation may change MAXOPBLOCKS blocks, and begin_op() lets one
     * start alone whatever the log's size: with fewer slots it could
     * overflow the transaction. The format allows such a log (the
     * replay above is fine), but the kernel only reads that disk. */
    fs_log_too_small = !fs_readonly && fs_sb.nlog - 1 < MAXOPBLOCKS;
    if (fs_log_too_small) {
        fs_readonly = true;
    }
    fs_mounted = true;
    fs_truncations = fs_readonly ? 0 : finish_truncations();
    fs_orphans = fs_readonly ? 0 : free_orphans();
    return 0;
}

void fs_count_free(u32 *blocks, u32 *inodes) {
    *blocks = 0;
    for (u32 bb = fs_sb.bmapstart; bb < fs_sb.datastart; bb++) {
        struct buf *b = fs_bread(fs_dev, bb);
        u32 first = (bb - fs_sb.bmapstart) * BPB;
        for (u32 i = 0; i < BPB && first + i < fs_sb.size; i++) {
            if (first + i >= fs_sb.datastart && (b->data[i / 8] & (1 << (i % 8))) == 0) {
                (*blocks)++;
            }
        }
        brelse(b);
    }
    *inodes = 0;
    for (u32 inum = 1; inum < fs_sb.ninodes; inum++) {
        struct buf *b = fs_bread(fs_dev, IBLOCK(inum, fs_sb));
        const struct dinode *dip = (const struct dinode *)b->data + inum % IPB;
        if (dip->type == T_FREE) {
            (*inodes)++;
        }
        brelse(b);
    }
}

/* ---- Blocks ---- */

/* Zeroes block `bno`, in the transaction. */
static void bzero(u32 dev, u32 bno) {
    struct buf *b = fs_bread(dev, bno);
    memset(b->data, 0, BSIZE);
    log_write(b);
    brelse(b);
}

/* Takes data block `bno` if its bit is clear: sets it and zeroes the block.
 * Whether it did. */
static bool btake(u32 dev, u32 bno) {
    struct buf *b = fs_bread(dev, BBLOCK(bno, fs_sb));
    u32 i = bno % BPB;
    u8 mask = (u8)(1 << (i % 8));
    bool was_free = (b->data[i / 8] & mask) == 0;
    if (was_free) {
        b->data[i / 8] |= mask;
        log_write(b);
    }
    brelse(b);
    if (was_free) {
        bzero(dev, bno);
    }
    return was_free;
}

/* Where the lowest 1 of `x`, which has one, is: whole zero bytes skipped
 * eight bits a step, then the bits of the byte that has it one at a time,
 * fourteen steps at most. Excalibur has no instruction that counts the
 * trailing zeros (Zbb's ctz), and this runs once per block allocated. */
static u32 first_one(u64 x) {
    u32 at = 0;
    for (; (x & 0xff) == 0; x >>= 8) {
        at += 8;
    }
    for (; (x & 1) == 0; x >>= 1) {
        at++;
    }
    return at;
}

/* The first clear bit of a bitmap of `nbits` bits at `map`, at `start`
 * or after it; `nbits` when every one of those is set. A 64-bit word at a
 * time, as ext4's allocator scans its bitmaps (Linux's
 * find_next_zero_bit_le): a word of 64 blocks in use is one comparison,
 * where testing bit by bit is 64 rounds.
 *
 * Bit i is bit i % 8 of byte i / 8 (docs/camelot-fs.md, "Free-block
 * bitmap"), which on little-endian Excalibur is bit i % 64 of 64-bit word
 * i / 64: inverting a word makes its clear bits the ones to look for. In
 * the word holding `start` the bits below it are cleared from that
 * inverse first, and a hit in the last word past `nbits` (a disk that
 * ends inside it) is no hit. */
static u32 bitmap_first_clear(const u64 *map, u32 start, u32 nbits) {
    for (u32 w = start / 64; w * 64 < nbits; w++) {
        u64 clear = ~map[w];
        if (w == start / 64) {
            clear &= ~(((u64)1 << (start % 64)) - 1);
        }
        if (clear != 0) {
            u32 bit = w * 64 + first_one(clear);
            return bit < nbits ? bit : nbits;
        }
    }
    return nbits;
}

/* Allocates a data block, zeroed; 0 if none is free. `goal` first, when it
 * is a free data block (the block after a file's last, so that the file
 * stays one extent; 0 for none), else the first free block from the start
 * of the data region: in each bitmap block, the first clear bit at or
 * after the data region's start and before the disk's end. A bitmap
 * block's data is 8-aligned (bio.h), so it is read as 64-bit words. */
static u32 balloc(u32 dev, u32 goal) {
    if (goal >= fs_sb.datastart && goal < fs_sb.size && btake(dev, goal)) {
        return goal;
    }
    for (u32 base = 0; base < fs_sb.size; base += BPB) {
        struct buf *b = fs_bread(dev, BBLOCK(base, fs_sb));
        u32 bits = fs_sb.size - base < BPB ? fs_sb.size - base : BPB;
        u32 from = fs_sb.datastart > base ? fs_sb.datastart - base : 0;
        u32 i = bitmap_first_clear((const u64 *)b->data, from, bits);
        if (i < bits) {
            b->data[i / 8] |= (u8)(1 << (i % 8));
            log_write(b);
            brelse(b);
            bzero(dev, base + i);
            return base + i;
        }
        brelse(b);
    }
    return 0;
}

/* Frees data block `bno`. */
static void bfree(u32 dev, u32 bno) {
    if (bno < fs_sb.datastart || bno >= fs_sb.size) {
        panic("bfree: not a data block");
    }
    struct buf *b = fs_bread(dev, BBLOCK(bno, fs_sb));
    u32 i = bno % BPB;
    u8 mask = (u8)(1 << (i % 8));
    if ((b->data[i / 8] & mask) == 0) {
        panic("bfree: block already free");
    }
    b->data[i / 8] &= (u8)~mask;
    log_write(b);
    brelse(b);
}

/* ---- Extent trees ----
 *
 * A file's blocks are mapped by an extent tree (docs/camelot-fs.md, "Extent
 * trees"): its root is the inode's `root` (4 entries), and when that is not
 * enough, the root holds index entries pointing to tree blocks of 84
 * entries, which hold extents or more index entries, up to EXT_MAX_DEPTH
 * levels. A node is reached by its block number: 0 for the root (which the
 * in-memory inode holds, written by iupdate()), else a tree block, read
 * through the buffer cache. Every function here takes a locked inode. */

/* A node's header, and its entry `i` as an extent or an index entry. */
static struct ext_header *ext_hdr(void *node) {
    return (struct ext_header *)node;
}

static struct ext_extent *ext_ent(void *node, u32 i) {
    return (struct ext_extent *)((u8 *)node + sizeof(struct ext_header) + i * sizeof(struct ext_extent));
}

static struct ext_index *ext_idx(void *node, u32 i) {
    return (struct ext_index *)ext_ent(node, i);
}

/* Node `blockno` of ip's tree, which must be at `depth`; *bp is its buffer
 * (NULL for the root), for node_put(). The kernel trusts a mounted disk,
 * but a node that isn't one panics here rather than send the walks past
 * their arrays (a depth over EXT_MAX_DEPTH would overrun a path), as ext4
 * checks every node it reads (__ext4_ext_check): the magic, the room, the
 * entries, the depth, and that only the root is ever empty. */
static void *node_get(struct inode *ip, u32 blockno, u32 depth, struct buf **bp) {
    void *n;
    u32 max;
    if (blockno == 0) {
        *bp = NULL;
        n = &ip->root;
        max = EXT_ROOT_MAX;
    } else {
        *bp = fs_bread(ip->dev, blockno);
        n = (*bp)->data;
        max = EXT_BLOCK_MAX;
    }
    struct ext_header *h = ext_hdr(n);
    if (h->magic != EXT_MAGIC || h->max != max || h->entries > max || h->depth != depth ||
        depth > EXT_MAX_DEPTH || (h->entries == 0 && (blockno != 0 || depth != 0))) {
        kprintf("fs: inode %u: extent tree block %u is broken\n", ip->inum, blockno);
        panic("fs: a broken extent tree");
    }
    return n;
}

/* Lets go of a node node_get() returned, logging it if it `changed` (the
 * root is the inode's: iupdate() writes it). */
static void node_put(struct buf *b, bool changed) {
    if (b != NULL) {
        if (changed) {
            log_write(b);
        }
        brelse(b);
    }
}

static void set_extent(struct ext_extent *e, u32 block, u32 start) {
    e->block = block;
    e->len = 1;
    e->start_hi = 0;
    e->start_lo = start;
}

static void set_index(struct ext_index *x, u32 block, u32 child) {
    x->block = block;
    x->leaf_lo = child;
    x->leaf_hi = 0;
    x->unused = 0;
}

/* A new tree's root: an empty leaf. */
static void ext_init(struct ext_node *root) {
    memset(root, 0, sizeof *root);
    root->h.magic = EXT_MAGIC;
    root->h.max = EXT_ROOT_MAX;
}

/* The rightmost path, from the root (path[0] = 0) to the leaf, the last
 * child at each level. Returns the tree's depth: path[depth] is the leaf. */
static u32 ext_rightmost(struct inode *ip, u32 path[EXT_MAX_DEPTH + 1]) {
    u32 depth = ip->root.h.depth;
    path[0] = 0;
    for (u32 l = 0; l < depth; l++) {
        struct buf *b;
        void *n = node_get(ip, path[l], depth - l, &b);
        path[l + 1] = ext_idx(n, ext_hdr(n)->entries - 1u)->leaf_lo;
        node_put(b, false);
    }
    return depth;
}

/* The disk block holding logical block `bn`, 0 if the tree maps none
 * ("Lookup": the last entry starting at or before it, at every level). */
static u32 ext_lookup(struct inode *ip, u32 bn) {
    u32 blockno = 0;
    u32 depth = ip->root.h.depth;
    for (;; depth--) {
        struct buf *b;
        void *n = node_get(ip, blockno, depth, &b);
        struct ext_header *h = ext_hdr(n);
        u32 i = h->entries;
        u32 found = 0;
        if (h->depth == 0) {
            while (i > 0) {
                i--;
                struct ext_extent *e = ext_ent(n, i);
                if (e->block <= bn) {
                    if (bn - e->block < e->len) {
                        found = e->start_lo + (bn - e->block);
                    }
                    break;
                }
            }
            node_put(b, false);
            return found;
        }
        while (i > 0) {
            i--;
            struct ext_index *x = ext_idx(n, i);
            if (x->block <= bn) {
                found = x->leaf_lo;
                break;
            }
        }
        node_put(b, false);
        if (found == 0) {
            return 0;
        }
        blockno = found;
    }
}

/* Where the file's next block should go: the disk block after its last,
 * or 0 (anywhere) if it has none. */
static u32 ext_goal(struct inode *ip) {
    u32 path[EXT_MAX_DEPTH + 1];
    u32 depth = ext_rightmost(ip, path);
    struct buf *b;
    void *leaf = node_get(ip, path[depth], 0, &b);
    u32 goal = 0;
    if (ext_hdr(leaf)->entries > 0) {
        struct ext_extent *e = ext_ent(leaf, ext_hdr(leaf)->entries - 1u);
        goal = e->start_lo + e->len;
    }
    node_put(b, false);
    return goal;
}

/* Maps logical block `bn`, the one after the file's last, to disk block `b`
 * ("Growing a file"): the last extent grows if `b` follows it, else a new
 * extent goes at the end, in the rightmost leaf, or in a new leaf (with a
 * chain of new index nodes) under the lowest node on the rightmost path
 * that has room; if none has, the tree first grows a level, the root's
 * entries moving to a new tree block, as ext4's ext4_ext_grow_indepth()
 * does. Every new tree block is allocated before anything changes: 0, or
 * -ENOSPC (the disk is full) or -EFBIG (the tree is full, which no
 * filesystem is big enough for) with the tree as it was. In a transaction:
 * at most 1 + 5 + 5 blocks change here, and the inode (log.h). */
static int ext_append(struct inode *ip, u32 bn, u32 b) {
    u32 path[EXT_MAX_DEPTH + 1];
    u32 depth = ext_rightmost(ip, path);
    struct buf *lb;
    void *leaf = node_get(ip, path[depth], 0, &lb);
    struct ext_header *lh = ext_hdr(leaf);
    if (lh->entries > 0) {
        struct ext_extent *e = ext_ent(leaf, lh->entries - 1u);
        if (e->start_lo + e->len == b && e->len < EXT_MAX_LEN) {
            e->len++;
            node_put(lb, true);
            return 0;
        }
    }
    node_put(lb, false);
    /* The lowest node on the path with room for one more entry. */
    i32 level = -1;
    for (i32 l = (i32)depth; l >= 0 && level < 0; l--) {
        struct buf *nb;
        void *n = node_get(ip, path[l], depth - (u32)l, &nb);
        if (ext_hdr(n)->entries < ext_hdr(n)->max) {
            level = l;
        }
        node_put(nb, false);
    }
    bool grow = level < 0;
    if (grow && depth == EXT_MAX_DEPTH) {
        return -EFBIG;
    }
    u32 new_depth = grow ? depth + 1 : depth;
    if (grow) {
        level = 1; /* the old root's copy, which has room */
    }
    /* New nodes: the old root's copy if the tree grows, then a chain of
     * one-entry nodes from the level below `level` down to a new leaf. */
    u32 chain = new_depth - (u32)level;
    u32 want = chain + (grow ? 1 : 0);
    u32 nodes[EXT_MAX_DEPTH + 1];
    for (u32 i = 0; i < want; i++) {
        nodes[i] = balloc(ip->dev, 0);
        if (nodes[i] == 0) {
            for (u32 j = 0; j < i; j++) {
                bfree(ip->dev, nodes[j]);
            }
            return -ENOSPC;
        }
    }
    u32 next = 0;
    u32 receiver = grow ? 0 : path[level]; /* 0, the root, until set below */
    if (grow) {
        u32 copy = nodes[next++];
        struct buf *cb = fs_bread(ip->dev, copy);
        memmove(cb->data, &ip->root, sizeof ip->root);
        ext_hdr(cb->data)->max = (u16)EXT_BLOCK_MAX;
        log_write(cb);
        brelse(cb);
        ext_init(&ip->root);
        ip->root.h.depth = (u16)new_depth;
        ip->root.h.entries = 1;
        set_index(ext_idx(&ip->root, 0), 0, copy);
        receiver = copy;
    }
    for (u32 i = 0; i < chain; i++) {
        struct buf *cb = fs_bread(ip->dev, nodes[next + i]);
        struct ext_header *h = ext_hdr(cb->data);
        h->magic = EXT_MAGIC;
        h->max = (u16)EXT_BLOCK_MAX;
        h->depth = (u16)(new_depth - ((u32)level + 1 + i));
        h->entries = 1;
        if (i + 1 < chain) {
            set_index(ext_idx(cb->data, 0), bn, nodes[next + i + 1]);
        } else {
            set_extent(ext_ent(cb->data, 0), bn, b);
        }
        log_write(cb);
        brelse(cb);
    }
    struct buf *rb;
    void *r = node_get(ip, receiver, new_depth - (u32)level, &rb);
    struct ext_header *rh = ext_hdr(r);
    if (chain > 0) {
        set_index(ext_idx(r, rh->entries), bn, nodes[next]);
    } else {
        set_extent(ext_ent(r, rh->entries), bn, b);
    }
    rh->entries++;
    node_put(rb, true);
    return 0;
}

/* ---- Truncation ---- */

/* The blocks one step of a truncation has logged, each counted once. */
struct trunc_log {
    u32 n;
    u32 blocks[MAXOPBLOCKS];
};

static void trunc_logged(struct trunc_log *t, u32 b) {
    for (u32 i = 0; i < t->n; i++) {
        if (t->blocks[i] == b) {
            return;
        }
    }
    t->blocks[t->n++] = b;
}

/* What a step may log besides the inode's block. */
#define TRUNC_BUDGET (MAXOPBLOCKS - 1)

/* One transaction's worth of a truncation ("Limits on transactions"):
 * frees ip's blocks from the end, one run at a time (the end of the last
 * extent, as far back as one bitmap block covers), the tree blocks the
 * runs empty, and their entries, while what it logs still fits. Before a
 * run it makes sure the worst it could add fits: the run's bitmap block and
 * leaf, and for each level it could empty, that node's bitmap block and
 * its parent. Returns whether the tree is empty. */
static bool trunc_step(struct inode *ip) {
    struct trunc_log t;
    t.n = 0;
    for (;;) {
        u32 path[EXT_MAX_DEPTH + 1];
        u32 depth = ext_rightmost(ip, path);
        struct buf *lb;
        void *leaf = node_get(ip, path[depth], 0, &lb);
        struct ext_header *lh = ext_hdr(leaf);
        if (lh->entries == 0) {
            node_put(lb, false); /* the root, an empty leaf */
            return true;
        }
        if (t.n + 2 + 2 * depth > TRUNC_BUDGET) {
            node_put(lb, false);
            return false;
        }
        struct ext_extent *e = ext_ent(leaf, lh->entries - 1u);
        u32 last = e->start_lo + e->len - 1u;
        u32 first = last - last % BPB;
        if (first < e->start_lo) {
            first = e->start_lo;
        }
        for (u32 x = first; x <= last; x++) {
            bfree(ip->dev, x);
        }
        trunc_logged(&t, BBLOCK(last, fs_sb));
        if (path[depth] != 0) {
            trunc_logged(&t, path[depth]);
        }
        e->len = (u16)(e->len - (last - first + 1));
        bool empty = false;
        if (e->len == 0) {
            /* A node's bytes past its last entry are 0 (the format's rule). */
            memset(e, 0, sizeof *e);
            lh->entries--;
            empty = lh->entries == 0;
        }
        node_put(lb, true);
        /* The nodes the run emptied, from the leaf up: each is freed and its
         * entry removed from its parent. */
        for (u32 l = depth; empty && l > 0; l--) {
            bfree(ip->dev, path[l]);
            trunc_logged(&t, BBLOCK(path[l], fs_sb));
            struct buf *pb;
            void *p = node_get(ip, path[l - 1], depth - (l - 1), &pb);
            ext_hdr(p)->entries--;
            memset(ext_idx(p, ext_hdr(p)->entries), 0, sizeof(struct ext_index));
            empty = ext_hdr(p)->entries == 0;
            if (path[l - 1] != 0) {
                trunc_logged(&t, path[l - 1]);
            }
            node_put(pb, true);
        }
        if (empty) {
            ext_init(&ip->root); /* every node is gone: an empty leaf again */
        }
    }
}

/* Ends the operation and begins another, `ip` unlocked in between: what
 * was logged so far commits (if anything was), and what follows has an
 * operation's room of its own. */
static void trunc_restart(struct inode *ip) {
    iunlock(ip);
    end_op();
    begin_op();
    ilock(ip);
}

/* itrunc(), and iput()'s freeing of an inode: then `free_inode`, and its
 * type becomes T_FREE in the last transaction, the one that empties it.
 * Returns whether that took transactions of their own: then, but for an
 * inode freed, the caller's operation goes on in a new one already. An
 * inode freed is iput()'s to finish first (see there). */
static bool truncate(struct inode *ip, bool free_inode) {
    ip->size = 0;
    if (ip->type == T_DEVICE || (ip->root.h.depth == 0 && ip->root.h.entries == 0)) {
        /* No blocks (a device's root is all zero): only the inode changes,
         * a block the caller's operation counts already. */
        ip->flags &= ~(u32)INODE_TRUNCATING;
        if (free_inode) {
            ip->type = T_FREE;
        }
        iupdate(ip);
        return false;
    }
    /* Set with the size, before the inode is unlocked for the first time,
     * and cleared in the last transaction: whoever writes the inode in
     * between (a link or an unlink of a file being emptied by O_TRUNC, in
     * one of the windows below) writes a valid state to the disk, size 0
     * and TRUNCATING with the blocks still to free, which a crash leaves to
     * the next mount (finish_truncations). When one transaction is enough
     * and nobody wrote the inode meanwhile, the flag never reaches the
     * disk. */
    ip->flags |= INODE_TRUNCATING;
    /* Freeing blocks takes transactions of its own (docs/camelot-fs.md,
     * "Limits on transactions"), as Linux's eviction takes a journal handle
     * of its own after the unlink's: whatever the caller logged commits
     * first (an unlink's entry and link count: a crash then leaves an
     * orphan, which the next mount frees), and the caller goes on after
     * in a new operation. */
    trunc_restart(ip);
    while (!trunc_step(ip)) {
        iupdate(ip);
        trunc_restart(ip);
    }
    ip->flags &= ~(u32)INODE_TRUNCATING;
    if (free_inode) {
        ip->type = T_FREE;
    } else {
        iupdate(ip);
        trunc_restart(ip);
        return true;
    }
    iupdate(ip);
    return true;
}

void itrunc(struct inode *ip) {
    truncate(ip, false);
}

bool inode_too_big(const struct inode *ip) {
    return ip->size > MAXFILE_BYTES;
}

/* ---- Inodes ---- */

struct inode *ialloc(u32 dev, u16 type) {
    for (u32 inum = 1; inum < fs_sb.ninodes; inum++) {
        struct buf *b = fs_bread(dev, IBLOCK(inum, fs_sb));
        struct dinode *dip = (struct dinode *)b->data + inum % IPB;
        if (dip->type == T_FREE) {
            memset(dip, 0, sizeof *dip);
            dip->type = type;
            if (type != T_DEVICE) {
                ext_init(&dip->root); /* a device's stays all zero */
            }
            log_write(b);
            brelse(b);
            return iget(dev, inum);
        }
        brelse(b);
    }
    return NULL;
}

void iupdate(struct inode *ip) {
    struct buf *b = fs_bread(ip->dev, IBLOCK(ip->inum, fs_sb));
    struct dinode *dip = (struct dinode *)b->data + ip->inum % IPB;
    dip->type = ip->type;
    dip->major = ip->major;
    dip->minor = ip->minor;
    dip->nlink = ip->nlink;
    dip->size = (u32)ip->size;
    dip->size_hi = (u32)(ip->size >> 32);
    memmove(&dip->root, &ip->root, sizeof ip->root);
    dip->flags = ip->flags;
    log_write(b);
    brelse(b);
}

struct inode *iget(u32 dev, u32 inum) {
    struct inode *empty = NULL;
    acquire(&itable_lock);
    for (u32 i = 0; i < NINODE; i++) {
        struct inode *ip = &itable[i];
        if (ip->ref > 0 && ip->dev == dev && ip->inum == inum) {
            ip->ref++;
            release(&itable_lock);
            return ip;
        }
        if (empty == NULL && ip->ref == 0) {
            empty = ip;
        }
    }
    if (empty == NULL) {
        panic("iget: no inodes");
    }
    empty->dev = dev;
    empty->inum = inum;
    empty->ref = 1;
    /* Nobody else has a reference, so nobody holds the sleep lock or
     * reads `valid`. */
    empty->valid = false;
    release(&itable_lock);
    return empty;
}

struct inode *idup(struct inode *ip) {
    acquire(&itable_lock);
    ip->ref++;
    release(&itable_lock);
    return ip;
}

void ilock(struct inode *ip) {
    if (ip == NULL || ip->ref < 1) {
        panic("ilock: no reference");
    }
    acquiresleep(&ip->lock);
    if (!ip->valid) {
        struct buf *b = fs_bread(ip->dev, IBLOCK(ip->inum, fs_sb));
        const struct dinode *dip = (const struct dinode *)b->data + ip->inum % IPB;
        ip->type = dip->type;
        ip->major = dip->major;
        ip->minor = dip->minor;
        ip->nlink = dip->nlink;
        ip->size = (u64)dip->size_hi << 32 | dip->size;
        memmove(&ip->root, &dip->root, sizeof ip->root);
        ip->flags = dip->flags;
        brelse(b);
        ip->valid = true;
        if (ip->type == T_FREE) {
            panic("ilock: free inode");
        }
    }
}

void iunlock(struct inode *ip) {
    if (ip == NULL || !holdingsleep(&ip->lock) || ip->ref < 1) {
        panic("iunlock: not locked");
    }
    releasesleep(&ip->lock);
}

void iput(struct inode *ip) {
    acquire(&itable_lock);
    if (ip->ref == 1 && ip->valid && ip->nlink == 0) {
        /* Nobody else has it (and no directory names it), so nobody can be
         * holding its lock or get a new reference while it is freed: the
         * sleep lock is free, and itable_lock can go while the blocks are
         * freed (which sleeps on the disk). */
        if (ip->lock.locked) {
            panic("iput: locked");
        }
        acquiresleep(&ip->lock);
        release(&itable_lock);
        bool restarted = truncate(ip, true);
        ip->valid = false;
        releasesleep(&ip->lock);
        acquire(&itable_lock);
        if (restarted) {
            /* The freeing took transactions of its own; the caller goes on
             * in a new one. Only now, with the cached copy no longer valid:
             * once the transaction that frees the inode commits, ialloc() may
             * hand the inode out again, and iget() this slot with it, which
             * must then read the new inode from the disk. */
            ip->ref--;
            release(&itable_lock);
            end_op();
            begin_op();
            return;
        }
    }
    ip->ref--;
    release(&itable_lock);
}

void iunlockput(struct inode *ip) {
    iunlock(ip);
    iput(ip);
}

/* ---- Contents ---- */

int readi(struct inode *ip, void *dst, u64 off, u32 n) {
    if (off >= ip->size) {
        return 0;
    }
    if (n > ip->size - off) {
        n = (u32)(ip->size - off);
    }
    /* The block `off` is in, and where in it: `off` is below the size, so
     * its block number fits the extents' u32. Then block by block, in u32
     * arithmetic. */
    u32 bn = (u32)(off >> BSHIFT);
    u32 at = (u32)off & (BSIZE - 1);
    u8 *d = (u8 *)dst;
    for (u32 done = 0; done < n; bn++, at = 0) {
        u32 addr = ext_lookup(ip, bn);
        if (addr == 0) {
            panic("readi: a hole in a file");
        }
        struct buf *b = fs_bread(ip->dev, addr);
        u32 m = MIN(n - done, BSIZE - at);
        /* The caller's buffer is never the cache's: memcpy (which hands
         * differently aligned offsets to memmove's shifting copy). */
        memcpy(d, b->data + at, m);
        brelse(b);
        done += m;
        d += m;
    }
    return (int)n;
}

int writei(struct inode *ip, const void *src, u64 off, u32 n) {
    if (off > ip->size) {
        return -EINVAL;
    }
    /* off + n > MAXFILE_BYTES, without overflowing to find out. */
    if (off > MAXFILE_BYTES || n > MAXFILE_BYTES - off) {
        return -EFBIG;
    }
    const u8 *s = (const u8 *)src;
    u32 done = 0;
    /* The file's blocks: no holes, so exactly the ones its size needs. A
     * block at or past them is the next one, added to the tree. The size
     * is at most MAXFILE_BYTES, at most EXT_MAX_BLOCKS blocks (fs.h), so
     * the count fits a u32, and so does the block of every byte written. */
    u32 blocks = (u32)((ip->size + BSIZE - 1) >> BSHIFT);
    u32 bn = (u32)(off >> BSHIFT);
    u32 at = (u32)off & (BSIZE - 1);
    for (; done < n; bn++, at = 0) {
        u32 addr;
        if (bn < blocks) {
            addr = ext_lookup(ip, bn);
            if (addr == 0) {
                panic("writei: a hole in a file");
            }
        } else {
            addr = balloc(ip->dev, ext_goal(ip));
            if (addr == 0) {
                break; /* the disk is full */
            }
            if (ext_append(ip, bn, addr) < 0) {
                bfree(ip->dev, addr); /* no room for the tree's new nodes */
                break;
            }
            blocks++;
        }
        struct buf *b = fs_bread(ip->dev, addr);
        u32 m = MIN(n - done, BSIZE - at);
        memcpy(b->data + at, s, m);
        log_write(b);
        brelse(b);
        done += m;
        s += m;
    }
    if (off + done > ip->size) {
        ip->size = off + done;
    }
    /* The size, and the root of the tree, which may have changed. */
    iupdate(ip);
    if (done == 0 && n > 0) {
        return -ENOSPC;
    }
    return (int)done;
}

/* ---- Directories ---- */

int namecmp(const char *a, const char *b) {
    for (u32 i = 0; i < DIRSIZ; i++) {
        if (a[i] != b[i]) {
            return (u8)a[i] - (u8)b[i];
        }
        if (a[i] == 0) {
            return 0;
        }
    }
    return 0;
}

struct inode *dirlookup(struct inode *dp, const char *name, u32 *poff) {
    if (dp->type != T_DIR) {
        panic("dirlookup: not a directory");
    }
    struct dirent de;
    for (u32 off = 0; off < dp->size; off += sizeof de) {
        if (readi(dp, &de, off, sizeof de) != sizeof de) {
            panic("dirlookup: short read");
        }
        if (de.inum != 0 && namecmp(name, de.name) == 0) {
            if (poff != NULL) {
                *poff = off;
            }
            return iget(dp->dev, de.inum);
        }
    }
    return NULL;
}

int dirlink(struct inode *dp, const char *name, u32 inum) {
    struct inode *ip = dirlookup(dp, name, NULL);
    if (ip != NULL) {
        iput(ip);
        return -EEXIST;
    }
    /* The first free slot, or the end of the directory. */
    struct dirent de;
    u32 off;
    for (off = 0; off < dp->size; off += sizeof de) {
        if (readi(dp, &de, off, sizeof de) != sizeof de) {
            panic("dirlink: short read");
        }
        if (de.inum == 0) {
            break;
        }
    }
    memset(&de, 0, sizeof de);
    de.inum = (u16)inum;
    for (u32 i = 0; i < DIRSIZ && name[i] != 0; i++) {
        de.name[i] = name[i];
    }
    if (writei(dp, &de, off, sizeof de) != sizeof de) {
        return -ENOSPC;
    }
    return 0;
}

/* ---- Paths ---- */

/* Copies the next name of `path` to `name` (NUL-terminated) and returns the
 * path after it and the slashes that follow; NULL when no name is left.
 * Sets *too_long for a name of more than DIRSIZ bytes (then `name` holds
 * its first DIRSIZ).
 *
 *   skipelem("a/bb/c", name) = "bb/c", name = "a"
 *   skipelem("///a//bb", name) = "bb", name = "a"
 *   skipelem("a", name) = "", name = "a"
 *   skipelem("", name) = skipelem("////", name) = NULL */
static const char *skipelem(const char *path, char *name, bool *too_long) {
    while (*path == '/') {
        path++;
    }
    if (*path == 0) {
        return NULL;
    }
    const char *s = path;
    while (*path != '/' && *path != 0) {
        path++;
    }
    u32 len = (u32)(path - s);
    *too_long = len > DIRSIZ;
    if (len > DIRSIZ) {
        len = DIRSIZ;
    }
    memmove(name, s, len);
    name[len] = 0;
    while (*path == '/') {
        path++;
    }
    return path;
}

/* namei() and nameiparent(): walks `path` from the root, or from the
 * working directory if it doesn't start with `/`. With `parent`, stops one
 * name early and copies that name to `name`. */
static struct inode *namex(const char *path, bool parent, char *name, int *err) {
    /* The working directory comes with a reference of our own, so that
     * another thread's chdir() can't free it under us. */
    struct inode *ip = *path != '/' ? proc_cwd() : NULL;
    if (ip == NULL) {
        ip = iget(fs_dev, ROOTINO);
    }
    bool too_long;
    while ((path = skipelem(path, name, &too_long)) != NULL) {
        if (too_long) {
            iput(ip);
            *err = -ENAMETOOLONG;
            return NULL;
        }
        ilock(ip);
        if (ip->type != T_DIR) {
            iunlockput(ip);
            *err = -ENOTDIR;
            return NULL;
        }
        if (parent && *path == 0) {
            /* The last name: stop here. */
            iunlock(ip);
            return ip;
        }
        if (ip->nlink == 0 && namecmp(name, ".") != 0) {
            /* A removed directory (still some process's working directory):
             * it is empty, so only its `..` is left, and that may name a
             * parent removed and freed since, or an inode reused for
             * something else. Nothing is found in it, as on Linux. */
            iunlockput(ip);
            *err = -ENOENT;
            return NULL;
        }
        struct inode *next = dirlookup(ip, name, NULL);
        /* Unlock before taking `next` (which may be `ip` itself, for `.`,
         * or its parent, for `..`). */
        iunlockput(ip);
        if (next == NULL) {
            *err = -ENOENT;
            return NULL;
        }
        ip = next;
    }
    if (parent) {
        iput(ip);
        *err = -EINVAL;
        return NULL;
    }
    return ip;
}

struct inode *namei(const char *path, int *err) {
    char name[DIRSIZ + 1];
    return namex(path, false, name, err);
}

struct inode *nameiparent(const char *path, char *name, int *err) {
    return namex(path, true, name, err);
}

/* ---- Operations ---- */

/* Whether `name` is `.` or `..`. */
static bool is_dot(const char *name) {
    return namecmp(name, ".") == 0 || namecmp(name, "..") == 0;
}

struct inode *fs_create(const char *path, u16 type, u16 major, u16 minor, int *err) {
    if (fs_readonly) {
        *err = -EROFS;
        return NULL;
    }
    char name[DIRSIZ + 1];
    struct inode *dp = nameiparent(path, name, err);
    if (dp == NULL) {
        /* A path with no last name is the root, which exists. */
        if (*err == -EINVAL) {
            *err = -EEXIST;
        }
        return NULL;
    }
    ilock(dp);
    if (dp->nlink == 0) {
        /* Removed while still in use (as a working directory): an entry
         * added now would be lost with it. */
        iunlockput(dp);
        *err = -ENOENT;
        return NULL;
    }
    struct inode *ip = dirlookup(dp, name, NULL);
    if (ip != NULL) {
        iunlockput(dp);
        iput(ip);
        *err = -EEXIST;
        return NULL;
    }
    ip = ialloc(dp->dev, type);
    if (ip == NULL) {
        iunlockput(dp);
        *err = -ENOSPC;
        return NULL;
    }
    ilock(ip);
    ip->major = major;
    ip->minor = minor;
    /* Its entry in dp. */
    ip->nlink = 1;
    iupdate(ip);
    if (type == T_DIR) {
        /* `.` doesn't count as a link; `..` is one more for dp, below. */
        if (dirlink(ip, ".", ip->inum) < 0 || dirlink(ip, "..", dp->inum) < 0) {
            goto fail;
        }
    }
    if (dirlink(dp, name, ip->inum) < 0) {
        goto fail;
    }
    if (type == T_DIR) {
        dp->nlink++;
        iupdate(dp);
    }
    iunlockput(dp);
    return ip;

fail:
    /* No room for the entries: the new inode goes back (iput frees it, with
     * any block it got, which may end this operation and begin others:
     * itrunc). The directory first, as no sleep lock may be held then. */
    ip->nlink = 0;
    iupdate(ip);
    iunlockput(dp);
    iunlockput(ip);
    *err = -ENOSPC;
    return NULL;
}

/* Whether directory `dp` (locked) has no entries but `.` and `..`. */
static bool dir_empty(struct inode *dp) {
    struct dirent de;
    for (u32 off = 2 * sizeof de; off < dp->size; off += sizeof de) {
        if (readi(dp, &de, off, sizeof de) != sizeof de) {
            panic("dir_empty: short read");
        }
        if (de.inum != 0) {
            return false;
        }
    }
    return true;
}

int fs_unlink(const char *path) {
    if (fs_readonly) {
        return -EROFS;
    }
    char name[DIRSIZ + 1];
    int err;
    struct inode *dp = nameiparent(path, name, &err);
    if (dp == NULL) {
        return err;
    }
    ilock(dp);
    if (is_dot(name)) {
        iunlockput(dp);
        return -EINVAL;
    }
    u32 off;
    struct inode *ip = dirlookup(dp, name, &off);
    if (ip == NULL) {
        iunlockput(dp);
        return -ENOENT;
    }
    ilock(ip);
    if (ip->nlink < 1) {
        panic("fs_unlink: no links");
    }
    if (ip->type == T_DIR && !dir_empty(ip)) {
        iunlockput(ip);
        iunlockput(dp);
        return -ENOTEMPTY;
    }
    struct dirent de;
    memset(&de, 0, sizeof de);
    if (writei(dp, &de, off, sizeof de) != sizeof de) {
        panic("fs_unlink: writei");
    }
    if (ip->type == T_DIR) {
        /* Its `..` named dp. */
        dp->nlink--;
        iupdate(dp);
    }
    iunlockput(dp);
    ip->nlink--;
    iupdate(ip);
    iunlockput(ip);
    return 0;
}

int fs_link(const char *old, const char *new) {
    if (fs_readonly) {
        return -EROFS;
    }
    int err;
    struct inode *ip = namei(old, &err);
    if (ip == NULL) {
        return err;
    }
    /* Both paths first, as Linux's linkat does, before anything is
     * written: looking a path up may drop the last reference to a removed
     * directory, whose freeing ends the operation (itrunc), which would
     * commit a half-made link. */
    char name[DIRSIZ + 1];
    struct inode *dp = nameiparent(new, name, &err);
    if (dp == NULL) {
        iput(ip);
        return err == -EINVAL ? -EEXIST : err; /* `/`: the root */
    }
    /* A directory has one name, in its parent: its `..` says which. Found
     * before taking a second lock, so that only a directory and then a file
     * are ever locked together here, as fs_unlink() locks them (two
     * directories, in whichever order, could wait for each other). */
    ilock(ip);
    bool is_dir = ip->type == T_DIR;
    iunlock(ip);
    if (is_dir) {
        iput(dp);
        iput(ip);
        return -EPERM;
    }
    ilock(dp);
    ilock(ip);
    if (dp->nlink == 0 || ip->nlink == 0) {
        /* A directory removed meanwhile; a file whose last name went
         * meanwhile, which Linux refuses to link back too. */
        err = -ENOENT;
    } else {
        /* The link and the entry, in one transaction: a failure takes the
         * link back. */
        ip->nlink++;
        iupdate(ip);
        err = dirlink(dp, name, ip->inum);
        if (err < 0) {
            ip->nlink--;
            iupdate(ip);
        }
    }
    iunlock(ip);
    iunlockput(dp);
    iput(ip);
    return err;
}
