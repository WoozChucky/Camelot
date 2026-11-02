/* Processes and the scheduler (spec section 10, "Kernel processes"), after
 * xv6-riscv's proc.c.
 *
 * Each process has two threads of control: its user program in U-mode, and
 * a kernel thread on its own kernel stack that runs whenever the process
 * traps. A kernel process has only the kernel thread. Each hart's scheduler
 * loop runs on the stack it booted with; swtch() moves between it and the
 * processes' kernel threads. A process gives up the CPU only from its kernel
 * thread: in yield() at the end of its time slice (one timer tick) or for
 * the yield system call, in sleep() when it has to wait, or in proc_exit().
 *
 * Locking (spec section 10, "Kernel locking"): one spinlock, proc_lock,
 * protects the whole process table, as in the x86 xv6 (xv6-riscv has one per
 * process). It is held across every switch between a process and the
 * scheduler: whoever gives up the CPU (yield, sleep, proc_exit) takes it and
 * calls sched(), and whoever the scheduler switches to lets go of it, either
 * on its way back out of sched() or, for a new process, first thing in
 * proc_start(). So a process's state and the switch away from it change
 * together, as far as any other hart can see: none can pick a process that
 * is still running on its own kernel stack. sleep() takes it before letting
 * go of the caller's lock, which is how no wakeup is lost. It comes after
 * every other lock but the allocator's and the UART's in the lock order. */
#include "proc.h"
#include "elf.h"
#include "exec.h"
#include "file.h"
#include "kalloc.h"
#include "kprintf.h"
#include "log.h"
#include "memlayout.h"
#include "riscv.h"
#include "spinlock.h"
#include "string.h"

static struct proc procs[NPROC];

/* The process table, the pids, the first process's status, and every
 * switch between a process and a scheduler. */
static struct spinlock proc_lock;

static u32 next_pid = 1;

/* Where the next round-robin search starts: the slot after the process that
 * ran last. */
static u32 next_slot;

/* The first process (proc_run_init), the program it runs, and its exit
 * status once it ended. */
static struct proc *init_proc;
static const char *init_path;
static const char *init_name;
static int init_status;

void proc_init(void) {
    initlock(&proc_lock, "proc");
}

/* No push_off() around the two reads, as xv6 has: a kernel thread is never
 * preempted, so it can't move to another hart in between. */
struct proc *myproc(void) {
    return mycpu()->proc;
}

const struct user_program *user_program_find(const char *name) {
    for (u32 i = 0; i < user_program_count; i++) {
        if (strcmp(user_programs[i].name, name) == 0) {
            return &user_programs[i];
        }
    }
    return NULL;
}

/* A new process's kernel thread starts here, from the scheduler's swtch(),
 * which still holds proc_lock, and goes straight to U-mode at the program's
 * entry point, unless it was killed before it got its first turn: then it
 * ends before running any of its code, as user_trap() would end it on any
 * later way back. */
static void proc_start(void) {
    release(&proc_lock);
    if (proc_killed()) {
        proc_exit(-1);
    }
    user_trap_return(myproc()->frame);
}

/* A new kernel process starts here, holding proc_lock as proc_start does:
 * runs its function, then ends. It has no parent, so the scheduler frees
 * it. */
static void kproc_start(void) {
    release(&proc_lock);
    myproc()->kfn();
    proc_exit(0);
}

/* Releases everything `p` holds (any of it may be missing) and frees its
 * slot. Its page table must not be the active one. Called with proc_lock
 * held. */
static void proc_free(struct proc *p) {
    if (p->pagetable != NULL) {
        uvm_free(p->pagetable);
        p->pagetable = NULL;
    }
    if (p->kstack != NULL) {
        kfree(p->kstack);
        p->kstack = NULL;
    }
    p->frame = NULL;
    p->parent = NULL;
    p->chan = NULL;
    p->state = PROC_UNUSED;
}

/* proc_free() for a slot the caller took and never made runnable. */
static void proc_unalloc(struct proc *p) {
    acquire(&proc_lock);
    proc_free(p);
    release(&proc_lock);
}

/* Copies at most PROC_NAME_MAX characters of `name`. Called with proc_lock
 * held, or on a slot nobody else looks at yet. */
