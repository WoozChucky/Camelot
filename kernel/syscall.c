/* The system calls (docs/camelot-syscalls.md). syscall() takes the call
 * number from a7 and the arguments from a0-a5 of the trap frame, and leaves
 * the result in its a0. A pointer argument is a user address: the kernel
 * never dereferences it, but checks it against the process's page table and
 * copies through the physical pages (copyin, copyout, copyinstr in vm.c), so
 * a bad pointer is an -EFAULT, never a fault in the kernel. The calls on
 * files and paths are in sysfile.c; running programs in exec.c; the
 * network's in net/net.c, and the sockets' in net/socket.c. */
#include "syscall.h"
#include "exec.h"
#include "kalloc.h"
#include "kprintf.h"
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
    acquire(&tickslock);
    u32 start = ticks;
    /* Unsigned differences, so the count may wrap. Every tick wakes the
     * sleepers (clock_tick in trap.c), which check again. */
    while (ticks - start < n && !proc_killed()) {
        sleep((const void *)&ticks, &tickslock);
    }
    release(&tickslock);
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

/* procs(buf, max): all of `buf`, `max` entries, must be writable, however
 * many are filled in. The table is built on the kernel stack (NPROC
 * entries, 256 bytes) and copied out. */
static int sys_procs(struct trap_frame *f) {
    pagetable_t pt = myproc()->pagetable;
    u32 buf = arg(f, 0);
    u32 max = arg(f, 1);
    if (max > USER_TOP / sizeof(struct procinfo) || !uvm_range_ok(pt, buf, max * sizeof(struct procinfo), PTE_W)) {
        return -EFAULT;
    }
    struct procinfo info[NPROC];
    int n = proc_list(info, max < NPROC ? (int)max : NPROC);
    u32 filled = (u32)n < max ? (u32)n : max;
    /* Checked above, but another thread may have unmapped it since. */
    if (copyout(pt, buf, info, filled * sizeof(struct procinfo)) != 0) {
        return -EFAULT;
    }
    return n;
}

/* harts(buf, max): as procs, with NCPU entries of 16 bytes. */
static int sys_harts(struct trap_frame *f) {
    pagetable_t pt = myproc()->pagetable;
    u32 buf = arg(f, 0);
    u32 max = arg(f, 1);
    if (max > USER_TOP / sizeof(struct hartinfo) || !uvm_range_ok(pt, buf, max * sizeof(struct hartinfo), PTE_W)) {
        return -EFAULT;
    }
    struct hartinfo info[NCPU];
    int n = proc_harts(info, max < NCPU ? (int)max : NCPU);
    u32 filled = (u32)n < max ? (u32)n : max;
    /* Checked above, but another thread may have unmapped it since. */
    if (copyout(pt, buf, info, filled * sizeof(struct hartinfo)) != 0) {
        return -EFAULT;
    }
    return n;
}

/* Whether call `num` goes into the network stack: netinfo to setsockopt,
 * numbered one after another. They run under net_lock (net.h). */
static bool is_net_call(u32 num) {
    return num >= SYS_netinfo && num <= SYS_setsockopt;
}

void syscall(struct trap_frame *f) {
    struct proc *p = myproc();
    int ret;
    u32 num = f->regs[REG_A7];
    /* Whatever the program has on its stack is at or above sp: map the stack
     * down to it, so that a buffer there the program hasn't touched yet (a
     * big local array passed to read()) is user memory like any other. A
     * thread whose stack is on the heap has sp outside the stack's range. */
    u32 sp = f->regs[REG_SP];
    if (sp >= USER_STACK_LIMIT && sp < p->stack_low && proc_grow_stack(sp) == -ENOMEM) {
        kprintf("pid %u (%s) killed: no memory to grow its stack to 0x%08x\n", p->pid, p->name, sp);
        proc_exit(-1);
    }
    bool net = is_net_call(num);
    if (net) {
        acquire(&net_lock);
    }
    switch (num) {
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
    case SYS_kill:
        ret = proc_kill_pid((int)arg(f, 0));
        break;
    case SYS_procs:
        ret = sys_procs(f);
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
    case SYS_socket:
        ret = sock_sys_socket(arg(f, 0), arg(f, 1), arg(f, 2));
        break;
    case SYS_bind:
        ret = sock_sys_bind(arg(f, 0), arg(f, 1));
        break;
    case SYS_listen:
        ret = sock_sys_listen(arg(f, 0), arg(f, 1));
        break;
    case SYS_accept:
        ret = sock_sys_accept(arg(f, 0), arg(f, 1));
        break;
    case SYS_connect:
        ret = sock_sys_connect(arg(f, 0), arg(f, 1));
        break;
    case SYS_send:
        ret = sock_sys_send(arg(f, 0), arg(f, 1), arg(f, 2));
        break;
    case SYS_recv:
        ret = sock_sys_recv(arg(f, 0), arg(f, 1), arg(f, 2));
        break;
    case SYS_sendto:
        ret = sock_sys_sendto(arg(f, 0), arg(f, 1), arg(f, 2), arg(f, 3));
        break;
    case SYS_recvfrom:
        ret = sock_sys_recvfrom(arg(f, 0), arg(f, 1), arg(f, 2), arg(f, 3));
        break;
    case SYS_shutdown:
        ret = sock_sys_shutdown(arg(f, 0), arg(f, 1));
        break;
    case SYS_getsockname:
        ret = sock_sys_name(arg(f, 0), arg(f, 1), false);
        break;
    case SYS_getpeername:
        ret = sock_sys_name(arg(f, 0), arg(f, 1), true);
        break;
    case SYS_setsockopt:
        ret = sock_sys_setsockopt(arg(f, 0), arg(f, 1), arg(f, 2), arg(f, 3), arg(f, 4));
        break;
    case SYS_harts:
        ret = sys_harts(f);
        break;
    case SYS_thread_create:
        ret = thread_create(arg(f, 0), arg(f, 1), arg(f, 2));
        break;
    case SYS_thread_exit:
        thread_exit((int)arg(f, 0));
        ret = 0; /* not reached */
        break;
    case SYS_thread_join:
        ret = thread_join((int)arg(f, 0), arg(f, 1));
        break;
    case SYS_gettid:
        ret = (int)mythread()->tid;
        break;
    case SYS_futex_wait:
        ret = futex_wait(arg(f, 0), arg(f, 1));
        break;
    case SYS_futex_wake:
        ret = futex_wake(arg(f, 0), (i32)arg(f, 1));
        break;
    default:
        ret = -ENOSYS;
        break;
    }
    if (net) {
        release(&net_lock);
        /* Closing the socket's file takes net_lock, if this was its last
         * reference (another thread closed the descriptor meanwhile). */
        struct thread *t = mythread();
        if (t->net_file != NULL) {
            struct file *file = t->net_file;
            t->net_file = NULL;
            fileclose(file);
        }
    }
    f->regs[REG_A0] = (u32)ret;
}
