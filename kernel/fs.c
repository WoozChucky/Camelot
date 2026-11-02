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
u32 fs_orphans;
u32 fs_dev;
struct superblock fs_sb;

static struct inode itable[NINODE];
static struct spinlock itable_lock;

void fs_init(void) {
    initlock(&itable_lock, "itable");
    for (u32 i = 0; i < NINODE; i++) {
        initsleeplock(&itable[i].lock, "inode");
    }
}

/* iget() panics when the cache is full, so it must hold every inode that
 * can be in use at once: one per open file (an FD_INODE file, or a device
 * opened by its path, keeps its inode until closed), one per process's
 * working directory, and the few each process holds for a moment inside a
 * call, at most INODES_PER_CALL (link() keeps `old` while walking `new`'s
 * path, which holds a directory and the entry found in it). A user can then
 * run out of open files (-ENFILE), never out of inodes. */
#define INODES_PER_CALL 3
_Static_assert(NINODE >= NFILE + NPROC * (1 + INODES_PER_CALL), "the inode cache holds every user");

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

/* Frees the inodes a crash left allocated with no links. fs_unlink() takes
 * a file's last link in one transaction, and the last iput() frees it in
 * another, when whoever still used it (an open file, a working directory)
 * lets go; a crash
 * in between leaves it on the disk, unnamed (docs/camelot-fs.md, "Link
 * count"). Each is freed in a transaction of its own, by iput() as it would
 * have been. The root is never one: if its count is 0 the disk is broken,
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

/* Allocates a data block, zeroed; 0 if none is free. Scans the bitmap from
 * the start of the data region for the first clear bit. */
