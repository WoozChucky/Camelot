/* The system calls (docs/camelot-syscalls.md). syscall() takes the call
 * number from a7 and the arguments from a0-a5 of the trap frame, and leaves
 * the result in its a0: a `long`, sign-extended to the register's width, as
 * Linux's riscv64 syscall handler stores regs->a0. Each call takes its
 * arguments at their C types (an int is the register's low 32 bits, as the
 * calling convention passes it; a pointer or a size all of it). A pointer
 * argument is a user address: the kernel
 * never dereferences it, but checks it against the process's page table and
 * copies through the physical pages (copyin, copyout, copyinstr in vm.c), so
 * a bad pointer is an -EFAULT, never a fault in the kernel. The calls on
 * files and paths are in sysfile.c; running programs in exec.c; the
 * network's in net/net.c, and the sockets' in net/socket.c. */
#include "syscall.h"
#include "exec.h"
#include "kalloc.h"
#include "kprintf.h"
#include "mmap.h"
#include "net.h"
#include "proc.h"
#include "string.h"
#include "sysfile.h"
#include "vm.h"

/* The arguments as the process passed them: a register each. */
static uintptr arg(struct trap_frame *f, int n) {
    return f->regs[REG_A0 + n];
}

static long sys_sleep(struct trap_frame *f) {
    u32 n = (u32)arg(f, 0);
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

/* spawn's and exec's argv and envp are taken as Linux's execve takes them
 * (do_execveat_common in fs/exec.c), in two passes. First both arrays are
 * counted, argv's then envp's: every pointer is read, none of the strings
 * (count()). Then the strings are copied, envp's then argv's, each array's
 * from its last string to its first, each one below the ones before it
 * (copy_strings()): so of two errors, the call's is the first one found in
 * that order, as on Linux. */

/* How many strings the user array `array` holds, in *count: string
 * pointers, 4 or 8 bytes each, up to a NULL. As Linux's count(): -EFAULT
 * if a pointer can't be read, -E2BIG if there are more than MAXARG
 * (MAX_ARG_STRINGS on Linux). A NULL array holds none. */
static int count_strings(pagetable_t pt, uintptr array, u32 *count) {
    *count = 0;
    if (array == 0) {
        return 0;
    }
    for (;;) {
        uintptr ptr;
        if (copyin(pt, &ptr, array + sizeof ptr * *count, sizeof ptr) != 0) {
            return -EFAULT;
        }
        if (ptr == 0) {
            return 0;
        }
        if (*count == MAXARG) {
            return -E2BIG;
        }
        (*count)++;
    }
}

/* Copies the first `count` strings of user array `array` into `args`,
 * below the strings it holds already, as Linux's copy_strings() copies
 * them: from the last to the first, each measured (strnlen_user) and then
 * put right below the one before, so that they lie in their order, the
 * first lowest. A pointer or a string that can't be read is -EFAULT; a
 * string that doesn't fit in what is left of ARG_MAX is -E2BIG (so none is
 * longer than that, Linux's MAX_ARG_STRLEN). */
static int copy_strings(pagetable_t pt, uintptr array, u32 count, struct proc_args *args) {
    while (count-- > 0) {
        /* Read again: another thread may have changed it since it was
         * counted. A NULL now is a string at 0, which can't be read. */
        uintptr ptr;
        if (copyin(pt, &ptr, array + sizeof ptr * count, sizeof ptr) != 0) {
            return -EFAULT;
        }
        u32 len = strnlen_user(pt, ptr, ARG_MAX);
        if (len == 0) {
            return -EFAULT;
        }
        if (len > ARG_MAX - args->len) {
            return -E2BIG;
        }
        char *to = proc_args_push(args, len);
        if (copyin(pt, to, ptr, len) != 0) {
            return -EFAULT; /* unmapped since it was measured */
        }
        /* Or changed: its last byte is its NUL, whatever it holds now, so
         * that the strings stay `count` (push_start walks them). */
        to[len - 1] = 0;
    }
    return 0;
}

/* Copies spawn's or exec's argv and envp into `args` (above). A NULL argv
 * stands for { path }, put in where argv's strings would go; a NULL envp
 * is an empty environment, as Linux takes it. */
static int copy_args(pagetable_t pt, uintptr argv, uintptr envp, const char *path, struct proc_args *args) {
    args->len = 0;
    int err = count_strings(pt, argv, &args->argc);
    if (err == 0) {
        err = count_strings(pt, envp, &args->envc);
    }
    if (err == 0) {
        err = copy_strings(pt, envp, args->envc, args);
    }
    if (err != 0) {
        return err;
    }
    if (argv != 0) {
        return copy_strings(pt, argv, args->argc, args);
    }
    u32 len = (u32)strlen(path) + 1;
    if (len > ARG_MAX - args->len) {
        return -E2BIG;
    }
    memcpy(proc_args_push(args, len), path, len);
    args->argc = 1;
    return 0;
}

_Static_assert(sizeof(struct proc_args) <= PGSIZE, "spawn's arguments fit a page");

/* spawn(path, argv, envp) and exec(path, argv, envp): the path, the
 * arguments and the environment are copied in, every pointer checked,
 * before anything is looked up. */
static long sys_run(struct trap_frame *f, bool replace) {
    pagetable_t pt = myproc()->pagetable;
    char path[MAXPATH + 1];
    int err = fetch_path(pt, arg(f, 0), path);
    /* Too big for the kernel stack: a page of its own while it's needed. */
    struct proc_args *args = NULL;
    if (err != -EFAULT) {
        /* argv and envp are checked even when the path is too long or
         * empty, which comes after a bad pointer among them. */
        args = kalloc();
        int arg_err = args == NULL ? -ENOMEM : copy_args(pt, arg(f, 1), arg(f, 2), err == 0 ? path : "", args);
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
static long sys_procs(struct trap_frame *f) {
    pagetable_t pt = myproc()->pagetable;
    uintptr buf = arg(f, 0);
    u32 max = (u32)arg(f, 1);
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
static long sys_harts(struct trap_frame *f) {
    pagetable_t pt = myproc()->pagetable;
    uintptr buf = arg(f, 0);
    u32 max = (u32)arg(f, 1);
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
static bool is_net_call(uintptr num) {
    return num >= SYS_netinfo && num <= SYS_setsockopt;
}

void syscall(struct trap_frame *f) {
    struct proc *p = myproc();
    long ret;
    /* The number is all of a7, as Linux takes it (a long): 2^32 + 2 is no
     * call, not write. */
    uintptr num = f->regs[REG_A7];
    /* Whatever the program has on its stack is at or above sp: map the stack
     * down to it, so that a buffer there the program hasn't touched yet (a
     * big local array passed to read()) is user memory like any other. A
     * thread whose stack is on the heap has sp outside the stack's range. */
    uintptr sp = f->regs[REG_SP];
    if (sp >= USER_STACK_LIMIT && sp < p->stack_low && proc_grow_stack(sp) == -ENOMEM) {
        kprintf("pid %u (%s) killed: no memory to grow its stack to " REG_FMT "\n", p->pid, p->name, sp);
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
        ret = proc_sbrk((long)arg(f, 0));
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
        /* On success the frame is the new program's, and the result 0,
         * which belongs in its a0 (proc.c, "The program's start"). */
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
        ret = net_sys_conf((u32)arg(f, 0), (u32)arg(f, 1), (u32)arg(f, 2));
        break;
    case SYS_ping:
        ret = net_sys_ping((u32)arg(f, 0), (u32)arg(f, 1), arg(f, 2));
        break;
    case SYS_socket:
        ret = sock_sys_socket((u32)arg(f, 0), (u32)arg(f, 1), (u32)arg(f, 2));
        break;
    case SYS_bind:
        ret = sock_sys_bind((u32)arg(f, 0), arg(f, 1));
        break;
    case SYS_listen:
        ret = sock_sys_listen((u32)arg(f, 0), (u32)arg(f, 1));
        break;
    case SYS_accept:
        ret = sock_sys_accept((u32)arg(f, 0), arg(f, 1));
        break;
    case SYS_connect:
        ret = sock_sys_connect((u32)arg(f, 0), arg(f, 1));
        break;
    case SYS_send:
        ret = sock_sys_send((u32)arg(f, 0), arg(f, 1), arg(f, 2));
        break;
    case SYS_recv:
        ret = sock_sys_recv((u32)arg(f, 0), arg(f, 1), arg(f, 2));
        break;
    case SYS_sendto:
        ret = sock_sys_sendto((u32)arg(f, 0), arg(f, 1), arg(f, 2), arg(f, 3));
        break;
    case SYS_recvfrom:
        ret = sock_sys_recvfrom((u32)arg(f, 0), arg(f, 1), arg(f, 2), arg(f, 3));
        break;
    case SYS_shutdown:
        ret = sock_sys_shutdown((u32)arg(f, 0), (u32)arg(f, 1));
        break;
    case SYS_getsockname:
        ret = sock_sys_name((u32)arg(f, 0), arg(f, 1), false);
        break;
    case SYS_getpeername:
        ret = sock_sys_name((u32)arg(f, 0), arg(f, 1), true);
        break;
    case SYS_setsockopt:
        ret = sock_sys_setsockopt((u32)arg(f, 0), (u32)arg(f, 1), (u32)arg(f, 2), arg(f, 3), (u32)arg(f, 4));
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
        ret = futex_wait(arg(f, 0), (u32)arg(f, 1));
        break;
    case SYS_futex_wake:
        ret = futex_wake(arg(f, 0), (i32)arg(f, 1));
        break;
    case SYS_fcntl:
        ret = sys_fcntl(f);
        break;
    case SYS_mmap:
        ret = mm_mmap(arg(f, 0), arg(f, 1), (u32)arg(f, 2), (u32)arg(f, 3), (int)arg(f, 4), (off_t)arg(f, 5));
        break;
    case SYS_munmap:
        ret = mm_munmap(arg(f, 0), arg(f, 1));
        break;
    case SYS_mprotect:
        ret = mm_mprotect(arg(f, 0), arg(f, 1), (u32)arg(f, 2));
        break;
    case SYS_memfd_create:
        ret = sys_memfd_create(f);
        break;
    case SYS_ftruncate:
        ret = sys_ftruncate(f);
        break;
    case SYS_ioctl:
        ret = sys_ioctl(f);
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
    /* A long is the register's width: a negative error stays negative to
     * the program (bits 63-32 copy bit 31 of -EFAULT, say), and a
     * pointer (sbrk's) comes back whole. */
    f->regs[REG_A0] = (uintptr)ret;
}
