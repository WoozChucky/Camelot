/* The Camelot filesystem (docs/camelot-fs.md; spec section 10, "Kernel
 * filesystem"), after xv6's fs.h and fs.c. The on-disk structures below are
 * the document's tables; crates/camelot-fs is the host's implementation of
 * the same format (mkfs, fsck), and the tests check one against the other.
 *
 * The layers, bottom up, each using the ones below:
 *
 *   bio.c   blocks, cached (bread/bwrite/brelse)
 *   log.c   transactions: begin_op() ... log_write(b) ... end_op()
 *   fs.c    the block allocator (bitmap), inodes (an in-memory cache of
 *           them, each with a sleep lock), file contents (readi/writei),
 *           directories (dirlookup/dirlink), paths (namei/nameiparent), and
 *           fs_create()/fs_unlink()/fs_link() for the file system calls
 *           (sysfile.c)
 *
 * Anything that changes the disk runs between begin_op() and end_op(), so
 * that a crash leaves either all of it or none of it. That includes iput(),
 * which frees an inode when its last link and last user are gone. */
#ifndef CAMELOT_FS_H
#define CAMELOT_FS_H

#include "bio.h"
#include "sleeplock.h"
#include "syscall.h"
#include "types.h"

#define FS_MAGIC 0x53464d43 /* "CMFS" */
#define FS_VERSION 2

#define ROOTINO 1 /* the root directory's inode; inode 0 is never used */
#define SUPERBLOCK_NO 1
#define LOGSTART 2

/* The largest file the kernel reads or writes, in bytes: its file offsets
 * are 32-bit (i32 in lseek) until off_t widens (#155). The format's own
 * limit is 2^32 blocks. A bigger file (only mkfs could make one) fails to
 * open with -EOVERFLOW, as Linux's open does for a large file without
 * O_LARGEFILE. */
#define MAXFILE_BYTES 0x7fffffffu
#define DIRSIZ 14

/* Inode types. */
#define T_FREE 0
#define T_DIR 1
#define T_FILE 2
#define T_DEVICE 3

/* The format's limits on the log (header included) and the inode count. */
#define LOG_MIN 2
#define LOG_MAX 128
#define INODES_MIN 2
#define INODES_MAX 65536

/* The filesystem's failures are minus the error numbers of syscall.h
 * (ENOENT, EEXIST, ENOTDIR, ENOSPC, ...), which the file system calls pass
 * on. EIO is only fs_mount()'s: once mounted, a disk error panics. */
#define EIO 5 /* the disk failed */

/* Block 1. */
struct superblock {
    u32 magic;
    u32 version;
    u32 size;       /* blocks in the filesystem */
    u32 nblocks;    /* data blocks */
    u32 ninodes;    /* inode slots, inode 0 included */
    u32 nlog;       /* log blocks, header included */
    u32 logstart;   /* the log header */
    u32 inodestart; /* the first inode block */
    u32 bmapstart;  /* the first bitmap block */
    u32 datastart;  /* the first data block */
};

/* Extent trees (docs/camelot-fs.md, "Extent trees"): ext4's format. A node
 * is a header and entries of 12 bytes: extents in a leaf (depth 0), index
 * entries above. The root is the inode's i_block (a header and 4 entries);
 * a tree block holds a header and 84. Block numbers are 48 bits, of which
 * the high 16 are always 0 (a filesystem is at most 2^31 blocks). */
#define EXT_MAGIC 0xf30a
#define EXT_ROOT_MAX 4
#define EXT_BLOCK_MAX ((BSIZE - sizeof(struct ext_header)) / sizeof(struct ext_extent))
#define EXT_MAX_DEPTH 5    /* ext4's EXT4_MAX_EXTENT_DEPTH */
#define EXT_MAX_LEN 32768  /* longer ee_len is ext4's "uninitialized" */

struct ext_header {
    u16 magic;
    u16 entries;
    u16 max;
    u16 depth;
    u32 generation; /* 0 */
};

/* Logical blocks `block` to `block` + `len` - 1 are disk blocks `start_lo`
 * on. */
struct ext_extent {
    u32 block;
    u16 len;
    u16 start_hi;
    u32 start_lo;
};

/* The logical blocks from `block` on are under the tree block `leaf_lo`. */
struct ext_index {
    u32 block;
    u32 leaf_lo;
    u16 leaf_hi;
    u16 unused;
};

/* A node: the root (EXT_ROOT_MAX entries) or the start of a tree block
 * (EXT_BLOCK_MAX). Entries are extents or index entries, both 12 bytes;
 * ext_index() reads one as the latter. */
struct ext_node {
    struct ext_header h;
    struct ext_extent e[EXT_ROOT_MAX];
};

/* inode flags: the inode's blocks are being freed (docs/camelot-fs.md,
 * "Truncation"). */
#define INODE_TRUNCATING 1