static void set_name(struct proc *p, const char *name) {
    u32 i = 0;
    while (name[i] != 0 && i < PROC_NAME_MAX) {
        p->name[i] = name[i];
        i++;
    }
    p->name[i] = 0;
}

/* Puts `args` at the top of the stack page `stack` (mapped at USER_STACK) and
 * points frame `f`'s sp, a0 and a1 at them:
 *
 *   USER_TOP    the strings, argv[0]'s first, NUL-terminated
 *               padding to a multiple of 4
 *   argv        argv[0] ... argv[argc - 1], then a NULL
 *               padding to a multiple of 16
 *   sp          the program's first stack frame grows down from here
 *
 * ARG_MAX and MAXARG keep all of it well inside the page. */
static void push_args(struct trap_frame *f, u8 *stack, const struct proc_args *args) {
    u32 strings = USER_TOP - args->len;
    u32 argv = (strings & ~3u) - 4 * (args->argc + 1);
    memcpy(stack + (strings - USER_STACK), args->strings, args->len);
    u32 *slots = (u32 *)(stack + (argv - USER_STACK));
    u32 at = strings;
    for (u32 i = 0; i < args->argc; i++) {
        slots[i] = at;
        at += strlen(args->strings + (at - strings)) + 1;
    }
    slots[args->argc] = 0;
    f->regs[REG_SP] = argv & ~15u;
    f->regs[REG_A0] = args->argc;
    f->regs[REG_A1] = argv;
}

/* A free slot, cleared, with a kernel stack; NULL with *err set to -EAGAIN
 * (no free slot) or -ENOMEM if it can't. The slot is PROC_NEW until
 * proc_ready(), so that nobody else takes it while proc_create() sleeps
 * reading the program from the disk, and nobody else looks at its fields:
 * the caller sets them up without proc_lock. */
static struct proc *proc_alloc(int *err) {
    struct proc *p = NULL;
    acquire(&proc_lock);
    for (u32 i = 0; i < NPROC; i++) {
        if (procs[i].state == PROC_UNUSED) {
            p = &procs[i];
            break;
        }
    }
    if (p == NULL) {
        release(&proc_lock);
        *err = -EAGAIN;
        return NULL;
    }
    memset(p, 0, sizeof *p);
    p->state = PROC_NEW;
    release(&proc_lock);
    p->kstack = kalloc();
    if (p->kstack == NULL) {
        proc_unalloc(p);
        *err = -ENOMEM;
        return NULL;
    }
    return p;
}

/* Gives a fully set up `p` its pid, name and parent and makes it
 * runnable. */
static struct proc *proc_ready(struct proc *p, const char *name, struct proc *parent) {
    acquire(&proc_lock);
    p->pid = next_pid;
    next_pid++;
    set_name(p, name);
    p->parent = parent;
    p->state = PROC_RUNNABLE;
    release(&proc_lock);
    return p;
}

/* The trap frame at the top of `p`'s kernel stack page. */
static struct trap_frame *frame_of(struct proc *p) {
    return (struct trap_frame *)((u8 *)p->kstack + PGSIZE - TRAP_FRAME_SIZE);
}

/* A new user address space for `prog`: its segments and a zeroed stack
 * page with `args` at the top. Returns its page table and sets *heap (the
 * page after the last segment) and frame `f`: all zero but sepc (the entry
 * point), sp, a0 and a1. Or returns NULL with *err = -ENOMEM, or -ENOEXEC
 * with "WHO: PATH: REASON" printed, leaving `f` alone and nothing
 * allocated. */
static pagetable_t build_space(const struct elf_file *prog, const char *who, const char *path,
                               const struct proc_args *args, struct trap_frame *f, u32 *heap, int *err) {
    pagetable_t pt = uvm_create();
    if (pt == NULL) {
        *err = -ENOMEM;
        return NULL;
    }
    u32 entry, end;
    const char *load_err = elf_load(pt, prog, &entry, &end);
    if (load_err != NULL) {
        if (strcmp(load_err, ELF_OUT_OF_MEMORY) == 0) {
            *err = -ENOMEM;
        } else {
            kprintf("%s: %s: %s\n", who, path, load_err);
            *err = -ENOEXEC;
        }
        uvm_free(pt);
        return NULL;
    }
    u8 *stack = kzalloc();
    if (stack == NULL ||
        map_pages(pt, USER_STACK, (u32)(uintptr)stack, USER_STACK_SIZE, PTE_U | PTE_R | PTE_W) != 0) {
        if (stack != NULL) {
            kfree(stack);
        }
        *err = -ENOMEM;
        uvm_free(pt);
        return NULL;
    }
    *heap = end;
    /* The user registers: all zero but pc, sp and the arguments. */
    memset(f, 0, sizeof *f);
    f->sepc = entry;
    push_args(f, stack, args);
    return pt;
}

