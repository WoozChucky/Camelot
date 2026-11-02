/* Processes and the scheduler (spec section 10, "Kernel processes"), after
 * xv6-riscv's proc.c, for one hart and without locks: the kernel never takes
 * an interrupt while it works for a process (see user_trap), so nothing here
 * runs concurrently with anything else. The interrupt handlers only call
 * wakeup(), from the scheduler's own loop.
 *
 * Each process has two threads of control: its user program in U-mode, and
 * a kernel thread on its own kernel stack that runs whenever the process
 * traps. A kernel process has only the kernel thread. The scheduler runs on
 * kmain's boot stack; swtch() moves between it and the processes' kernel
 * threads. A process gives up the CPU only from its kernel thread: in yield()
 * at the end of its time slice (one timer tick) or for the yield system call,
 * in sleep() when it has to wait, or in proc_exit(). */
#include "proc.h"
#include "elf.h"
#include "exec.h"
#include "file.h"
#include "kalloc.h"
#include "kprintf.h"
#include "log.h"
#include "memlayout.h"
#include "riscv.h"
#include "string.h"

static struct proc procs[NPROC];

/* The process on the CPU; NULL in the scheduler. */
static struct proc *current;

/* The scheduler's registers while a process runs. */
static struct context scheduler_context;

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

struct proc *myproc(void) {
    return current;
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
 * and goes straight to U-mode at the program's entry point. */
static void proc_start(void) {
    user_trap_return(current->frame);
}

/* A new kernel process starts here: runs its function, then ends. It has no
 * parent, so the scheduler frees it. */
static void kproc_start(void) {
    current->kfn();
    proc_exit(0);
}

/* Releases everything `p` holds (any of it may be missing) and frees its
 * slot. Its page table must not be the active one. */
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

/* Copies at most PROC_NAME_MAX characters of `name`. */
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
 * reading the program from the disk. */
static struct proc *proc_alloc(int *err) {
    struct proc *p = NULL;
    for (u32 i = 0; i < NPROC; i++) {
        if (procs[i].state == PROC_UNUSED) {
            p = &procs[i];
            break;
        }
    }
    if (p == NULL) {
        *err = -EAGAIN;
        return NULL;
    }
    memset(p, 0, sizeof *p);
    p->kstack = kalloc();
    if (p->kstack == NULL) {
        *err = -ENOMEM;
        return NULL;
    }
    p->state = PROC_NEW;
    return p;
}

/* Gives a fully set up `p` its pid and name and makes it runnable. */
static struct proc *proc_ready(struct proc *p, const char *name) {
    p->pid = next_pid;
    next_pid++;
    set_name(p, name);
    p->state = PROC_RUNNABLE;
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
        proc_free(p);
        return NULL;
    }
    p->heap_base = heap;
    p->brk = heap;

    /* The kernel thread: its first swtch() lands in proc_start, on the empty
     * kernel stack below the frame. */
    p->context.ra = (u32)(uintptr)proc_start;
    p->context.sp = (u32)(uintptr)p->frame;
    p->parent = current;
    /* What the child inherits: the standard descriptors, sharing their open
     * files (and offsets), and the working directory. A descriptor above 2
     * stays the parent's own, so a pipe end the parent keeps for itself
     * doesn't keep the pipe open in every child. */
    if (current != NULL) {
        for (u32 fd = 0; fd <= FD_STDERR; fd++) {
            if (current->ofile[fd] != NULL) {
                p->ofile[fd] = filedup(current->ofile[fd]);
            }
        }
        if (current->cwd != NULL) {
            p->cwd = idup(current->cwd);
        }
    }
    return proc_ready(p, name);
}

int proc_exec(const struct elf_file *prog, const char *path, const char *name, const struct proc_args *args) {
    struct proc *p = current;
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
    set_name(p, name);
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
    return proc_ready(p, name);
}

