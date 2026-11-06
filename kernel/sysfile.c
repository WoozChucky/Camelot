/* The file system calls (docs/camelot-syscalls.md), after xv6's sysfile.c:
 * descriptors (read, write, close, dup, fcntl, lseek, fstat), pipes, and the
 * calls that take paths (open, mkdir, unlink, link, chdir).
 *
 * Every change to the disk happens inside one transaction (begin_op() to
 * end_op(), log.h), so a crash leaves all of a call or none of it: open's
 * creating and truncating, mkdir, unlink, link, and chdir (letting go of the
 * old directory may free it, if it was removed meanwhile). A write is the
 * exception: file.c splits a big one into several transactions. The order
 * of the checks is the document's: a bad descriptor first, then a bad
 * pointer, then everything else.
 *
 * Another thread of the process may close a descriptor while a call uses
 * it: so each call takes its own reference to the open file (fd_get(),
 * proc.c) and lets go of it at the end, and the file stays open until
 * then. */
#include "sysfile.h"
#include "file.h"
#include "fs.h"
#include "kalloc.h"
#include "log.h"
#include "memlayout.h"
#include "pipe.h"
#include "proc.h"
#include "shm.h"
#include "string.h"
#include "syscall.h"

/* The arguments as the process passed them: a register each (syscall.c).
 * A descriptor or flags are an int, the register's low 32 bits. */
static uintptr arg(struct trap_frame *f, int n) {
    return f->regs[REG_A0 + n];
}

/* Reads the rest of a user string, from `va` to its NUL, without keeping
 * it. Returns 0, or -1 if it runs into memory the process can't read. */
static int skip_string(pagetable_t pt, uintptr va) {
    char chunk[64];
    int n;
    /* -2: `sizeof chunk` bytes read, none of them a NUL. */
    while ((n = copyinstr(pt, chunk, va, sizeof chunk)) == -2) {
        va += sizeof chunk;
    }
    return n == -1 ? -1 : 0;
}

int fetch_path(pagetable_t pt, uintptr va, char *buf) {
    int n = copyinstr(pt, buf, va, MAXPATH + 1);
    if (n == -2) {
        return skip_string(pt, va + MAXPATH + 1) != 0 ? -EFAULT : -ENAMETOOLONG;
    }
    if (n == -1) {
        return -EFAULT;
    }
    return n == 0 ? -ENOENT : 0;
}

long sys_read(struct trap_frame *f) {
    struct file *file = fd_get((u32)arg(f, 0));
    if (file == NULL) {
        return -EBADF;
    }
    isize r = fileread(file, myproc()->pagetable, arg(f, 1), arg(f, 2));
    fileclose(file);
    return r;
}

long sys_write(struct trap_frame *f) {
    struct file *file = fd_get((u32)arg(f, 0));
    if (file == NULL) {
        return -EBADF;
    }
    isize r = filewrite(file, myproc()->pagetable, arg(f, 1), arg(f, 2));
    fileclose(file);
    return r;
}

long sys_close(struct trap_frame *f) {
    struct file *file = fd_take((u32)arg(f, 0));
    if (file == NULL) {
        return -EBADF;
    }
    fileclose(file);
    return 0;
}

long sys_dup(struct trap_frame *f) {
    struct file *file = fd_get((u32)arg(f, 0));
    if (file == NULL) {
        return -EBADF;
    }
    /* Our reference becomes the new descriptor's. */
    int fd = fd_install(file);
    if (fd < 0) {
        fileclose(file);
    }
    return fd;
}

/* fcntl(fd, cmd, arg), Linux's for the descriptor's flags and copies: the
 * descriptor is checked first, as Linux does, then the command. */
long sys_fcntl(struct trap_frame *f) {
    u32 fd = (u32)arg(f, 0);
    u32 cmd = (u32)arg(f, 1);
    u32 a = (u32)arg(f, 2);
    struct file *file = fd_get(fd);
    if (file == NULL) {
        return -EBADF;
    }
    int r;
    switch (cmd) {
    case F_DUPFD:
    case F_DUPFD_CLOEXEC:
        /* No descriptor reaches `a`: Linux's answer past RLIMIT_NOFILE
         * (a negative int is a huge one there too). */
        if (a >= NOFILE) {
            r = -EINVAL;
            break;
        }
        r = fd_install_from(file, a, cmd == F_DUPFD_CLOEXEC);
        if (r >= 0) {
            return r; /* our reference is the new descriptor's */
        }
        break;
    case F_GETFD:
        r = fd_flags(fd);
        break;
    case F_SETFD:
        r = fd_set_flags(fd, a);
        break;
    case F_ADD_SEALS:
        /* A shared-memory object's, open for writing, the seals Camelot
         * has (no write seals). */
        if (file->type != FD_SHM || (a & ~(u32)(F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW)) != 0) {
            r = -EINVAL;
        } else if (!file->writable) {
            r = -EPERM;
        } else {
            r = shm_add_seals(file->shm, a);
        }
        break;
    case F_GET_SEALS:
        r = file->type == FD_SHM ? (int)shm_seals(file->shm) : -EINVAL;
        break;
    default:
        r = -EINVAL;
        break;
    }
    fileclose(file);
    return r;
}

