/* Open files (file.h), after xv6's file.c: the kernel's table of them, and
 * read, write, lseek and fstat on each kind. Data moves between the disk
 * and user memory through a page of the kernel's: readi() and writei()
 * work on kernel memory, and the user range has been checked first, so a
 * bad buffer fails before anything happens. */
#include "file.h"
#include "console.h"
#include "fb.h"
#include "kalloc.h"
#include "log.h"
#include "memlayout.h"
#include "mmap.h"
#include "net.h"
#include "pipe.h"
#include "shm.h"
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
static isize nodev_read(pagetable_t pt, uintptr dst, usize n) {
    (void)pt;
    (void)dst;
    (void)n;
    return -ENODEV;
}

static isize nodev_write(pagetable_t pt, uintptr src, usize n) {
    (void)pt;
    (void)src;
    (void)n;
    return -ENODEV;
}

/* The drivers, by major number: 0 is none, CONSOLE the console (which has
 * no memory to map), FB the framebuffer (its memory, for mmap). */
const struct devsw devsw[NDEV] = {
    {nodev_read, nodev_write, NULL},
    {console_read, console_write, NULL},
    {fb_read, fb_write, fb_mmap},
};

bool device_has_driver(u32 major) {
    return major != 0 && major < NDEV;
}

/* The most a write puts in one transaction: a page, 4 blocks. A chunk
 * changes at most its 4 new blocks and the partly written one it starts
 * in, the new ones' bitmap blocks, the extent tree's rightmost leaf, the
 * inode, and one split of the tree (a new leaf has room for 83 extents
 * before the next): 1 + 4 + 4 + 1 + 1 + 11 = MAXOPBLOCKS
 * (docs/camelot-fs.md, "Limits on transactions"). Bigger writes take
 * several transactions: a crash part way leaves the file with a prefix of
 * the bytes, never a torn block. */
#define MAXWRITE (4 * BSIZE)

_Static_assert(MAXWRITE <= PGSIZE, "a write's chunk fits the bounce page");
_Static_assert(1 + 2 * (MAXWRITE / BSIZE) + 1 + 1 + 11 <= MAXOPBLOCKS, "a write's chunk fits an operation");

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
    } else if (closed.type == FD_SHM) {
        /* The object stays while a mapping holds it. */
        shm_put(closed.shm);
    } else if (closed.ip != NULL) {
        /* The last reference to an unlinked file frees it: a transaction. */
        begin_op();
        iput(closed.ip);
        end_op();
    }
}

struct file *file_shm(int *err) {
    struct file *f = filealloc();
    if (f == NULL) {
        *err = -ENFILE;
        return NULL;
    }
    struct shm_object *o = shm_create();
    if (o == NULL) {
        fileclose(f); /* nothing set yet: just gives the slot back */
        *err = -ENFILE;
        return NULL;
    }
    f->type = FD_SHM;
    f->shm = o;
    f->readable = true;
    f->writable = true;
    return f;
}

int filetruncate(struct file *f, off_t length) {
    if (f->type != FD_SHM || !f->writable || length < 0) {
        return -EINVAL;
    }
    return shm_truncate(f->shm, (u64)length);
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
static isize inode_read(struct file *f, pagetable_t pt, uintptr dst, usize n) {
    u8 *buf = kalloc();
    if (buf == NULL) {
        return -ENOMEM;
    }
    usize done = 0;
    ilock(f->ip);
    while (done < n) {
        u32 m = (u32)MIN(n - done, PGSIZE);
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
            return done > 0 ? (isize)done : -EFAULT;
        }
        f->off += (u32)r;
        done += (u32)r;
        if ((u32)r < m) {
            break;
        }
    }
    iunlock(f->ip);
    kfree(buf);
    return (isize)done;
}