struct proc *proc_create(const struct elf_file *prog, const char *path, const char *name,
                         const struct proc_args *args, int *err) {
    struct proc *p = proc_alloc(err);
    if (p == NULL) {
        return NULL;
    }
    p->frame = frame_of(p);
    u32 heap;
    p->pagetable = build_space(prog, "spawn", path, args, p->frame, &heap, err);
    if (p->pagetable == NULL) {
        proc_unalloc(p);
        return NULL;
    }
    p->heap_base = heap;
    p->brk = heap;

    /* The kernel thread: its first swtch() lands in proc_start, on the empty
     * kernel stack below the frame. */
    p->context.ra = (u32)(uintptr)proc_start;
    p->context.sp = (u32)(uintptr)p->frame;
    /* What the child inherits: the standard descriptors, sharing their open
     * files (and offsets), and the working directory. A descriptor above 2
     * stays the parent's own, so a pipe end the parent keeps for itself
     * doesn't keep the pipe open in every child. */
    struct proc *parent = myproc();
    if (parent != NULL) {
        for (u32 fd = 0; fd <= FD_STDERR; fd++) {
            if (parent->ofile[fd] != NULL) {
                p->ofile[fd] = filedup(parent->ofile[fd]);
            }
        }
        if (parent->cwd != NULL) {
            p->cwd = idup(parent->cwd);
        }
    }
    return proc_ready(p, name, parent);
}

int proc_exec(const struct elf_file *prog, const char *path, const char *name, const struct proc_args *args) {
    struct proc *p = myproc();
    pagetable_t old = p->pagetable;
    u32 heap;
    int err;
    pagetable_t pt = build_space(prog, "exec", path, args, p->frame, &heap, &err);
    if (pt == NULL) {
        return err;
    }
    p->pagetable = pt;
    p->heap_base = heap;
    p->brk = heap;
    acquire(&proc_lock);
    set_name(p, name);
    release(&proc_lock);
    /* The new table holds the kernel's mappings too: switch to it before
     * freeing the old one, which satp points at. */
    w_satp(MAKE_SATP(pt));
    sfence_vma();
    if (old != NULL) {
        uvm_free(old);
    }
    return (int)args->argc;
}

struct proc *kproc_create(const char *name, void (*fn)(void), const char **err) {
    int alloc_err;
    struct proc *p = proc_alloc(&alloc_err);
    if (p == NULL) {
        *err = alloc_err == -EAGAIN ? "too many processes" : "out of memory";
        return NULL;
    }
    /* No user space, so no page table of its own and no trap frame: its first
     * swtch() lands in kproc_start, on the empty kernel stack. */
    p->kfn = fn;
    p->context.ra = (u32)(uintptr)kproc_start;
    p->context.sp = (u32)(uintptr)p->kstack + PGSIZE;
    return proc_ready(p, name, NULL);
}

/* The first process's kernel thread starts here, from the scheduler (holding
 * proc_lock, as for proc_start): it gets the console as its descriptors and
 * the root as its working directory, loads its program (which may sleep on
 * the disk, so it has to be done by a process) and goes to U-mode. */
static void init_start(void) {
    release(&proc_lock);
    struct proc *p = myproc();
    p->ofile[FD_STDIN] = file_console(true, false);
    p->ofile[FD_STDOUT] = file_console(false, true);
    p->ofile[FD_STDERR] = file_console(false, true);
    if (fs_mounted) {
        p->cwd = iget(fs_dev, ROOTINO);
    }
    int err = exec_init(init_path, init_name);
    if (err < 0) {
        kprintf("cannot start %s: error %d\n", init_path, -err);
        proc_exit(-1);
    }
    user_trap_return(p->frame);
}

