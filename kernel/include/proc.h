/* Processes (spec section 10, "Kernel processes"): user programs in U-mode,
 * each with its own page table and kernel stack, switched round robin on
 * every timer tick, and kernel processes, which run a kernel function on
 * their own kernel stack. Either can sleep in the kernel until an event wakes
 * it (input, a child ending, the clock, a disk); the system calls are in
 * syscall.c. */
#ifndef CAMELOT_PROC_H
#define CAMELOT_PROC_H

#include "syscall.h"
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
#define PROC_ZOMBIE 3   /* exited or killed; its parent (or the scheduler) frees it */
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

/* The fields up to `name` are at fixed offsets (state 0, pid 4, name 8):
 * the kernel tests read them. */
struct proc {
    u32 state;
    u32 pid;
    char name[PROC_NAME_MAX + 1];
    /* The process that spawned it, which collects its exit status with
     * wait(); NULL for the first process, for kernel processes, and once the
     * parent has ended. */
    struct proc *parent;
    /* The exit status, for wait(), once it is a zombie. */
    int xstatus;
    /* User space below USER_TOP, the kernel's mappings above (vm.h). NULL
     * for a kernel process, which runs on the kernel page table. */
    pagetable_t pagetable;
    /* The heap, from the page after the program's last segment: sbrk() moves
     * `brk` (one past its last byte) between `heap_base` and USER_LOAD_TOP.
     * Every page from heap_base up to brk is mapped. */
    u32 heap_base;
    u32 brk;
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
     * the event, such as the disk or the buffer it wants, a child's parent or
     * the tick counter (wakeup() compares them). */
    const void *chan;
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

/* A new process's arguments, in kernel memory: `argc` NUL-terminated strings
 * one after another in `strings`, `len` bytes in all. proc_create() copies
 * them to the top of the new process's stack. */
struct proc_args {
    u32 argc;
    u32 len;
    char strings[ARG_MAX];
};

/* The process on the CPU, or NULL while the kernel runs for itself (kmain,
 * the scheduler loop). */
struct proc *myproc(void);

/* Makes a runnable process from `prog`, a child of the current process (of
 * no one, from kmain): a page table with the program loaded, a stack holding
 * `args` with sp below them, a kernel stack, a pid. It starts at the
 * program's entry point with a0 = argc, a1 = argv. Returns NULL with *err set
 * to -EAGAIN (no free slot), -ENOMEM or -ENOEXEC (a bad ELF image: the
 * loader's reason is printed as "spawn: NAME: REASON") when it can't; nothing
 * is left allocated then. */
struct proc *proc_create(const struct user_program *prog, const struct proc_args *args, int *err);

/* Makes a runnable kernel process called `name` that runs `fn` in S-mode on
 * its own kernel stack, with the kernel page table and interrupts off (like
 * any process's kernel code), and ends quietly when `fn` returns. It has no
 * parent: the scheduler frees it. Returns NULL with *err saying why when it
 * can't (no free slot, no memory). */
struct proc *kproc_create(const char *name, void (*fn)(void), const char **err);

/* Starts `prog` as the first process, with argv = { its name }, and runs the
 * scheduler until no process is left: the first process has exited and so has
 * everything still running when it did. Returns the first process's exit
 * status in *status, or false if it couldn't be started (having said why). */
bool proc_run_init(const struct user_program *prog, int *status);

/* Runs the processes round robin, one timer tick each for user processes,
 * until none is left; then returns with interrupts on. While every process
 * left is asleep (or a zombie its parent will free) it idles in wfi. Called
 * by proc_run_init() and by the boot self-tests that use kernel processes. */
void scheduler(void);

/* Puts the current process to sleep on `chan` and gives up the CPU; returns
 * once wakeup(chan) has made it runnable and the scheduler picked it again.
 * Callers check their condition in a loop around it: a wakeup only says it
 * may have changed. Must be called from a process's kernel code, where
 * interrupts are off, so nothing can wake the channel between the caller's
 * check and the sleep. */
void sleep(const void *chan);

/* Makes every process sleeping on `chan` runnable. Callable anywhere,
 * interrupt handlers included. */
void wakeup(const void *chan);

/* Gives the CPU back to the scheduler; the process stays runnable. */
void yield(void);

/* Ends the current process with `status`: it becomes a zombie until its
 * parent waits for it (the scheduler frees it if it has none), its parent is
 * woken, and its own children are orphaned (zombies among them freed). Never
 * returns. */
void proc_exit(int status);

/* Kills the current process for the trap in `f`, a fault in U-mode: prints
 * why (the cause, scause/sepc/stval and, for a page fault, how the process's
 * page table maps stval), then exits it with status -1. Never returns. */
void proc_kill(struct trap_frame *f);

/* wait(): waits for a child of the current process to end, any child if
 * `pid` is -1, frees it and returns its pid, storing its exit status at user
 * address `status` unless that is 0. -ECHILD if there is no such child,
 * -EFAULT if `status` is not writable user memory. */
int proc_wait(int pid, u32 status);

/* sbrk(): moves the end of the current process's heap by `increment` bytes
 * and returns where it was. New heap pages read zero; pages wholly above the
 * new end are freed when it shrinks. -ENOMEM if the heap would reach
 * USER_LOAD_TOP or a page can't be had (nothing changes then), -EINVAL if it
 * would shrink below its start. */
int proc_sbrk(i32 increment);

/* Saves the callee-saved registers in `old` and resumes `new` (swtch.s). */
void swtch(struct context *old, struct context *new);

#endif