/* An inode on the disk: 128 bytes, 8 per block. The size is a u64, as
 * size and size_hi (Merlin has no long long yet). */
struct dinode {
    u16 type;
    u16 major; /* devices only */
    u16 minor;
    u16 nlink;               /* entries naming it, `.` not counted */
    u32 size;                /* bytes: the low half */
    u32 size_hi;             /* and the high half */
    struct ext_node root;    /* i_block: the extent tree's root (zeros for a device) */
    u32 flags;               /* INODE_TRUNCATING, or 0 */
    u8 reserved[48];         /* 0 */
};

_Static_assert(sizeof(struct ext_header) == 12 && sizeof(struct ext_extent) == 12 &&
                   sizeof(struct ext_index) == 12 && sizeof(struct ext_node) == 60,
               "ext4's node layout");
_Static_assert(sizeof(struct dinode) == 128, "an inode is 128 bytes");

/* Inodes per block, and the block holding inode i. */
#define IPB (BSIZE / sizeof(struct dinode))
#define IBLOCK(i, sb) ((i) / IPB + (sb).inodestart)

/* Bits per bitmap block, and the bitmap block for block b. */
#define BPB (BSIZE * 8)
#define BBLOCK(b, sb) ((b) / BPB + (sb).bmapstart)

/* A directory entry: 16 bytes. inum 0 is a free slot. The name is padded
 * with NULs; a DIRSIZ-byte name has none. */
struct dirent {
    u16 inum;
    char name[DIRSIZ];
};

/* An inode in memory: the kernel's copy of a dinode, shared by everyone
 * using that file. `ref` counts the users (iget/idup to iput); the copy
 * stays in the cache while ref > 0. The fields from `valid` on are valid
 * only while the inode is locked (ilock) and `valid`. The cache's spinlock
 * (fs.c) protects `dev`, `inum` and `ref`; the sleep lock the rest. */
struct inode {
    u32 dev;
    u32 inum;
    u32 ref;
    struct sleeplock lock; /* held from ilock() to iunlock() */
    /* Held by whoever changes the contents of a file across transactions,
     * a write() or a truncation, from before its first begin_op() to after
     * its last end_op() (Linux's i_rwsem): so a truncation can end its
     * transaction, let go of `lock` and begin another, with no writer
     * coming in between. Lock order: wlock, then begin_op(), then lock. */
    struct sleeplock wlock;
    bool valid; /* the fields below hold the dinode */
    u16 type;
    u16 major;
    u16 minor;
    u16 nlink;
    u32 size;
    u32 size_hi; /* not 0: bigger than the kernel handles (MAXFILE_BYTES) */
    struct ext_node root;
    u32 flags;
};

/* Inodes in the cache. iget() panics if every one is in use, so there are
 * enough for every open file, working directory and call in progress at
 * once (fs.c checks: NFILE + NPROC + NTHREAD * 3, as every thread may be
 * in a call). */
#define NINODE 120

/* The mounted filesystem: its disk, superblock, and whether it is
 * read-only (then nothing writes to it): because the disk is, or because
 * its log has fewer than MAXOPBLOCKS slots (`fs_log_too_small`), too few
 * for an operation. `fs_truncations` counts the truncations the mount
 * finished that a crash had interrupted, and `fs_orphans` the inodes it
 * freed that a crash had left allocated with no links. */
extern bool fs_mounted;
extern bool fs_readonly;
extern bool fs_log_too_small;
extern u32 fs_truncations;
extern u32 fs_orphans;
extern u32 fs_dev;
extern struct superblock fs_sb;

/* Sets up the inode cache's locks. kmain calls it at boot, before any
 * process can use a file. */
void fs_init(void);

/* Mounts disk `dev` as the root: reads and checks the superblock (every
 * rule of the format document's superblock section), replays a committed
 * transaction from the log, checks the root is a directory, finishes the
 * truncations a crash interrupted and frees the inodes left with no links
 * (unless the filesystem is read-only, which it is on a read-only disk or
 * with a log too small to write). Returns 0, or:
 * -ENOENT if the disk has no filesystem (another magic), -EINVAL if its
 * superblock or log header is broken, -EROFS if it is read-only and its log
 * needs replaying, -EIO if the superblock can't be read; *why then says
 * what is wrong. Must be called by a process, once. */
int fs_mount(u32 dev, const char **why);

/* bread() for the filesystem: panics if the read fails (the filesystem
 * trusts its disk once mounted). */
struct buf *fs_bread(u32 dev, u32 blockno);

/* Free data blocks (from the bitmap) and free inodes. */
void fs_count_free(u32 *blocks, u32 *inodes);

/* ---- Inodes ---- */

/* Allocates a free inode of `type`, zeroed, and returns it unlocked with a
 * reference; NULL when none is free. In a transaction. */
struct inode *ialloc(u32 dev, u16 type);