bool proc_run_init(const char *path, const char *name, int *status) {
    int err;
    struct proc *p = proc_alloc(&err);
    if (p == NULL) {
        kprintf("cannot start %s: error %d\n", path, -err);
        return false;
    }
    /* No address space yet: until its exec, it runs on the kernel page
     * table, like a kernel process. */
    p->frame = frame_of(p);
    memset(p->frame, 0, sizeof *p->frame);
    p->context.ra = (u32)(uintptr)init_start;
    p->context.sp = (u32)(uintptr)p->frame;
    acquire(&proc_lock);
    init_path = path;
    init_name = name;
    init_proc = p;
    release(&proc_lock);
    proc_ready(p, name, NULL);
    scheduler();
    acquire(&proc_lock);
    *status = init_status;
    release(&proc_lock);
    return true;
}

/* The next runnable process after the last one that ran, or NULL. Called
 * with proc_lock held. */
static struct proc *pick_next(void) {
    for (u32 i = 0; i < NPROC; i++) {
        u32 slot = (next_slot + i) % NPROC;
        if (procs[slot].state == PROC_RUNNABLE) {
            next_slot = (slot + 1) % NPROC;
            return &procs[slot];
        }
    }
    return NULL;
}

/* Whether any slot holds a process that hasn't been freed. Called with
 * proc_lock held. */
static bool any_process(void) {
    for (u32 i = 0; i < NPROC; i++) {
        if (procs[i].state != PROC_UNUSED) {
            return true;
        }
    }
    return false;
}

/* The page table to run `p` on: its own, or the kernel's for a kernel
 * process (and for the first process until it has loaded its program). */
static pagetable_t proc_pagetable(struct proc *p) {
    if (p->pagetable != NULL) {
        return p->pagetable;
    }
    return kernel_pagetable;
}

void scheduler(void) {
    struct cpu *c = mycpu();
    /* proc_create() fills the lowest free slots, so the processes start in
     * the order they were created. */
    acquire(&proc_lock);
    next_slot = 0;
    release(&proc_lock);
    for (;;) {
        /* Let pending interrupts in (a byte typed on the console, a tick
         * that came while the last process was in the kernel, a disk's
         * completion) before going on with interrupts off. Their handlers
         * may wake processes up. Interrupts stay off from here on: the
         * acquire() below finds them off, so its release() leaves them
         * off. */
        intr_on();
        intr_off();
        acquire(&proc_lock);
        struct proc *p = pick_next();
        if (p == NULL) {
            bool left = any_process();
            release(&proc_lock);
            if (!left) {
                break;
            }
            /* Everyone sleeps (or is a zombie its parent will free). Idle
             * until an interrupt: interrupts are off from the search to wfi,
             * so one arriving in between still wakes it, and is taken by the
             * intr_on() at the top of the loop. proc_lock is not held across
             * the wait, so other harts can go on scheduling. */
            wfi();
            continue;
        }
        p->state = PROC_RUNNING;
        c->proc = p;
        /* Its page table holds the kernel's mappings too, so the kernel
         * carries on as before; from the next sret on, U-mode sees the
         * process's memory. */
        w_satp(MAKE_SATP(proc_pagetable(p)));
        sfence_vma();
        /* proc_lock stays held across the switch: the process lets go of
         * it (proc_start, or the end of yield, sleep), and takes it again
         * before it switches back here (sched). */
        swtch(&c->context, &p->context);
        /* Back when it yields, sleeps or exits. */
        w_satp(MAKE_SATP(kernel_pagetable));
        sfence_vma();
        c->proc = NULL;
        /* A zombie with a parent waits for it; an orphan has no one. */
        if (p->state == PROC_ZOMBIE && p->parent == NULL) {
            proc_free(p);
        }
        release(&proc_lock);
    }
    intr_on();
}

/* Back to the scheduler, which resumes this process's kernel thread here
 * when the process's turn comes again. The caller holds proc_lock and no
 * other spinlock, and has changed the process's state. `intena` belongs to
 * this kernel thread rather than to the hart, so it is saved and restored
 * around the switch (another thread may run on the hart meanwhile). */
