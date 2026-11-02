/* Open files (file.h), after xv6's file.c: the kernel's table of them, and
 * read, write, lseek and fstat on each kind. Data moves between the disk
 * and user memory through a page of the kernel's: readi() and writei()
 * work on kernel memory, and the user range has been checked first, so a
 * bad buffer fails before anything happens. */
#include "file.h"
#include "console.h"
#include "kalloc.h"
#include "log.h"
#include "memlayout.h"
#include "net.h"
#include "pipe.h"
#include "string.h"
#include "trap.h"

static struct file ftable[NFILE];

/* Protects every entry's `ref`, and so which entries are free. The rest of
 * an entry is set up by whoever took it before anyone else can reach it,
 * and stays as it is until the last close; the offset of an inode's file
 * changes under the inode's sleep lock (inode_read, inode_write). A leaf of
 * the lock order, but for the allocator's lock: fileclose() lets go of it
 * before closing what the file was open on. */
static struct spinlock ftable_lock;

void file_init(void) {
    initlock(&ftable_lock, "ftable");
}

/* Major 0 has no driver: device_has_driver() says so before anything is
 * opened, and these fail just in case. */
static int nodev_read(pagetable_t pt, u32 dst, u32 n) {
    (void)pt;
    (void)dst;
    (void)n;
    return -ENODEV;
}

static int nodev_write(pagetable_t pt, u32 src, u32 n) {
    (void)pt;
    (void)src;
    (void)n;
    return -ENODEV;
}

/* The drivers, by major number: 0 is none, CONSOLE the console. */
const struct devsw devsw[NDEV] = {
    {nodev_read, nodev_write},
    {console_read, console_write},
};

bool device_has_driver(u32 major) {
    return major != 0 && major < NDEV;
}

/* The most a write puts in one transaction: an operation may change
 * MAXOPBLOCKS blocks, and a write of this many bytes changes at most the
 * inode, the indirect block, and its data blocks (one more than it fills,
 * when it doesn't start on a block boundary) with as many bitmap blocks
 * (xv6's formula). Bigger writes take several transactions: a crash part
 * way leaves the file with a prefix of the bytes, never a torn block. */
#define MAXWRITE (((MAXOPBLOCKS - 1 - 1 - 2) / 2) * BSIZE)

_Static_assert(MAXWRITE >= BSIZE && MAXWRITE <= PGSIZE, "a write's chunk fits the bounce page");

#define MIN(a, b) ((a) < (b) ? (a) : (b))

struct file *filealloc(void) {
    acquire(&ftable_lock);
    for (u32 i = 0; i < NFILE; i++) {
        struct file *f = &ftable[i];
        if (f->ref == 0) {
            memset(f, 0, sizeof *f);
            f->ref = 1;
            release(&ftable_lock);
            return f;
        }
    }
    release(&ftable_lock);
    return NULL;
}

struct file *filedup(struct file *f) {
    acquire(&ftable_lock);
    if (f->ref < 1) {
        panic("filedup: a closed file");
    }
    f->ref++;
    release(&ftable_lock);
    return f;
}

void fileclose(struct file *f) {
    acquire(&ftable_lock);
    if (f->ref < 1) {
        panic("fileclose: a closed file");
    }
    f->ref--;
    if (f->ref > 0) {
        release(&ftable_lock);
        return;
    }
    /* The slot is free from here on, even while iput() below sleeps:
     * what it was open on is closed from a copy, without the lock. */
    struct file closed = *f;
    f->type = FD_NONE;
    release(&ftable_lock);
    if (closed.type == FD_PIPE) {
        pipeclose(closed.pipe, closed.writable);
    } else if (closed.type == FD_SOCKET) {
        socket_close(closed.sock);
    } else if (closed.ip != NULL) {
        /* The last reference to an unlinked file frees it: a transaction. */
        begin_op();
        iput(closed.ip);
        end_op();
    }
}

struct file *file_console(bool readable, bool writable) {
    struct file *f = filealloc();
    if (f != NULL) {
        f->type = FD_DEVICE;
        f->major = CONSOLE;
        f->readable = readable;
        f->writable = writable;
    }
    return f;
}

/* Reads from `f`'s inode at its offset, a page at a time. */
static int inode_read(struct file *f, pagetable_t pt, u32 dst, u32 n) {
    u8 *buf = kalloc();
    if (buf == NULL) {
        return -ENOMEM;
    }
    u32 done = 0;
    ilock(f->ip);
    while (done < n) {
        u32 m = MIN(n - done, PGSIZE);
        int r = readi(f->ip, buf, f->off, m);
        if (r <= 0) {
            break; /* the end of the file */
        }
        /* Checked by fileread(), but another thread may have unmapped the
         * buffer since (sbrk): the read stops short, the offset left after
         * what was copied. */
        if (copyout(pt, dst + done, buf, (u32)r) != 0) {
            iunlock(f->ip);
            kfree(buf);
            return done > 0 ? (int)done : -EFAULT;
        }
        f->off += (u32)r;
        done += (u32)r;
        if ((u32)r < m) {
            break;
        }
    }
    iunlock(f->ip);
    kfree(buf);
    return (int)done;
}

