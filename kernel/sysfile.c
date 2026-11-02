/* The file system calls (docs/camelot-syscalls.md), after xv6's sysfile.c:
 * descriptors (read, write, close, dup, lseek, fstat), pipes, and the calls
 * that take paths (open, mkdir, unlink, link, chdir).
 *
 * Every change to the disk happens inside one transaction (begin_op() to
 * end_op(), log.h), so a crash leaves all of a call or none of it: open's
 * creating and truncating, mkdir, unlink, link, and chdir (letting go of the
 * old directory may free it, if it was removed meanwhile). A write is the
 * exception: file.c splits a big one into several transactions. The order
 * of the checks is the document's: a bad descriptor first, then a bad
 * pointer, then everything else. */
#include "sysfile.h"
#include "file.h"
#include "fs.h"
#include "log.h"
#include "pipe.h"
#include "proc.h"
#include "string.h"
#include "syscall.h"

/* The arguments as the process passed them. */
static u32 arg(struct trap_frame *f, int n) {
    return f->regs[REG_A0 + n];
}

/* The open file behind descriptor `fd` of the current process, or NULL. */
static struct file *fd_file(u32 fd) {
    if (fd >= NOFILE) {
        return NULL;
    }
    return myproc()->ofile[fd];
}

/* Gives `f` the lowest free descriptor of the current process: the
 * descriptor, or -EMFILE when there is none. */
static int fd_alloc(struct file *f) {
    struct proc *p = myproc();
    for (u32 fd = 0; fd < NOFILE; fd++) {
        if (p->ofile[fd] == NULL) {
            p->ofile[fd] = f;
            return (int)fd;
        }
    }
    return -EMFILE;
}

/* Reads the rest of a user string, from `va` to its NUL, without keeping
 * it. Returns 0, or -1 if it runs into memory the process can't read. */
static int skip_string(pagetable_t pt, u32 va) {
    char chunk[64];
    int n;
    /* -2: `sizeof chunk` bytes read, none of them a NUL. */
    while ((n = copyinstr(pt, chunk, va, sizeof chunk)) == -2) {
        va += sizeof chunk;
    }
    return n == -1 ? -1 : 0;
}

int fetch_path(pagetable_t pt, u32 va, char *buf) {
    int n = copyinstr(pt, buf, va, MAXPATH + 1);
    if (n == -2) {
        return skip_string(pt, va + MAXPATH + 1) != 0 ? -EFAULT : -ENAMETOOLONG;
    }
    if (n == -1) {
        return -EFAULT;
    }
    return n == 0 ? -ENOENT : 0;
}

int sys_read(struct trap_frame *f) {
    struct file *file = fd_file(arg(f, 0));
    if (file == NULL) {
        return -EBADF;
    }
    return fileread(file, myproc()->pagetable, arg(f, 1), arg(f, 2));
}

int sys_write(struct trap_frame *f) {
    struct file *file = fd_file(arg(f, 0));
    if (file == NULL) {
        return -EBADF;
    }
    return filewrite(file, myproc()->pagetable, arg(f, 1), arg(f, 2));
}

int sys_close(struct trap_frame *f) {
    u32 fd = arg(f, 0);
    struct file *file = fd_file(fd);
    if (file == NULL) {
        return -EBADF;
    }
    myproc()->ofile[fd] = NULL;
    fileclose(file);
    return 0;
}

int sys_dup(struct trap_frame *f) {
    struct file *file = fd_file(arg(f, 0));
    if (file == NULL) {
        return -EBADF;
    }
    int fd = fd_alloc(file);
    if (fd >= 0) {
        filedup(file);
    }
    return fd;
}

int sys_lseek(struct trap_frame *f) {
    struct file *file = fd_file(arg(f, 0));
    if (file == NULL) {
        return -EBADF;
    }
    return filelseek(file, (i32)arg(f, 1), arg(f, 2));
}

int sys_fstat(struct trap_frame *f) {
    struct file *file = fd_file(arg(f, 0));
    if (file == NULL) {
        return -EBADF;
    }
    struct stat st;
    pagetable_t pt = myproc()->pagetable;
    if (!uvm_range_ok(pt, arg(f, 1), sizeof st, PTE_W)) {
        return -EFAULT;
    }
    filestat(file, &st);
    copyout(pt, arg(f, 1), &st, sizeof st);
    return 0;
}

int sys_pipe(struct trap_frame *f) {
    pagetable_t pt = myproc()->pagetable;
    u32 va = arg(f, 0);
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
    fds[0] = fd_alloc(rf);
    fds[1] = fds[0] >= 0 ? fd_alloc(wf) : -EMFILE;
    if (fds[1] < 0) {
        if (fds[0] >= 0) {
            myproc()->ofile[fds[0]] = NULL;
        }
        fileclose(rf);
        fileclose(wf);
        return -EMFILE;
    }
    copyout(pt, va, fds, sizeof fds);
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
        ip = fs_create(path, T_FILE, 0, 0, err);
        /* -EEXIST: made by another process while namei() slept on the
         * disk. Look it up again. */
        if (ip != NULL || *err != -EEXIST) {
            return ip;
        }
    }
}

int sys_open(struct trap_frame *f) {
    char path[MAXPATH + 1];
    int err = fetch_path(myproc()->pagetable, arg(f, 0), path);
    if (err < 0) {
        return err;
    }
    u32 flags = arg(f, 1);
    u32 mode = flags & O_ACCMODE;
    if ((flags & ~(O_ACCMODE | O_CREAT | O_TRUNC)) != 0 || mode == O_ACCMODE) {
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
    int fd = fd_alloc(file);
    if (fd < 0) {
        file->ref = 0; /* nothing set yet: just give the slot back */
        return fd;
    }
    begin_op();
    struct inode *ip = open_inode(path, flags, &err);
    if (ip != NULL) {
        err = 0; /* namei()'s -ENOENT, if fs_create() made it */
        if (ip->type == T_DIR && writable) {
            err = -EISDIR;
        } else if (ip->type == T_DEVICE && !device_has_driver(ip->major)) {
            err = -ENODEV;
        } else if (ip->type == T_FILE && writable && fs_readonly) {
            err = -EROFS;
        }
        if (err < 0) {
            iunlockput(ip);
        }
    }
    if (err < 0) {
        end_op();
        myproc()->ofile[fd] = NULL;
        file->ref = 0;
        return err;
    }
    file->type = ip->type == T_DEVICE ? FD_DEVICE : FD_INODE;
    file->major = ip->major;
    file->ip = ip;
    file->readable = readable;
    file->writable = writable;
    if ((flags & O_TRUNC) != 0 && writable && ip->type == T_FILE) {
        itrunc(ip);
    }
    iunlock(ip);
    end_op();
    return fd;
}

int sys_mkdir(struct trap_frame *f) {
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

int sys_unlink(struct trap_frame *f) {
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

int sys_link(struct trap_frame *f) {
    char old[MAXPATH + 1];
    char new[MAXPATH + 1];
    pagetable_t pt = myproc()->pagetable;
    int err = fetch_path(pt, arg(f, 0), old);
    if (err == 0) {
        err = fetch_path(pt, arg(f, 1), new);
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

int sys_chdir(struct trap_frame *f) {
    struct proc *p = myproc();
    char path[MAXPATH + 1];
    int err = fetch_path(p->pagetable, arg(f, 0), path);
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
    if (p->cwd != NULL) {
        iput(p->cwd);
    }
    p->cwd = ip;
    end_op();
    return 0;
}
