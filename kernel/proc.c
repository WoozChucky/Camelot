/* Processes and the scheduler (spec section 10, "Kernel processes"), after
 * xv6-riscv's proc.c, for one hart and without locks: the kernel never takes
 * an interrupt while it works for a process (see user_trap), so nothing here
 * runs concurrently with anything else.
 *
 * Each process has two threads of control: its user program in U-mode, and
 * a kernel thread on its own kernel stack that runs whenever the process
 * traps. The scheduler runs on kmain's boot stack; swtch() moves between it
 * and the processes' kernel threads. A process gives up the CPU only from
 * its kernel thread: in yield() at the end of its time slice (one timer
 * tick), or in proc_exit(). */
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

struct proc *proc_create(const struct user_program *prog, const char **err) {
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
    p->pagetable = uvm_create();
    if (p->kstack == NULL || p->pagetable == NULL) {
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

    p->pid = next_pid;
    next_pid++;
    set_name(p, prog->name);
    p->state = PROC_RUNNABLE;
    return p;
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

void scheduler(void) {
    /* proc_create() fills the lowest free slots, so the processes start in
     * the order they were created. */
    next_slot = 0;
    for (;;) {
        /* Let pending interrupts in (a byte typed on the console, a tick
         * that came while the last process was in the kernel) before going
         * on with interrupts off. */
        intr_on();
        intr_off();
        struct proc *p = pick_next();
        if (p == NULL) {
            break;
        }
        p->state = PROC_RUNNING;
        current = p;
        /* Its page table holds the kernel's mappings too, so the kernel
         * carries on as before; from the next sret on, U-mode sees the
         * process's memory. */
        w_satp(MAKE_SATP(p->pagetable));
        sfence_vma();
        swtch(&scheduler_context, &p->context);
        /* Back when it yields or exits. */
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
