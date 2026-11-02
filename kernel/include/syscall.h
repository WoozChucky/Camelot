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

/* Call numbers. */
#define SYS_exit 1    /* exit(status): never returns */
#define SYS_write 2   /* write(fd, buf, len): bytes written */
#define SYS_read 3    /* read(fd, buf, len): bytes read, 0 at the end of input */
#define SYS_getpid 4  /* getpid(): the caller's pid */
#define SYS_yield 5   /* yield(): 0, after the other runnable processes had a turn */
#define SYS_sleep 6   /* sleep(ticks): 0, at least `ticks` timer ticks later */
#define SYS_spawn 7   /* spawn(name, argv): the new process's pid */
#define SYS_wait 8    /* wait(pid, status): the pid of the child that ended */
#define SYS_sbrk 9    /* sbrk(increment): the old end of the heap */
#define SYS_uptime 10 /* uptime(): timer ticks since boot */

/* Error numbers (Linux's values); a call returns minus one of them. */
#define ENOENT 2   /* spawn: no program by that name */
#define E2BIG 7    /* spawn: argument list too long */
#define ENOEXEC 8  /* spawn: the program's image is not a valid executable */
#define EBADF 9    /* read, write: not an open file descriptor */
#define ECHILD 10  /* wait: no such child */
#define EAGAIN 11  /* spawn: every process slot is taken */
#define ENOMEM 12  /* spawn, sbrk: out of memory, or the heap would reach the stack */
#define EFAULT 14  /* a pointer argument is not valid user memory */
#define EINVAL 22  /* an argument is out of range */
#define ENOSYS 38  /* no such system call */

/* File descriptors: there are no files yet, only the console. */
#define FD_STDIN 0
#define FD_STDOUT 1
#define FD_STDERR 2

/* spawn's limits: at most MAXARG arguments (argv[0] included), whose strings
 * take at most ARG_MAX bytes with their NULs. They go on the new process's
 * stack, above its first stack frame. */
#define MAXARG 16
#define ARG_MAX 512

/* Handles the ecall in `f` (from U-mode, sepc already past it): runs the
 * call, puts the result in the frame's a0. Unknown numbers return -ENOSYS. */
void syscall(struct trap_frame *f);

#endif
