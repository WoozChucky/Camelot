/* The Camelot system call ABI (docs/camelot-syscalls.md), Linux RISC-V
 * style: `ecall` from U-mode with the call number in a7 and the arguments in
 * a0-a5; the result comes back in a0, a negative value being an error, minus
 * one of the E* numbers below. Every other register is preserved.
 *
 * kernel/user/usys.s (the stubs) and kernel/user/user.h (the error numbers)
 * have their own copies of these numbers, and the document a table of them;
 * a test checks that all four agree. */
#ifndef CAMELOT_SYSCALL_H
#define CAMELOT_SYSCALL_H

#include "trap.h"
#include "types.h"

/* Call numbers. */
#define SYS_exit 1    /* exit(status): never returns */
#define SYS_write 2   /* write(fd, buf, len): bytes written */
#define SYS_read 3    /* read(fd, buf, len): bytes read, 0 at the end of input */
#define SYS_getpid 4  /* getpid(): the caller's pid */
#define SYS_yield 5   /* yield(): 0, after the other runnable processes had a turn */
#define SYS_sleep 6   /* sleep(ticks): 0, at least `ticks` timer ticks later */
#define SYS_spawn 7   /* spawn(path, argv): the new process's pid */
#define SYS_wait 8    /* wait(pid, status): the pid of the child that ended */
#define SYS_sbrk 9    /* sbrk(increment): the old end of the heap */
#define SYS_uptime 10 /* uptime(): timer ticks since boot */
#define SYS_open 11   /* open(path, flags): a new file descriptor */
#define SYS_close 12  /* close(fd): 0 */
#define SYS_lseek 13  /* lseek(fd, offset, whence): the new offset */
#define SYS_fstat 14  /* fstat(fd, st): 0, with *st filled in */
#define SYS_mkdir 15  /* mkdir(path): 0 */
#define SYS_unlink 16 /* unlink(path): 0 */
#define SYS_link 17   /* link(old, new): 0 */
#define SYS_chdir 18  /* chdir(path): 0 */
#define SYS_dup 19    /* dup(fd): a new descriptor for the same open file */
#define SYS_pipe 20   /* pipe(fds): 0, with fds[0] the read end and fds[1] the write end */
#define SYS_exec 21   /* exec(path, argv): does not return */

/* Error numbers (Linux's values); a call returns minus one of them. */
#define EPERM 1         /* link: not for a directory */
#define ENOENT 2        /* no such file or directory */
#define E2BIG 7         /* spawn, exec: argument list too long */
#define ENOEXEC 8       /* spawn, exec: the file is not a valid executable */
#define EBADF 9         /* not an open file descriptor, or not open for that */
#define ECHILD 10       /* wait: no such child */
#define EAGAIN 11       /* spawn: every process slot is taken */
#define ENOMEM 12       /* out of memory, or the heap would reach the stack */
#define EACCES 13       /* spawn, exec: not a regular file */
#define EFAULT 14       /* a pointer argument is not valid user memory */
#define EEXIST 17       /* the name is taken */
#define ENODEV 19       /* open: a device node with no driver */
#define ENOTDIR 20      /* a name on the way is not a directory */
#define EISDIR 21       /* open: a directory, for writing */
#define EINVAL 22       /* an argument is out of range */
#define ENFILE 23       /* every slot of the kernel's open file table is taken */
#define EMFILE 24       /* every descriptor of the process is taken */
#define EFBIG 27        /* write: past the largest file */
#define ENOSPC 28       /* no free block or inode on the disk */
#define ESPIPE 29       /* lseek: not on a pipe or a device */
#define EROFS 30        /* the filesystem is read-only */
#define EPIPE 32        /* write: a pipe nobody reads any more */
#define ENAMETOOLONG 36 /* a name longer than DIRSIZ, or a path longer than MAXPATH */
#define ENOSYS 38       /* no such system call */
#define ENOTEMPTY 39    /* unlink: a directory with entries */

/* The console's file descriptors, open in the first process (0 for
 * reading, 1 and 2 for writing) and passed on by spawn. */
#define FD_STDIN 0
#define FD_STDOUT 1
#define FD_STDERR 2

/* open's flags (Linux's values): one access mode, plus O_CREAT and
 * O_TRUNC. */
#define O_RDONLY 0x000
#define O_WRONLY 0x001
#define O_RDWR 0x002
#define O_ACCMODE 0x003
#define O_CREAT 0x040
#define O_TRUNC 0x200

/* lseek's `whence`. */
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2

/* The longest path a call takes, without its NUL. */
#define MAXPATH 127

/* What fstat stores: 16 bytes. `type` is the inode's (T_DIR, T_FILE or
 * T_DEVICE, fs.h) or T_PIPE; `ino` and `nlink` are 0 for a pipe and for the
 * console the first process got without opening a path, and a pipe's size
 * is the bytes waiting in it. */
struct stat {
    u32 ino;
    u16 type;
    u16 nlink;
    u32 size;
    u16 major;
    u16 minor;
};

#define T_PIPE 4

/* spawn's and exec's limits: at most MAXARG arguments (argv[0] included), whose strings
 * take at most ARG_MAX bytes with their NULs. They go on the new process's
 * stack, above its first stack frame. */
#define MAXARG 16
#define ARG_MAX 512

/* Handles the ecall in `f` (from U-mode, sepc already past it): runs the
 * call, puts the result in the frame's a0. Unknown numbers return -ENOSYS. */
void syscall(struct trap_frame *f);

#endif
