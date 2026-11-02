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
 *           fs_create()/fs_unlink() for the system calls of #67
 *
 * Anything that changes the disk runs between begin_op() and end_op(), so
 * that a crash leaves either all of it or none of it. That includes iput(),
 * which frees an inode when its last link and last user are gone. */
#ifndef CAMELOT_FS_H
#define CAMELOT_FS_H

#include "bio.h"
#include "syscall.h"
#include "types.h"

#define FS_MAGIC 0x53464d43 /* "CMFS" */
#define FS_VERSION 1

#define ROOTINO 1 /* the root directory's inode; inode 0 is never used */
#define SUPERBLOCK_NO 1
#define LOGSTART 2

#define NDIRECT 12
#define NINDIRECT (BSIZE / 4)
#define MAXFILE (NDIRECT + NINDIRECT) /* blocks */
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

/* Error numbers the filesystem returns (minus them), Linux's like the ones
 * in syscall.h; they move there with the file system calls (#67). */
#define EIO 5           /* the disk failed */
#define EEXIST 17       /* the name is taken */
#define ENOTDIR 20      /* a name on the way is not a directory */
#define EISDIR 21       /* not for a directory */
#define EFBIG 27        /* past the largest file */
#define ENOSPC 28       /* no free block or inode */
#define EROFS 30        /* the disk is read-only */
#define ENAMETOOLONG 36 /* a name longer than DIRSIZ */
#define ENOTEMPTY 39    /* a directory with entries */

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

/* An inode on the disk: 64 bytes, 16 per block. */
struct dinode {
    u16 type;
    u16 major; /* devices only */
    u16 minor;
    u16 nlink;                /* entries naming it, `.` not counted */
    u32 size;                 /* bytes */
    u32 addrs[NDIRECT + 1];   /* direct blocks, then the indirect block */
};

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
 * stays in the cache while ref > 0. The fields from `type` on are valid
 * only while the inode is locked (ilock) and `valid`. */
struct inode {
    u32 dev;
    u32 inum;
    u32 ref;
    bool locked; /* a sleep lock, like a buffer's */
    bool valid;  /* the fields below hold the dinode */
    u16 type;
    u16 major;
    u16 minor;
    u16 nlink;
    u32 size;
    u32 addrs[NDIRECT + 1];
};

/* Inodes in the cache, as in xv6. iget() panics if every one is in use. */
#define NINODE 50

/* The mounted filesystem: its disk, superblock, and whether the disk is
 * read-only (then nothing writes to it). */
extern bool fs_mounted;
extern bool fs_readonly;
extern u32 fs_dev;
extern struct superblock fs_sb;

/* Mounts disk `dev` as the root: reads and checks the superblock (every
 * rule of the format document's superblock section), replays a committed
 * transaction from the log, checks the root is a directory. Returns 0, or:
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

/* Reads up to `n` bytes at `off` of locked inode `ip` into kernel memory
 * `dst`. Returns the bytes read: fewer at the end of the file, 0 at or past
 * it. */
int readi(struct inode *ip, void *dst, u32 off, u32 n);

/* Writes `n` bytes from kernel memory `src` at `off` of locked inode `ip`,
 * growing it as needed (`off` at most its size: no holes). Returns the bytes
 * written, fewer if the disk filled up; -ENOSPC if none could be, -EINVAL
 * for an `off` past the end, -EFBIG past the largest file. In a
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
 * root (there is no working directory yet: #67); NULL with *err = -ENOENT,
 * -ENOTDIR or -ENAMETOOLONG. */
struct inode *namei(const char *path, int *err);

/* The directory `path`'s last name is in, and that name (copied to `name`,
 * DIRSIZ + 1 bytes); NULL with *err as namei, or -EINVAL for a path with no
 * last name ("/"). */
struct inode *nameiparent(const char *path, char *name, int *err);

/* Makes `path` a new inode of `type` (T_FILE, T_DIR with its `.` and `..`,
 * or T_DEVICE with `major`/`minor`) and returns it locked, with a reference;
 * NULL with *err: -EEXIST, -ENOENT/-ENOTDIR/-ENAMETOOLONG (the path),
 * -ENOSPC, -EROFS. In a transaction. */
struct inode *fs_create(const char *path, u16 type, u16 major, u16 minor, int *err);

/* Removes the entry `path`: a file loses a link (and is freed once it has
 * none and nobody uses it), a directory must be empty. 0, -ENOENT,
 * -ENOTDIR, -ENAMETOOLONG, -EINVAL (`.`, `..` or `/`), -ENOTEMPTY or
 * -EROFS. In a transaction. */
int fs_unlink(const char *path);

#endif