/* Writes to `f`'s inode at its offset, MAXWRITE bytes per transaction. */
static isize inode_write(struct file *f, pagetable_t pt, uintptr src, usize n) {
    u8 *buf = kalloc();
    if (buf == NULL) {
        return -ENOMEM;
    }
    usize done = 0;
    int err = 0;
    /* The whole write, against a truncation (and another write) coming in
     * between its transactions. */
    acquiresleep(&f->ip->wlock);
    while (done < n) {
        u32 m = (u32)MIN(n - done, MAXWRITE);
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
    releasesleep(&f->ip->wlock);
    kfree(buf);
    return done > 0 || err == 0 ? (isize)done : err;
}

isize fileread(struct file *f, pagetable_t pt, uintptr dst, usize n) {
    if (!f->readable) {
        return -EBADF;
    }
    if (!uvm_range_ok(pt, dst, n, PTE_W)) {
        return -EFAULT;
    }
    /* A shared-memory object is read and written through a mapping. */
    if (f->type == FD_SHM) {
        return -EINVAL;
    }
    if (n > MAX_RW_COUNT) {
        n = MAX_RW_COUNT;
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

isize filewrite(struct file *f, pagetable_t pt, uintptr src, usize n) {
    if (!f->writable) {
        return -EBADF;
    }
    if (!uvm_range_ok(pt, src, n, PTE_R)) {
        return -EFAULT;
    }
    if (f->type == FD_SHM) {
        return -EINVAL;
    }
    if (n > MAX_RW_COUNT) {
        n = MAX_RW_COUNT;
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

off_t filelseek(struct file *f, off_t offset, u32 whence) {
    if (f->type != FD_INODE) {
        return -ESPIPE;
    }
    ilock(f->ip);
    off_t size = (off_t)f->ip->size;
    iunlock(f->ip);
    off_t base;
    if (whence == SEEK_SET) {
        base = 0;
    } else if (whence == SEEK_CUR) {
        base = (off_t)f->off;
    } else if (whence == SEEK_END) {
        base = size;
    } else {
        return -EINVAL;
    }
    /* The new offset, base + offset, must be in [0, size]. Files are at
     * most MAXFILE_BYTES, which an off_t holds (open refuses a bigger
     * one), and so is an offset, so base and size are both in
     * [0, MAXFILE_BYTES]; base may be past size, though (SEEK_CUR after
     * another descriptor truncated the file). Neither -base nor size - base
     * overflows, then, and the sum is only made once it is known to be in
     * range. */
    if (offset < -base || offset > size - base) {
        return -EINVAL;
    }
    f->off = (u64)(base + offset);
    return base + offset;
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
        /* Like a file's names: 0 once the other end is gone. */
        st->nlink = socket_hung_up(f->sock) ? 0 : 1;
        return;
    }
    if (f->type == FD_SHM) {
        /* A regular file to Linux's fstat too, with no inode number. */
        st->type = T_FILE;
        st->size = (off_t)shm_size(f->shm);
        st->nlink = 1;
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

int filemmap(struct file *f, u32 type, u32 prot, u64 offset, usize size, struct vm_area *a) {
    /* Linux's checks: a mapping reads the file, and a shared one that
     * writes, writes it. */
    if (!f->readable) {
        return -EACCES;
    }
    bool writes_file = type == MAP_SHARED || f->type == FD_DEVICE;
    if (writes_file && (prot & PROT_WRITE) != 0 && !f->writable) {
        return -EACCES;
    }
    if (f->type == FD_SHM) {
        /* The object's pages, shared: a private mapping would be a
         * copy-on-write one, which Camelot doesn't have. */
        if (type != MAP_SHARED) {
            return -EINVAL;
        }
        /* No page past the largest object can ever be: a mapping that
         * reaches there is refused, as Linux refuses one past a file's
         * largest size (file_mmap_ok()). It also keeps a mapping of an
         * object, like an anonymous one, a size the kernel can go over. */
        if (offset > SHM_MAX_SIZE || size > SHM_MAX_SIZE - offset) {
            return -EOVERFLOW;
        }
        a->kind = AREA_SHM;
        a->shm = f->shm;
        a->pgoff = offset / PGSIZE;
        a->may_write = f->writable;
        /* Taken now, while the descriptor keeps the object alive. */
        shm_map(f->shm);
        return 0;
    }
    if (f->type == FD_DEVICE && devsw[f->major].mmap != NULL) {
        /* A device's memory is the device's whatever the type: a private
         * mapping of it writes it too, as Linux's fbdev maps it. */
        int r = devsw[f->major].mmap(offset, size, &a->device);
        if (r != 0) {
            return r;
        }
        a->kind = AREA_DEVICE;
        a->may_write = f->writable;
        return 0;
    }
    return -ENODEV;
}