static void sched(void) {
    struct proc *p = myproc();
    struct cpu *c = mycpu();
    if (!holding(&proc_lock)) {
        panic("sched: proc_lock not held");
    }
    if (c->noff != 1) {
        panic("sched: holding a lock other than proc_lock");
    }
    if (p->state == PROC_RUNNING) {
        panic("sched: the process is still running");
    }
    if ((r_sstatus() & SSTATUS_SIE) != 0) {
        panic("sched: interrupts are on");
    }
    u32 intena = c->intena;
    swtch(&p->context, &c->context);
    mycpu()->intena = intena;
}

void yield(void) {
    acquire(&proc_lock);
    myproc()->state = PROC_RUNNABLE;
    sched();
    release(&proc_lock);
}

void sleep(const void *chan, struct spinlock *lk) {
    struct proc *p = myproc();
    /* proc_lock first, then let go of lk: a wakeup(chan) needs proc_lock,
     * so it can't run between the caller's check (under lk) and the
     * process being marked asleep. */
    if (lk != &proc_lock) {
        acquire(&proc_lock);
        release(lk);
    }
    p->chan = chan;
    p->state = PROC_SLEEPING;
    sched();
    p->chan = NULL;
    if (lk != &proc_lock) {
        release(&proc_lock);
        acquire(lk);
    }
}

/* wakeup(), with proc_lock held. */
static void wakeup_locked(const void *chan) {
    for (u32 i = 0; i < NPROC; i++) {
        if (procs[i].state == PROC_SLEEPING && procs[i].chan == chan) {
            procs[i].state = PROC_RUNNABLE;
        }
    }
}

void wakeup(const void *chan) {
    acquire(&proc_lock);
    wakeup_locked(chan);
    release(&proc_lock);
}

void proc_exit(int status) {
    struct proc *p = myproc();
    /* Its files and working directory, while it can still sleep: closing a
     * file or letting go of a directory may free an unlinked inode, a
     * transaction. */
    for (u32 fd = 0; fd < NOFILE; fd++) {
        if (p->ofile[fd] != NULL) {
            struct file *f = p->ofile[fd];
            p->ofile[fd] = NULL;
            fileclose(f);
        }
    }
    if (p->cwd != NULL) {
        begin_op();
        iput(p->cwd);
        end_op();
        p->cwd = NULL;
    }
    acquire(&proc_lock);
    for (u32 i = 0; i < NPROC; i++) {
        struct proc *child = &procs[i];
        if (child->state == PROC_UNUSED || child->parent != p) {
            continue;
        }
        /* No one will wait for it now. A zombie can go at once (it isn't
         * running, and its page table isn't the active one); the scheduler
         * frees the others when they end. */
        child->parent = NULL;
        if (child->state == PROC_ZOMBIE) {
            proc_free(child);
        }
    }
    p->xstatus = status;
    if (p == init_proc) {
        init_status = status;
        init_proc = NULL;
    }
    if (p->parent != NULL) {
        wakeup_locked(p->parent);
    }
    p->state = PROC_ZOMBIE;
    sched();
    panic("zombie ran again");
}

void proc_kill(struct trap_frame *f) {
    struct proc *p = myproc();
    kprintf("pid %u (%s) killed: %s\n", p->pid, p->name, trap_cause_name(f->scause));
    kprintf("  scause 0x%08x  sepc 0x%08x  stval 0x%08x\n", f->scause, f->sepc, f->stval);
    if (is_page_fault(f->scause)) {
        vm_print_mapping(p->pagetable, f->stval);
    }
    proc_exit(-1);
}

int proc_kill_pid(int pid) {
    int r = -ESRCH;
    acquire(&proc_lock);
    for (u32 i = 0; i < NPROC; i++) {
        struct proc *p = &procs[i];
        if (p->state == PROC_UNUSED || p->state == PROC_NEW || p->pid != (u32)pid) {
            continue;
        }
        /* Neither the kernel's own processes nor the first one, the shell,
         * whose end is Camelot's. */
        if (p->kfn != NULL || p == init_proc) {
            r = -EPERM;
            break;
        }
        if (p->state != PROC_ZOMBIE) {
            p->killed = true;
            /* Every sleep() is in a loop that checks what it waits for, so
             * an early wakeup is harmless; the long waits check `killed`. */
            if (p->state == PROC_SLEEPING) {
                p->state = PROC_RUNNABLE;
            }
        }
        r = 0;
        break;
    }
    release(&proc_lock);
    return r;
}