long sys_lseek(struct trap_frame *f) {
    struct file *file = fd_get((u32)arg(f, 0));
    if (file == NULL) {
        return -EBADF;
    }
    off_t r = filelseek(file, (off_t)arg(f, 1), (u32)arg(f, 2));
    fileclose(file);
    return r;
}

long sys_fstat(struct trap_frame *f) {
    struct file *file = fd_get((u32)arg(f, 0));
    if (file == NULL) {
        return -EBADF;
    }
    struct stat st;
    pagetable_t pt = myproc()->pagetable;
    if (!uvm_range_ok(pt, arg(f, 1), sizeof st, PTE_W)) {
        fileclose(file);
        return -EFAULT;
    }
    filestat(file, &st);
    fileclose(file);
    /* Checked above, but another thread may have unmapped it since. */
    return copyout(pt, arg(f, 1), &st, sizeof st) == 0 ? 0 : -EFAULT;
}

long sys_stat(struct trap_frame *f) {
    pagetable_t pt = myproc()->pagetable;
    char path[MAXPATH + 1];
    int err = fetch_path(pt, arg(f, 0), path);
    if (err < 0) {
        return err;
    }
    struct stat st;
    if (!uvm_range_ok(pt, arg(f, 1), sizeof st, PTE_W)) {
        return -EFAULT;
    }
    if (!fs_mounted) {
        return -ENOENT;
    }
    /* What fstat() of the file opened would say, without opening it: a
     * socket's name or a device node without a driver too, which open()
     * refuses. A transaction for iput(), which frees an inode unlinked
     * meanwhile. */
    begin_op();
    struct inode *ip = namei(path, &err);
    if (ip == NULL) {
        end_op();
        return err;
    }
    ilock(ip);
    inode_stat(ip, &st);
    iunlockput(ip);
    end_op();
    /* Checked above, but another thread may have unmapped it since. */
    return copyout(pt, arg(f, 1), &st, sizeof st) == 0 ? 0 : -EFAULT;
}

long sys_getcwd(struct trap_frame *f) {
    /* Linux's getcwd: the path, built in a page (Linux's limit, longer
     * than MAXPATH: a directory reached by relative chdir()s may be deeper
     * than any path a call takes), then copied out with its NUL if it fits,
     * the length returned with the NUL counted. */
    pagetable_t pt = myproc()->pagetable;
    uintptr buf = arg(f, 0);
    u64 size = arg(f, 1);
    char *page = kalloc();
    if (page == NULL) {
        return -ENOMEM;
    }
    int at;
    if (!fs_mounted) {
        /* No disk: the root, where every process is. */
        at = PGSIZE - 2;
        page[at] = '/';
        page[at + 1] = 0;
    } else {
        /* A transaction for iput(), which frees an inode removed
         * meanwhile. */
        begin_op();
        at = fs_getcwd(page, PGSIZE);
        end_op();
    }
    long ret = at;
    if (at >= 0) {
        /* The path's length, its NUL counted. */
        u64 len = PGSIZE - (u64)at;
        if (len > size) {
            ret = -ERANGE;
        } else {
            ret = copyout(pt, buf, page + at, len) == 0 ? (long)len : -EFAULT;
        }
    }
    kfree(page);
    return ret;
}