/* The first process's kernel thread starts here, from the scheduler: it
 * gets the console as its descriptors and the root as its working
 * directory, loads its program (which may sleep on the disk, so it has to
 * be done by a process) and goes to U-mode. */
static void init_start(void) {
    struct proc *p = current;
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
    init_path = path;
    init_name = name;
    init_proc = proc_ready(p, name);
    scheduler();
    *status = init_status;
    return true;
}

/* The next runnable process after the last one that ran, or NULL. */
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

/* Whether any slot holds a process that hasn't been freed. */
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
    /* proc_create() fills the lowest free slots, so the processes start in
     * the order they were created. */
    next_slot = 0;
    for (;;) {
        /* Let pending interrupts in (a byte typed on the console, a tick
         * that came while the last process was in the kernel, a disk's
         * completion) before going on with interrupts off. Their handlers
         * may wake processes up. */
        intr_on();
        intr_off();
        struct proc *p = pick_next();
        if (p == NULL) {
            if (!any_process()) {
                break;
            }
            /* Everyone sleeps (or is a zombie its parent will free). Idle
             * until an interrupt: interrupts are off from the search to wfi,
             * so one arriving in between still wakes it, and is taken by the
             * intr_on() at the top of the loop. */
            wfi();
            continue;
        }
        p->state = PROC_RUNNING;
        current = p;
        /* Its page table holds the kernel's mappings too, so the kernel
         * carries on as before; from the next sret on, U-mode sees the
         * process's memory. */
        w_satp(MAKE_SATP(proc_pagetable(p)));
        sfence_vma();
        swtch(&scheduler_context, &p->context);
        /* Back when it yields, sleeps or exits. */
        w_satp(MAKE_SATP(kernel_pagetable));
        sfence_vma();
        current = NULL;
        /* A zombie with a parent waits for it; an orphan has no one. */
        if (p->state == PROC_ZOMBIE && p->parent == NULL) {
            proc_free(p);
        }
    }
    intr_on();
}

/* Back to the scheduler, which resumes this process's kernel thread here
 * when the process's turn comes again. */
static void sched(void) {
    swtch(&current->context, &scheduler_context);
}

void yield(void) {
    current->state = PROC_RUNNABLE;
    sched();
}

void sleep(const void *chan) {
    current->chan = chan;
    current->state = PROC_SLEEPING;
    sched();
    current->chan = NULL;
}

void wakeup(const void *chan) {
    for (u32 i = 0; i < NPROC; i++) {
        if (procs[i].state == PROC_SLEEPING && procs[i].chan == chan) {
            procs[i].state = PROC_RUNNABLE;
        }
    }
}

void proc_exit(int status) {
    struct proc *p = current;
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
        wakeup(p->parent);
    }
    p->state = PROC_ZOMBIE;
    sched();
    panic("zombie ran again");
}

void proc_kill(struct trap_frame *f) {
    kprintf("pid %u (%s) killed: %s\n", current->pid, current->name, trap_cause_name(f->scause));
    kprintf("  scause 0x%08x  sepc 0x%08x  stval 0x%08x\n", f->scause, f->sepc, f->stval);
    if (is_page_fault(f->scause)) {
        vm_print_mapping(current->pagetable, f->stval);
    }
    proc_exit(-1);
}

int proc_wait(int pid, u32 status) {
    struct proc *p = current;
    if (status != 0 && !uvm_range_ok(p->pagetable, status, sizeof(int), PTE_W)) {
        return -EFAULT;
    }
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
                /* Checked above, and nothing unmaps it while we sleep. */
                if (status != 0 && copyout(p->pagetable, status, &xstatus, sizeof xstatus) != 0) {
                    return -EFAULT;
                }
                proc_free(child);
                return child_pid;
            }
        }
        if (!found) {
            return -ECHILD;
        }
        /* proc_exit() wakes the parent. */
        sleep(p);
    }
}

int proc_sbrk(i32 increment) {
    struct proc *p = current;
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
