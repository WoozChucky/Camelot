/* The system calls (docs/camelot-syscalls.md). syscall() takes the call
 * number from a7 and the arguments from a0-a5 of the trap frame, and leaves
 * the result in its a0. A pointer argument is a user address: the kernel
 * never dereferences it, but checks it against the process's page table and
 * copies through the physical pages (copyin, copyout, copyinstr in vm.c), so
 * a bad pointer is an -EFAULT, never a fault in the kernel. The calls on
 * files and paths are in sysfile.c; running programs in exec.c; the
 * network's in net/net.c. */
#include "syscall.h"
#include "exec.h"
#include "kalloc.h"
#include "net.h"
#include "proc.h"
#include "string.h"
#include "sysfile.h"
#include "vm.h"

/* The arguments as the process passed them. */
static u32 arg(struct trap_frame *f, int n) {
    return f->regs[REG_A0 + n];
}

static int sys_sleep(struct trap_frame *f) {
    u32 n = arg(f, 0);
    u32 start = ticks;
    /* Unsigned differences, so the count may wrap. Every tick wakes the
     * sleepers (clock_tick in trap.c), which check again. */
    while (ticks - start < n) {
        sleep((const void *)&ticks);
    }
    return 0;
}

/* Copies spawn's or exec's argv (a user array of string pointers ending
 * with a NULL) into `args`. A NULL argv stands for { path }. */
static int copy_args(pagetable_t pt, u32 argv, const char *path, struct proc_args *args) {
    args->argc = 0;
    args->len = 0;
    if (argv == 0) {
        args->argc = 1;
        args->len = strlen(path) + 1;
        memcpy(args->strings, path, args->len);
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

/* spawn(path, argv) and exec(path, argv): the path and the arguments are
 * copied in, every pointer checked, before anything is looked up. */
static int sys_run(struct trap_frame *f, bool replace) {
    pagetable_t pt = myproc()->pagetable;
    char path[MAXPATH + 1];
    int err = fetch_path(pt, arg(f, 0), path);
    /* Too big for the kernel stack: a page of its own while it's needed. */
    struct proc_args *args = NULL;
    if (err != -EFAULT) {
        /* argv is checked even when the path is too long or empty, which
         * comes after a bad pointer among argv's. */
        args = kalloc();
        int arg_err = args == NULL ? -ENOMEM : copy_args(pt, arg(f, 1), err == 0 ? path : "", args);
        if (err == 0 || arg_err == -EFAULT) {
            err = arg_err;
        }
    }
    if (err == 0) {
        err = replace ? exec_replace(path, args) : exec_spawn(path, args);
    }
    if (args != NULL) {
        kfree(args);
    }
    return err;
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
        ret = sys_run(f, false);
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
    case SYS_open:
        ret = sys_open(f);
        break;
    case SYS_close:
        ret = sys_close(f);
        break;
    case SYS_lseek:
        ret = sys_lseek(f);
        break;
    case SYS_fstat:
        ret = sys_fstat(f);
        break;
    case SYS_mkdir:
        ret = sys_mkdir(f);
        break;
    case SYS_unlink:
        ret = sys_unlink(f);
        break;
    case SYS_link:
        ret = sys_link(f);
        break;
    case SYS_chdir:
        ret = sys_chdir(f);
        break;
    case SYS_dup:
        ret = sys_dup(f);
        break;
    case SYS_pipe:
        ret = sys_pipe(f);
        break;
    case SYS_exec:
        /* On success the frame is the new program's, and the result its
         * argc, which belongs in its a0. */
        ret = sys_run(f, true);
        break;
    case SYS_netinfo:
        ret = net_sys_info(arg(f, 0));
        break;
    case SYS_netconf:
        ret = net_sys_conf(arg(f, 0), arg(f, 1), arg(f, 2));
        break;
    case SYS_ping:
        ret = net_sys_ping(arg(f, 0), arg(f, 1), arg(f, 2));
        break;
    default:
        ret = -ENOSYS;
        break;
    }
    f->regs[REG_A0] = (u32)ret;
}