long sys_memfd_create(struct trap_frame *f) {
    /* The name only names the object to someone debugging (Linux shows it
     * in /proc/PID/maps): it is checked as Linux checks it, and dropped. */
    pagetable_t pt = myproc()->pagetable;
    char name[MFD_NAME_MAX + 1];
    int n = copyinstr(pt, name, arg(f, 0), sizeof name);
    if (n == -1) {
        return -EFAULT;
    }
    if (n == -2) {
        /* Too long; read to its end all the same, so that a bad pointer is
         * reported as one. */
        return skip_string(pt, arg(f, 0) + sizeof name) != 0 ? -EFAULT : -EINVAL;
    }
    u32 flags = (u32)arg(f, 1);
    if ((flags & ~(u32)(MFD_CLOEXEC | MFD_ALLOW_SEALING)) != 0) {
        return -EINVAL;
    }
    if (!fd_available()) {
        return -EMFILE;
    }
    int err;
    struct file *file = file_shm((flags & MFD_ALLOW_SEALING) != 0, &err);
    if (file == NULL) {
        return err;
    }
    int fd = fd_install_from(file, 0, (flags & MFD_CLOEXEC) != 0);
    if (fd < 0) {
        fileclose(file);
    }
    return fd;
}

long sys_ftruncate(struct trap_frame *f) {
    struct file *file = fd_get((u32)arg(f, 0));
    if (file == NULL) {
        return -EBADF;
    }
    int r = filetruncate(file, (off_t)arg(f, 1));
    fileclose(file);
    return r;
}

/* ioctl(fd, request, arg): Linux's request numbers, an unsigned int (the
 * upper half of the register ignored, as Linux's do_vfs_ioctl() takes
 * it), and `arg` a user address or a value, as the request says. */
long sys_ioctl(struct trap_frame *f) {
    struct file *file = fd_get((u32)arg(f, 0));
    if (file == NULL) {
        return -EBADF;
    }
    long r = fileioctl(file, (u32)arg(f, 1), arg(f, 2));
    fileclose(file);
    return r;
}

long sys_pipe(struct trap_frame *f) {
    pagetable_t pt = myproc()->pagetable;
    uintptr va = arg(f, 0);
    int fds[2];
    if (!uvm_range_ok(pt, va, sizeof fds, PTE_W)) {
        return -EFAULT;
    }
    struct file *rf;
    struct file *wf;
    int err = pipealloc(&rf, &wf);
    if (err < 0) {
        return err;
    }
    if (fd_install2(rf, wf, fds) < 0) {
        fileclose(rf);
        fileclose(wf);
        return -EMFILE;
    }
    /* Checked above, but another thread may have unmapped it since: then
     * the caller never learns the descriptors, so they are taken back
     * (those another thread hasn't closed or replaced meanwhile). */
    if (copyout(pt, va, fds, sizeof fds) != 0) {
        if (fd_take_if(fds[0], rf)) {
            fileclose(rf);
        }
        if (fd_take_if(fds[1], wf)) {
            fileclose(wf);
        }
        return -EFAULT;
    }
    return 0;
}

/* The inode `path` names, locked, opening it for open(): made if it is
 * missing and `flags` has O_CREAT. In the caller's transaction. NULL with
 * *err. */
static struct inode *open_inode(const char *path, u32 flags, int *err) {
    for (;;) {
        struct inode *ip = namei(path, err);
        if (ip != NULL) {
            ilock(ip);
            return ip;
        }
        if (*err != -ENOENT || (flags & O_CREAT) == 0) {
            return NULL;
        }
        if (path[strlen(path) - 1] == '/') {
            /* "name/" says a directory, and open() makes only files (as on
             * Linux). The path isn't empty: fetch_path() refused that. */
            *err = -EISDIR;
            return NULL;
        }
        ip = fs_create(path, T_FILE, 0, 0, err);
        /* -EEXIST: made by another process while namei() slept on the
         * disk. Look it up again. */
        if (ip != NULL || *err != -EEXIST) {
            return ip;
        }
    }
}

