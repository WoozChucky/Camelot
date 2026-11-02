/* Processes (spec section 10, "Kernel processes"): user programs in U-mode,
 * each with its own page table and kernel stack, switched round robin on
 * every timer tick, and kernel processes, which run a kernel function on
 * their own kernel stack. Either can sleep until an event wakes it. */
#ifndef CAMELOT_PROC_H
#define CAMELOT_PROC_H

#include "trap.h"
#include "types.h"
#include "vm.h"

/* Process slots. */
#define NPROC 8

/* Longest process name, without the NUL. */
#define PROC_NAME_MAX 15

/* Process states. */
#define PROC_UNUSED 0   /* a free slot */
#define PROC_RUNNABLE 1 /* waiting for the scheduler */
#define PROC_RUNNING 2  /* on the CPU */
#define PROC_ZOMBIE 3   /* exited or killed; the scheduler frees it */
#define PROC_SLEEPING 4 /* waiting in sleep() for a wakeup() on its channel */

/* What swtch() saves of a kernel thread: the return address, the stack
 * pointer and the callee-saved registers, which is all a function call has
 * to preserve. swtch.s uses the same offsets (ra at 0, sp at 4, s0-s11 from
 * 8): keep them in sync. */
struct context {
    u32 ra;
    u32 sp;
    u32 s[12];
};

struct proc {
    u32 state;
    u32 pid;
    char name[PROC_NAME_MAX + 1];
    /* User space below USER_TOP, the kernel's mappings above (vm.h). NULL
     * for a kernel process, which runs on the kernel page table. */
    pagetable_t pagetable;
    /* One page. The trap frame takes its top TRAP_FRAME_SIZE bytes, and the
     * kernel's stack for this process grows down from the frame. A kernel
     * process has no frame: its stack starts at the top of the page. */
    void *kstack;
    /* The user registers while the process is in the kernel. user_vec saves
     * them here (sscratch points at it in U-mode) and user_return loads
     * them back. NULL for a kernel process. */
    struct trap_frame *frame;
    /* Where swtch() resumes the process's kernel thread. */
    struct context context;
    /* What a PROC_SLEEPING process waits for: any address that stands for
     * the event, such as the disk or the buffer it wants. */
    void *chan;
    /* A kernel process's function (NULL for a user process). */
    void (*kfn)(void);
};

/* A user program embedded in the kernel image as an ELF executable
 * (kernel/userprogs.s, generated from kernel/user/ by `cargo xtask kernel`). */
struct user_program {
    const char *name;
    const u8 *elf;
    u32 size;
};

extern const struct user_program user_programs[];
extern const u32 user_program_count;

/* The embedded program called `name`, or NULL. */
const struct user_program *user_program_find(const char *name);

/* The process on the CPU, or NULL while the kernel runs for itself (the
 * shell, the scheduler loop). */
struct proc *myproc(void);

/* Makes a runnable process from `prog`: a page table with the program loaded
 * and a stack, a kernel stack, a pid. Returns NULL with *err saying why when
 * it can't (no free slot, no memory, a bad ELF image); nothing is left
 * allocated then. */
struct proc *proc_create(const struct user_program *prog, const char **err);

/* Makes a runnable kernel process called `name` that runs `fn` in S-mode on
 * its own kernel stack, with the kernel page table and interrupts off (like
 * any process's kernel code), and ends quietly when `fn` returns. Returns
 * NULL with *err saying why when it can't (no free slot, no memory). */
struct proc *kproc_create(const char *name, void (*fn)(void), const char **err);

/* Runs the processes round robin, one timer tick each for user processes,
 * until none is left; then returns with interrupts on. While every process
 * left is asleep it idles in wfi. Called by the shell's `run` and by the
 * boot self-tests that use kernel processes. */
void scheduler(void);

/* Puts the current process to sleep on `chan` and gives up the CPU; returns
 * once wakeup(chan) has made it runnable and the scheduler picked it again.
 * Callers check their condition in a loop around it: a wakeup only says it
 * may have changed. Must be called from a process's kernel code, where
 * interrupts are off, so nothing can wake the channel between the caller's
 * check and the sleep. */
void sleep(void *chan);

/* Makes every process sleeping on `chan` runnable. Callable anywhere,
 * interrupt handlers included. */
void wakeup(void *chan);

/* Gives the CPU back to the scheduler; the process stays runnable. */
void yield(void);

/* Ends the current process: it becomes a zombie, which the scheduler frees.
 * Prints "pid N (name) exited: status" unless the kernel killed it. Never
 * returns. */
void proc_exit(int status);

/* Kills the current process for the trap in `f`, a fault in U-mode: prints
 * why (the cause, scause/sepc/stval and, for a page fault, how the process's
 * page table maps stval), then exits it. Never returns. */
void proc_kill(struct trap_frame *f);

/* Saves the callee-saved registers in `old` and resumes `new` (swtch.s). */
void swtch(struct context *old, struct context *new);

#endif