/* The in-memory inode for `inum`, with a reference; not locked, and not
 * read from the disk until ilock(). */
struct inode *iget(u32 dev, u32 inum);

/* Another reference to `ip`; returns it. */
struct inode *idup(struct inode *ip);

/* Locks `ip`, reading it from the disk if needed; unlocks it. */
void ilock(struct inode *ip);
void iunlock(struct inode *ip);

/* Drops a reference. If it was the last one and the inode has no links
 * left, frees its blocks and the inode itself. In a transaction. */
void iput(struct inode *ip);

/* iunlock() then iput(). */
void iunlockput(struct inode *ip);

/* Writes a locked inode's fields to its dinode. In a transaction. */
void iupdate(struct inode *ip);

/* Frees every block of locked inode `ip` and sets its size to 0 (open's
 * O_TRUNC, and iput() freeing an inode). In a transaction, which it may end
 * and begin again, as often as freeing the blocks takes (docs/camelot-fs.md,
 * "Truncation"): `ip` is unlocked in between, so the caller holds no other
 * sleep lock that an operation might wait for, and holds ip->wlock if
 * others can reach the inode (a file with links), so that no writer comes
 * in. A small inode, whose blocks fit one transaction, is freed in the
 * caller's alone. */
void itrunc(struct inode *ip);

/* Whether locked inode `ip` is bigger than the kernel handles
 * (MAXFILE_BYTES): open() refuses it with -EOVERFLOW. */
bool inode_too_big(const struct inode *ip);

/* Reads up to `n` bytes at `off` of locked inode `ip` into kernel memory
 * `dst`. Returns the bytes read: fewer at the end of the file, 0 at or past
 * it. */
int readi(struct inode *ip, void *dst, u32 off, u32 n);

/* Writes `n` bytes from kernel memory `src` at `off` of locked inode `ip`,
 * growing it as needed (`off` at most its size: no holes). Returns the bytes
 * written, fewer if the disk filled up; -ENOSPC if none could be, -EINVAL
 * for an `off` past the end, -EFBIG past MAXFILE_BYTES. In a
 * transaction, which must have room for the blocks (see log.h). */
int writei(struct inode *ip, const void *src, u32 off, u32 n);

/* ---- Directories and paths ---- */

/* Compares two names of at most DIRSIZ bytes, as strcmp. */
int namecmp(const char *a, const char *b);

/* Looks `name` up in locked directory `dp`. Returns its inode (unlocked,
 * with a reference) and, if `poff` isn't NULL, the entry's offset; NULL if
 * there is no such entry. */
struct inode *dirlookup(struct inode *dp, const char *name, u32 *poff);

/* Adds entry `name` → `inum` to locked directory `dp`, in the first free
 * slot or at the end. 0, -EEXIST or -ENOSPC. In a transaction. */
int dirlink(struct inode *dp, const char *name, u32 inum);

/* The inode `path` names (unlocked, with a reference), looked up from the
 * root if it starts with `/`, else from the current process's working
 * directory (the root for a kernel process, which has none); NULL with
 * *err = -ENOENT, -ENOTDIR or -ENAMETOOLONG. */
struct inode *namei(const char *path, int *err);

/* The directory `path`'s last name is in, and that name (copied to `name`,
 * DIRSIZ + 1 bytes); NULL with *err as namei, or -EINVAL for a path with no
 * last name ("/"). */
struct inode *nameiparent(const char *path, char *name, int *err);

/* Makes `path` a new inode of `type` (T_FILE, T_DIR with its `.` and `..`,
 * or T_DEVICE with `major`/`minor`) and returns it locked, with a reference;
 * NULL with *err: -EEXIST (`/` included), -ENOENT/-ENOTDIR/-ENAMETOOLONG
 * (the path; -ENOENT also for a directory that has been removed while still
 * in use, as a working directory), -ENOSPC, -EROFS. In a transaction. */
struct inode *fs_create(const char *path, u16 type, u16 major, u16 minor, int *err);

/* Removes the entry `path`: a file loses a link (and is freed once it has
 * none and nobody uses it), a directory must be empty. 0, -ENOENT,
 * -ENOTDIR, -ENAMETOOLONG, -EINVAL (`.`, `..` or `/`), -ENOTEMPTY or
 * -EROFS. In a transaction. */
int fs_unlink(const char *path);

/* Adds the entry `new` for the file `old` names, which gains a link. 0,
 * -ENOENT/-ENOTDIR/-ENAMETOOLONG (either path; -ENOENT for a directory to
 * put `new` in that has been removed, or a file whose last name was
 * removed meanwhile), -EPERM (`old` is a directory), -EEXIST, -ENOSPC,
 * -EROFS. In a transaction: both paths are looked up before anything is
 * written, so the link and the entry are one change. */
int fs_link(const char *old, const char *new);

#endif