/* Writes to `f`'s inode at its offset, MAXWRITE bytes per transaction. */
static int inode_write(struct file *f, pagetable_t pt, u32 src, u32 n) {
    u8 *buf = kalloc();
    if (buf == NULL) {
        return -ENOMEM;
    }
    u32 done = 0;
    int err = 0;
    while (done < n) {
        u32 m = MIN(n - done, MAXWRITE);
        /* As in inode_read: never write a chunk that wasn't copied in (the
         * page holds whatever it held last). */
        if (copyin(pt, buf, src + done, m) != 0) {
            err = -EFAULT;
            break;
        }
        begin_op();
        ilock(f->ip);
        int r = writei(f->ip, buf, f->off, m);
        if (r > 0) {
            f->off += (u32)r;
        }
        iunlock(f->ip);
        end_op();
        if (r < 0) {
            err = r;
            break;
        }
        done += (u32)r;
        if ((u32)r < m) {
            break; /* the disk is full */
        }
    }
    kfree(buf);
    return done > 0 || err == 0 ? (int)done : err;
}

int fileread(struct file *f, pagetable_t pt, u32 dst, u32 n) {
    if (!f->readable) {
        return -EBADF;
    }
    if (!uvm_range_ok(pt, dst, n, PTE_W)) {
        return -EFAULT;
    }
    /* A socket's read is its recv, which for 0 bytes on UDP still takes
     * the next datagram. */
    if (f->type == FD_SOCKET) {
        return socket_read(f->sock, pt, dst, n);
    }
    if (n == 0) {
        return 0;
    }
    if (f->type == FD_PIPE) {
        return piperead(f->pipe, pt, dst, n);
    }
    if (f->type == FD_DEVICE) {
        return devsw[f->major].read(pt, dst, n);
    }
    return inode_read(f, pt, dst, n);
}

int filewrite(struct file *f, pagetable_t pt, u32 src, u32 n) {
    if (!f->writable) {
        return -EBADF;
    }
    if (!uvm_range_ok(pt, src, n, PTE_R)) {
        return -EFAULT;
    }
    /* A socket's write is its send, which for 0 bytes on UDP is still a
     * datagram (an empty one). */
    if (f->type == FD_SOCKET) {
        return socket_write(f->sock, pt, src, n);
    }
    if (n == 0) {
        return 0;
    }
    if (f->type == FD_PIPE) {
        return pipewrite(f->pipe, pt, src, n);
    }
    if (f->type == FD_DEVICE) {
        return devsw[f->major].write(pt, src, n);
    }
    return inode_write(f, pt, src, n);
}

int filelseek(struct file *f, i32 offset, u32 whence) {
    if (f->type != FD_INODE) {
        return -ESPIPE;
    }
    ilock(f->ip);
    u32 size = f->ip->size;
    iunlock(f->ip);
    i32 base;
    if (whence == SEEK_SET) {
        base = 0;
    } else if (whence == SEEK_CUR) {
        base = (i32)f->off;
    } else if (whence == SEEK_END) {
        base = (i32)size;
    } else {
        return -EINVAL;
    }
    /* Files are far smaller than 2^31 bytes (MAXFILE), so none of this
     * overflows unless `offset` is huge, which the checks below refuse. */
    if ((offset > 0 && base > 0x7fffffff - offset) || base + offset < 0 || (u32)(base + offset) > size) {
        return -EINVAL;
    }
    f->off = (u32)(base + offset);
    return (int)f->off;
}

void filestat(struct file *f, struct stat *st) {
    memset(st, 0, sizeof *st);
    if (f->type == FD_PIPE) {
        st->type = T_PIPE;
        st->size = pipe_bytes(f->pipe);
        return;
    }
    if (f->type == FD_SOCKET) {
        st->type = T_SOCKET;
        st->size = socket_bytes(f->sock);
        return;
    }
    if (f->ip == NULL) {
        /* The console of the first process, opened by the kernel. */
        st->type = T_DEVICE;
        st->major = f->major;
        return;
    }
    ilock(f->ip);
    st->ino = f->ip->inum;
    st->type = f->ip->type;
    st->nlink = f->ip->nlink;
    st->size = f->ip->size;
    st->major = f->ip->major;
    st->minor = f->ip->minor;
    iunlock(f->ip);
}