long sys_open(struct trap_frame *f) {
    char path[MAXPATH + 1];
    int err = fetch_path(myproc()->pagetable, arg(f, 0), path);
    if (err < 0) {
        return err;
    }
    u32 flags = (u32)arg(f, 1);
    u32 mode = flags & O_ACCMODE;
    if ((flags & ~(O_ACCMODE | O_CREAT | O_TRUNC | O_CLOEXEC)) != 0 || mode == O_ACCMODE) {
        return -EINVAL;
    }
    if (!fs_mounted) {
        return -ENOENT;
    }
    bool readable = mode != O_WRONLY;
    bool writable = mode != O_RDONLY;
    /* The file and the descriptor first: with none free, nothing is made. */
    struct file *file = filealloc();
    if (file == NULL) {
        return -ENFILE;
    }
    if (!fd_available()) {
        fileclose(file); /* nothing set yet: just gives the slot back */
        return -EMFILE;
    }
    begin_op();
    struct inode *ip = open_inode(path, flags, &err);
    if (ip != NULL) {
        err = 0; /* namei()'s -ENOENT, if fs_create() made it */
        if (ip->type == T_DIR && writable) {
            err = -EISDIR;
        } else if (ip->type == T_SOCKET) {
            err = -ENXIO; /* a socket's name is for connect(), as on Linux */
        } else if (ip->type == T_DEVICE && !device_has_driver(ip->major)) {
            err = -ENODEV;
        } else if (ip->type == T_FILE && writable && fs_readonly) {
            err = -EROFS;
        } else if (inode_too_big(ip)) {
            err = -EOVERFLOW;
        }
        if (err < 0) {
            iunlockput(ip);
        }
    }
    if (err < 0) {
        end_op();
        fileclose(file);
        return err;
    }
    file->type = ip->type == T_DEVICE ? FD_DEVICE : FD_INODE;
    file->major = ip->major;
    file->minor = ip->minor;
    file->ip = ip;
    file->readable = readable;
    file->writable = writable;
    iunlock(ip);
    end_op();
    /* A device's driver opens it (an input device's reader, say), outside
     * the transaction: it may sleep, or refuse. A refused file goes
     * without the driver's close, as Linux's does. */
    if (file->type == FD_DEVICE) {
        int r = device_open(file);
        if (r < 0) {
            file->type = FD_NONE;
            fileclose(file);
            return r;
        }
    }
    /* Truncated in transactions of its own: freeing a big file's blocks
     * takes several (itrunc), under the file's write lock, which comes
     * before begin_op(). Writers wait for it; a reader meanwhile sees the
     * file empty already. */
    if ((flags & O_TRUNC) != 0 && writable && ip->type == T_FILE) {
        acquiresleep(&ip->wlock);
        begin_op();
        ilock(ip);
        itrunc(ip);
        iunlock(ip);
        end_op();
        releasesleep(&ip->wlock);
    }
    /* The descriptor last, once the file is whole: no other thread can use
     * it half set up. It fails only if another thread took the last free
     * one meanwhile. */
    int fd = fd_install_from(file, 0, (flags & O_CLOEXEC) != 0);
    if (fd < 0) {
        fileclose(file);
    }
    return fd;
}

long sys_mkdir(struct trap_frame *f) {
    char path[MAXPATH + 1];
    int err = fetch_path(myproc()->pagetable, arg(f, 0), path);
    if (err < 0) {
        return err;
    }
    if (!fs_mounted) {
        return -ENOENT;
    }
    begin_op();
    struct inode *ip = fs_create(path, T_DIR, 0, 0, &err);
    if (ip != NULL) {
        iunlockput(ip);
    }
    end_op();
    return ip != NULL ? 0 : err;
}

long sys_unlink(struct trap_frame *f) {
    char path[MAXPATH + 1];
    int err = fetch_path(myproc()->pagetable, arg(f, 0), path);
    if (err < 0) {
        return err;
    }
    if (!fs_mounted) {
        return -ENOENT;
    }
    begin_op();
    err = fs_unlink(path);
    end_op();
    return err;
}

long sys_link(struct trap_frame *f) {
    char old[MAXPATH + 1];
    char new[MAXPATH + 1];
    pagetable_t pt = myproc()->pagetable;
    /* Both paths are read before either's error is returned: a bad pointer
     * in `new` comes before `old` being too long or empty. */
    int err = fetch_path(pt, arg(f, 0), old);
    int err_new = fetch_path(pt, arg(f, 1), new);
    if (err == -EFAULT || err_new == -EFAULT) {
        return -EFAULT;
    }
    if (err == 0) {
        err = err_new;
    }
    if (err < 0) {
        return err;
    }
    if (!fs_mounted) {
        return -ENOENT;
    }
    begin_op();
    err = fs_link(old, new);
    end_op();
    return err;
}

long sys_chdir(struct trap_frame *f) {
    char path[MAXPATH + 1];
    int err = fetch_path(myproc()->pagetable, arg(f, 0), path);
    if (err < 0) {
        return err;
    }
    if (!fs_mounted) {
        return -ENOENT;
    }
    begin_op();
    struct inode *ip = namei(path, &err);
    if (ip == NULL) {
        end_op();
        return err;
    }
    ilock(ip);
    if (ip->type != T_DIR) {
        iunlockput(ip);
        end_op();
        return -ENOTDIR;
    }
    iunlock(ip);
    struct inode *old = proc_set_cwd(ip);
    if (old != NULL) {
        iput(old);
    }
    end_op();
    return 0;
}