bool proc_killed(void) {
    struct proc *p = myproc();
    if (p == NULL) {
        return false;
    }
    acquire(&proc_lock);
    bool killed = p->killed;
    release(&proc_lock);
    return killed;
}

int proc_list(struct procinfo *out, int max) {
    int n = 0;
    u32 last = 0;
    acquire(&proc_lock);
    /* In pid order: the smallest pid above the last one, each time. */
    for (;;) {
        struct proc *next = NULL;
        for (u32 i = 0; i < NPROC; i++) {
            struct proc *p = &procs[i];
            if (p->state == PROC_UNUSED || p->state == PROC_NEW || p->pid <= last) {
                continue;
            }
            if (next == NULL || p->pid < next->pid) {
                next = p;
            }
        }
        if (next == NULL) {
            release(&proc_lock);
            return n;
        }
        if (n < max) {
            struct procinfo *info = &out[n];
            memset(info, 0, sizeof *info);
            info->pid = next->pid;
            info->ppid = next->parent != NULL ? next->parent->pid : 0;
            info->state = next->state;
            info->size = next->pagetable != NULL ? next->brk : 0;
            memcpy(info->name, next->name, sizeof info->name);
        }
        n++;
        last = next->pid;
    }
}

int proc_wait(int pid, u32 status) {
    struct proc *p = myproc();
    if (status != 0 && !uvm_range_ok(p->pagetable, status, sizeof(int), PTE_W)) {
        return -EFAULT;
    }
    acquire(&proc_lock);
    for (;;) {
        bool found = false;
        for (u32 i = 0; i < NPROC; i++) {
            struct proc *child = &procs[i];
            if (child->state == PROC_UNUSED || child->parent != p) {
                continue;
            }
            if (pid != -1 && child->pid != (u32)pid) {
                continue;
            }
            found = true;
            if (child->state == PROC_ZOMBIE) {
                int child_pid = (int)child->pid;
                int xstatus = child->xstatus;
                /* Checked above, and nothing unmaps it while we sleep.
                 * copyout() walks the page table and never sleeps, so it
                 * may run under the lock. */
                if (status != 0 && copyout(p->pagetable, status, &xstatus, sizeof xstatus) != 0) {
                    release(&proc_lock);
                    return -EFAULT;
                }
                proc_free(child);
                release(&proc_lock);
                return child_pid;
            }
        }
        if (!found || p->killed) {
            /* Killed: never seen, the caller ends on the way out. */
            release(&proc_lock);
            return -ECHILD;
        }
        /* proc_exit() wakes the parent. */
        sleep(p, &proc_lock);
    }
}

int proc_sbrk(i32 increment) {
    struct proc *p = myproc();
    u32 old = p->brk;
    if (increment > 0) {
        u32 grow = (u32)increment;
        if (grow > USER_LOAD_TOP - old) {
            return -ENOMEM;
        }
        u32 end = old + grow;
        /* Map the pages from the first one not yet mapped up to the new
         * end; on failure, unmap them again. */
        u32 first = PGROUNDUP(old);
        for (u32 a = first; a < end; a += PGSIZE) {
            void *mem = kzalloc();
            if (mem == NULL || map_pages(p->pagetable, a, (u32)(uintptr)mem, PGSIZE, PTE_U | PTE_R | PTE_W) != 0) {
                if (mem != NULL) {
                    kfree(mem);
                }
                if (a > first) {
                    unmap_pages(p->pagetable, first, a - first, true);
                }
                return -ENOMEM;
            }
        }
        p->brk = end;
    } else if (increment < 0) {
        u32 shrink = 0u - (u32)increment;
        if (shrink > old - p->heap_base) {
            return -EINVAL;
        }
        u32 end = old - shrink;
        u32 keep = PGROUNDUP(end);
        if (keep < PGROUNDUP(old)) {
            unmap_pages(p->pagetable, keep, PGROUNDUP(old) - keep, true);
        }
        /* The rest of the last page kept reads zero again, so that growing
         * later hands out zeroed memory, as for a new page. */
        if (end < keep) {
            pte_t *pte = walk(p->pagetable, end, false);
            u8 *page = (u8 *)(uintptr)PTE2PA(*pte);
            memset(page + (end % PGSIZE), 0, keep - end);
        }
        p->brk = end;
    }
    return (int)old;
}
