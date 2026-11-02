/* Processes and the scheduler (spec section 10, "Kernel processes"), after
 * xv6-riscv's proc.c, for one hart and without locks: the kernel never takes
 * an interrupt while it works for a process (see user_trap), so nothing here
 * runs concurrently with anything else.
 *
 * Each process has two threads of control: its user program in U-mode, and
 * a kernel thread on its own kernel stack that runs whenever the process
 * traps. A kernel process has only the kernel thread. The scheduler runs on
 * kmain's boot stack; swtch() moves between it and the processes' kernel
 * threads. A process gives up the CPU only from its kernel thread: in yield()
 * at the end of its time slice (one timer tick), in sleep(), or when it
 * ends. */
#include "proc.h"
#include "elf.h"
#include "kalloc.h"
#include "kprintf.h"
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

static void zombie(void);

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

/* A new kernel process starts here: runs its function, then ends. */
static void kproc_start(void) {
    current->kfn();
    zombie();
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

/* A free slot, cleared, with a kernel stack; NULL with *err set if there is
 * no free slot or no memory. The slot stays PROC_UNUSED until proc_ready(). */
static struct proc *proc_alloc(const char **err) {
    struct proc *p = NULL;
    for (u32 i = 0; i < NPROC; i++) {
        if (procs[i].state == PROC_UNUSED) {
            p = &procs[i];
            break;
        }
    }
    if (p == NULL) {
        *err = "too many processes";
        return NULL;
    }
    memset(p, 0, sizeof *p);
    p->kstack = kalloc();
    if (p->kstack == NULL) {
        *err = "out of memory";
        return NULL;
    }
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

struct proc *proc_create(const struct user_program *prog, const char **err) {
    struct proc *p = proc_alloc(err);
    if (p == NULL) {
        return NULL;
    }
    p->pagetable = uvm_create();
    if (p->pagetable == NULL) {
        *err = "out of memory";
        proc_free(p);
        return NULL;
    }

    u32 entry;
    const char *load_err = elf_load(p->pagetable, prog->elf, prog->size, &entry);
    if (load_err != NULL) {
        *err = load_err;
        proc_free(p);
        return NULL;
    }
    void *stack = kzalloc();
    if (stack == NULL || map_pages(p->pagetable, USER_STACK, (u32)(uintptr)stack, USER_STACK_SIZE,
                                   PTE_U | PTE_R | PTE_W) != 0) {
        if (stack != NULL) {
            kfree(stack);
        }
        *err = "out of memory";
        proc_free(p);
        return NULL;
    }

    /* The user registers: all zero but pc and sp. */
    p->frame = (struct trap_frame *)((u8 *)p->kstack + PGSIZE - TRAP_FRAME_SIZE);
    memset(p->frame, 0, sizeof *p->frame);
    p->frame->sepc = entry;
    p->frame->regs[REG_SP] = USER_TOP;

    /* The kernel thread: its first swtch() lands in proc_start, on the empty
     * kernel stack below the frame. */
    p->context.ra = (u32)(uintptr)proc_start;
    p->context.sp = (u32)(uintptr)p->frame;
    return proc_ready(p, prog->name);
}

struct proc *kproc_create(const char *name, void (*fn)(void), const char **err) {
    struct proc *p = proc_alloc(err);
    if (p == NULL) {
        return NULL;
    }
    /* No user space, so no page table of its own and no trap frame: its first
     * swtch() lands in kproc_start, on the empty kernel stack. */
    p->kfn = fn;
    p->context.ra = (u32)(uintptr)kproc_start;
    p->context.sp = (u32)(uintptr)p->kstack + PGSIZE;
    return proc_ready(p, name);
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

/* Whether some process sleeps, waiting for an interrupt to wake it. */
static bool any_sleeping(void) {
    for (u32 i = 0; i < NPROC; i++) {
        if (procs[i].state == PROC_SLEEPING) {
            return true;
        }
    }
    return false;
}

/* The page table to run `p` on: its own, or the kernel's for a kernel
 * process. */
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
         * completion that wakes a sleeper) before going on with interrupts
         * off. */
        intr_on();
        intr_off();
        struct proc *p = pick_next();
        if (p == NULL) {
            if (!any_sleeping()) {
                break;
            }
            /* Everyone left waits for an interrupt: idle until it comes.
             * Interrupts are off from pick_next() to wfi, so one that came
             * in between still wakes wfi, and is taken by intr_on() above. */
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
        /* Back when it yields, sleeps or ends. */
        w_satp(MAKE_SATP(kernel_pagetable));
        sfence_vma();
        current = NULL;
        if (p->state == PROC_ZOMBIE) {
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

void sleep(void *chan) {
    current->chan = chan;
    current->state = PROC_SLEEPING;
    sched();
    current->chan = NULL;
}

void wakeup(void *chan) {
    for (u32 i = 0; i < NPROC; i++) {
        if (procs[i].state == PROC_SLEEPING && procs[i].chan == chan) {
            procs[i].state = PROC_RUNNABLE;
        }
    }
}

/* Ends the current process; the scheduler frees it. */
static void zombie(void) {
    current->state = PROC_ZOMBIE;
    sched();
    panic("zombie ran again");
}

void proc_exit(int status) {
    kprintf("pid %u (%s) exited: %d\n", current->pid, current->name, status);
    zombie();
}

void proc_kill(struct trap_frame *f) {
    kprintf("pid %u (%s) killed: %s\n", current->pid, current->name, trap_cause_name(f->scause));
    kprintf("  scause 0x%08x  sepc 0x%08x  stval 0x%08x\n", f->scause, f->sepc, f->stval);
    if (is_page_fault(f->scause)) {
        vm_print_mapping(current->pagetable, f->stval);
    }
    zombie();
}