static u32 balloc(u32 dev) {
    for (u32 base = 0; base < fs_sb.size; base += BPB) {
        struct buf *b = fs_bread(dev, BBLOCK(base, fs_sb));
        for (u32 i = 0; i < BPB && base + i < fs_sb.size; i++) {
            u8 mask = (u8)(1 << (i % 8));
            if (base + i < fs_sb.datastart || (b->data[i / 8] & mask) != 0) {
                continue;
            }
            b->data[i / 8] |= mask;
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

/* ---- Inodes ---- */

struct inode *ialloc(u32 dev, u16 type) {
    for (u32 inum = 1; inum < fs_sb.ninodes; inum++) {
        struct buf *b = fs_bread(dev, IBLOCK(inum, fs_sb));
        struct dinode *dip = (struct dinode *)b->data + inum % IPB;
        if (dip->type == T_FREE) {
            memset(dip, 0, sizeof *dip);
            dip->type = type;
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
    dip->size = ip->size;
    memmove(dip->addrs, ip->addrs, sizeof ip->addrs);
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
        ip->size = dip->size;
        memmove(ip->addrs, dip->addrs, sizeof ip->addrs);
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

void itrunc(struct inode *ip) {
    for (u32 i = 0; i < NDIRECT; i++) {
        if (ip->addrs[i] != 0) {
            bfree(ip->dev, ip->addrs[i]);
            ip->addrs[i] = 0;
        }
    }
    if (ip->addrs[NDIRECT] != 0) {
        struct buf *b = fs_bread(ip->dev, ip->addrs[NDIRECT]);
        const u32 *a = (const u32 *)b->data;
        for (u32 i = 0; i < NINDIRECT; i++) {
            if (a[i] != 0) {
                bfree(ip->dev, a[i]);
            }
        }
        brelse(b);
        bfree(ip->dev, ip->addrs[NDIRECT]);
        ip->addrs[NDIRECT] = 0;
    }
    ip->size = 0;
    iupdate(ip);
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
        itrunc(ip);
        ip->type = T_FREE;
        iupdate(ip);
        ip->valid = false;
        releasesleep(&ip->lock);
        acquire(&itable_lock);
    }
    ip->ref--;
    release(&itable_lock);
}

void iunlockput(struct inode *ip) {
    iunlock(ip);
    iput(ip);
}

/* ---- Contents ---- */

/* The disk block holding block `bn` of locked inode `ip`. If it has none
 * and `alloc` is set, allocates it (and the indirect block, if needed);
 * returns 0 when there is no block, or no free block to allocate. */
static u32 bmap(struct inode *ip, u32 bn, bool alloc) {
    if (bn < NDIRECT) {
        if (ip->addrs[bn] == 0 && alloc) {
            ip->addrs[bn] = balloc(ip->dev);
        }
        return ip->addrs[bn];
    }
    bn -= NDIRECT;
    if (bn >= NINDIRECT) {
        panic("bmap: past the largest file");
    }
    bool new_indirect = false;
    if (ip->addrs[NDIRECT] == 0) {
        if (!alloc) {
            return 0;
        }
        ip->addrs[NDIRECT] = balloc(ip->dev);
        if (ip->addrs[NDIRECT] == 0) {
            return 0;
        }
        new_indirect = true;
    }
    struct buf *b = fs_bread(ip->dev, ip->addrs[NDIRECT]);
    u32 *a = (u32 *)b->data;
    u32 addr = a[bn];
    if (addr == 0 && alloc) {
        addr = balloc(ip->dev);
        if (addr != 0) {
            a[bn] = addr;
            log_write(b);
        }
    }
    brelse(b);
    if (addr == 0 && new_indirect) {
        /* The indirect block was got for this block, and there is none for
         * it (the disk is full): give the indirect block back. A file has
         * one exactly when it has more than NDIRECT blocks (no holes), and
         * the caller writes the inode with what bmap() leaves in addrs. */
        bfree(ip->dev, ip->addrs[NDIRECT]);
        ip->addrs[NDIRECT] = 0;
    }
    return addr;
}

int readi(struct inode *ip, void *dst, u32 off, u32 n) {
    if (off >= ip->size || off + n < off) {
        return 0;
    }
    if (n > ip->size - off) {
        n = ip->size - off;
    }
    u8 *d = (u8 *)dst;
    for (u32 done = 0; done < n;) {
        u32 addr = bmap(ip, off / BSIZE, false);
        if (addr == 0) {
            panic("readi: a hole in a file");
        }
        struct buf *b = fs_bread(ip->dev, addr);
        u32 m = MIN(n - done, BSIZE - off % BSIZE);
        memmove(d, b->data + off % BSIZE, m);
        brelse(b);
        done += m;
        off += m;
        d += m;
    }
    return (int)n;
}

int writei(struct inode *ip, const void *src, u32 off, u32 n) {
    if (off > ip->size || off + n < off) {
        return -EINVAL;
    }
    if (off + n > MAXFILE * BSIZE) {
        return -EFBIG;
    }
    const u8 *s = (const u8 *)src;
    u32 done = 0;
    while (done < n) {
        u32 addr = bmap(ip, off / BSIZE, true);
        if (addr == 0) {
            break; /* the disk is full */
        }
        struct buf *b = fs_bread(ip->dev, addr);
        u32 m = MIN(n - done, BSIZE - off % BSIZE);
        memmove(b->data + off % BSIZE, s, m);
        log_write(b);
        brelse(b);
        done += m;
        off += m;
        s += m;
    }
    if (off > ip->size) {
        ip->size = off;
    }
    /* Even if the size is the same, bmap() may have added blocks. */
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
     * any block it got). */
    ip->nlink = 0;
    iupdate(ip);
    iunlockput(ip);
    iunlockput(dp);
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
    ilock(ip);
    if (ip->type == T_DIR) {
        /* A directory has one name, in its parent: its `..` says which. */
        iunlockput(ip);
        return -EPERM;
    }
    /* The link first, then the entry, both in the caller's transaction; a
     * failure below takes the link back. */
    ip->nlink++;
    iupdate(ip);
    iunlock(ip);

    char name[DIRSIZ + 1];
    struct inode *dp = nameiparent(new, name, &err);
    if (dp == NULL) {
        if (err == -EINVAL) {
            err = -EEXIST; /* `/`: the root */
        }
        goto undo;
    }
    ilock(dp);
    if (dp->nlink == 0) {
        err = -ENOENT;
    } else {
        err = dirlink(dp, name, ip->inum);
    }
    iunlockput(dp);
    if (err < 0) {
        goto undo;
    }
    iput(ip);
    return 0;

undo:
    ilock(ip);
    ip->nlink--;
    iupdate(ip);
    iunlockput(ip);
    return err;
}
