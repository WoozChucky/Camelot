/* The system calls (docs/camelot-syscalls.md). syscall() takes the call
 * number from a7 and the arguments from a0-a5 of the trap frame, and leaves
 * the result in its a0. A pointer argument is a user address: the kernel
 * never dereferences it, but checks it against the process's page table and
 * copies through the physical pages (copyin, copyout, copyinstr in vm.c), so
 * a bad pointer is an -EFAULT, never a fault in the kernel. */
#include "syscall.h"
#include "console.h"
#include "kalloc.h"
#include "proc.h"
#include "string.h"
#include "vm.h"

/* The arguments as the process passed them. */
static u32 arg(struct trap_frame *f, int n) {
    return f->regs[REG_A0 + n];
}

static int sys_write(struct trap_frame *f) {
    u32 fd = arg(f, 0);
    if (fd != FD_STDOUT && fd != FD_STDERR) {
        return -EBADF;
    }
    return console_write(myproc()->pagetable, arg(f, 1), arg(f, 2));
}

static int sys_read(struct trap_frame *f) {
    if (arg(f, 0) != FD_STDIN) {
        return -EBADF;
    }
    return console_read(myproc()->pagetable, arg(f, 1), arg(f, 2));
}

static int sys_sleep(struct trap_frame *f) {
    u32 n = arg(f, 0);
    u32 start = ticks;
    /* Unsigned differences, so the count may wrap. Every tick wakes the
     * sleepers (clock_tick in trap.c), which check again. */
    while (ticks - start < n) {
        proc_sleep((const void *)&ticks);
    }
    return 0;
}

/* Copies spawn's argv (a user array of string pointers ending with a NULL)
 * into `args`. A NULL argv stands for { name }. */
static int copy_args(pagetable_t pt, u32 argv, const char *name, struct proc_args *args) {
    args->argc = 0;
    args->len = 0;
    if (argv == 0) {
        args->argc = 1;
        args->len = strlen(name) + 1;
        memcpy(args->strings, name, args->len);
        return 0;
    }
    for (;;) {
        u32 ptr;
        if (copyin(pt, &ptr, argv + 4 * args->argc, sizeof ptr) != 0) {
            return -EFAULT;
        }
        if (ptr == 0) {
            return 0;
        }
        if (args->argc == MAXARG || args->len == ARG_MAX) {
            return -E2BIG;
        }
        int n = copyinstr(pt, args->strings + args->len, ptr, ARG_MAX - args->len);
        if (n == -1) {
            return -EFAULT;
        }
        if (n < 0) {
            return -E2BIG;
        }
        args->len += (u32)n + 1;
        args->argc++;
    }
}

static int sys_spawn(struct trap_frame *f) {
    pagetable_t pt = myproc()->pagetable;
    /* A name longer than any program's can't be found, but is still read to
     * its end, so a bad pointer is reported as one. */
    char name[PROC_NAME_MAX + 2];
    int n = copyinstr(pt, name, arg(f, 0), sizeof name);
    if (n == -1) {
        return -EFAULT;
    }
    const struct user_program *prog = n < 0 ? NULL : user_program_find(name);
    /* Too big for the kernel stack: a page of its own while it's needed. */
    struct proc_args *args = kalloc();
    if (args == NULL) {
        return -ENOMEM;
    }
    int err = copy_args(pt, arg(f, 1), name, args);
    if (err == 0 && prog == NULL) {
        err = -ENOENT;
    }
    struct proc *child = NULL;
    if (err == 0) {
        child = proc_create(prog, args, &err);
    }
    kfree(args);
    return child != NULL ? (int)child->pid : err;
}

void syscall(struct trap_frame *f) {
    struct proc *p = myproc();
    int ret;
    switch (f->regs[REG_A7]) {
    case SYS_exit:
        proc_exit((int)arg(f, 0));
        ret = 0; /* not reached */
        break;
    case SYS_write:
        ret = sys_write(f);
        break;
    case SYS_read:
        ret = sys_read(f);
        break;
    case SYS_getpid:
        ret = (int)p->pid;
        break;
    case SYS_yield:
        yield();
        ret = 0;
        break;
    case SYS_sleep:
        ret = sys_sleep(f);
        break;
    case SYS_spawn:
        ret = sys_spawn(f);
        break;
    case SYS_wait:
        ret = proc_wait((int)arg(f, 0), arg(f, 1));
        break;
    case SYS_sbrk:
        ret = proc_sbrk((i32)arg(f, 0));
        break;
    case SYS_uptime:
        ret = (int)ticks;
        break;
    default:
        ret = -ENOSYS;
        break;
    }
    f->regs[REG_A0] = (u32)ret;
}
