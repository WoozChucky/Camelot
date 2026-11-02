/* Processes (spec section 10, "Kernel processes"): user programs in U-mode,
 * each with its own page table and kernel stack, switched round robin on
 * every timer tick, and kernel processes, which run a kernel function on
 * their own kernel stack. Either can sleep in the kernel until an event wakes
 * it (input, a child ending, the clock, a disk); the system calls are in
 * syscall.c. */
#ifndef CAMELOT_PROC_H
#define CAMELOT_PROC_H

#include "context.h"
#include "elf.h"
#include "file.h"
#include "spinlock.h"
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
#define PROC_NEW 5      /* taken by proc_create(), which may sleep reading the program */

/* The fields up to `name` are at fixed offsets (state 0, pid 4, name 8):
 * the kernel tests read them.
 *
 * Locking: `proc_lock` (proc.c) protects every slot's state, pid, name,
 * parent, xstatus, chan and killed, and the scheduler's choices. What only
 * the process itself changes (its address space, heap, files, working
 * directory, trap frame and context) needs no lock while it runs; others
 * (ps) read it under proc_lock and may see it just before or after a
 * change. */
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
    /* Its file descriptors: ofile[fd] is the open file, or NULL (file.h). */
    struct file *ofile[NOFILE];
    /* Its working directory, where paths that don't start with `/` are
     * looked up from: an inode with a reference, or NULL when there is no
     * filesystem (and for kernel processes, which use the root). */
    struct inode *cwd;
    /* Set by kill(): the process ends (exit status -1) when it next would
     * return to U-mode, and the system calls that sleep for long (sleep,
     * wait, reading the console or a pipe, writing a pipe) give up at once. */
    bool killed;
};

/* A user program embedded in the kernel image as an ELF executable
 * (kernel/userprogs.s, generated from kernel/user/ by `cargo xtask kernel`):
 * what spawn and exec run when there is no root filesystem (exec.c). */
struct user_program {
    const char *name;
    const u8 *elf;
    u32 size;
};

extern const struct user_program user_programs[];
extern const u32 user_program_count;

/* The embedded program called `name`, or NULL. */
const struct user_program *user_program_find(const char *name);

/* A new program's arguments, in kernel memory: `argc` NUL-terminated strings
 * one after another in `strings`, `len` bytes in all. proc_create() and
 * proc_exec() copy them to the top of the program's stack. */
struct proc_args {
    u32 argc;
    u32 len;
    char strings[ARG_MAX];
};

/* Sets up the process table's lock. kmain calls it before anything starts a
 * process. */
void proc_init(void);

/* The process on this hart (mycpu()->proc), or NULL while the kernel runs
 * for itself (kmain, the scheduler loop). */
struct proc *myproc(void);

/* Makes a runnable process called `name` from the executable `prog`, a
 * child of the current process: a page table with the program loaded, a
 * stack holding `args` with sp below them, a kernel stack, a pid, copies of
 * the parent's descriptors 0, 1 and 2 (the others are not passed on) and
 * its working directory. It starts at the program's entry point with a0 =
 * argc, a1 = argv. Returns NULL with *err set to -EAGAIN (no free slot),
 * -ENOMEM or -ENOEXEC (a bad ELF image: the loader's reason is printed as
 * "spawn: PATH: REASON", `path` being what the caller asked for) when it
 * can't; nothing is left allocated then. Reading `prog` may sleep. */
struct proc *proc_create(const struct elf_file *prog, const char *path, const char *name,
                         const struct proc_args *args, int *err);

/* Replaces the current process's program with `prog`, called `name`: a new
 * address space as proc_create() makes, registers all zero but pc, sp, a0
 * (argc) and a1 (argv). The descriptors, the working directory, the pid and
 * the parent stay. Returns argc, which the system call leaves in a0; or,
 * with the old program untouched, -ENOMEM or -ENOEXEC ("exec: PATH:
 * REASON"). */
int proc_exec(const struct elf_file *prog, const char *path, const char *name, const struct proc_args *args);

/* Makes a runnable kernel process called `name` that runs `fn` in S-mode on
 * its own kernel stack, with the kernel page table and interrupts off (like
 * any process's kernel code), and ends quietly when `fn` returns. It has no
 * parent: the scheduler frees it. Returns NULL with *err saying why when it
 * can't (no free slot, no memory). */
struct proc *kproc_create(const char *name, void (*fn)(void), const char **err);

/* Starts the program at `path` (the shell) as the first process, with argv
 * = { name } and the console as its descriptors 0 (for reading), 1 and 2
 * (for writing), in the root directory if there is a filesystem, and runs
 * the scheduler until no process is left: the first process has exited and
 * so has everything still running when it did. The process loads its
 * program itself (exec.c's exec_init(), which may sleep on the disk).
 * Returns the first process's exit status in *status, or false if it
 * couldn't be started (having said why). */
bool proc_run_init(const char *path, const char *name, int *status);

/* Runs the processes round robin, one timer tick each for user processes,
 * until none is left; then returns with interrupts on. While every process
 * left is asleep (or a zombie its parent will free) it idles in wfi. Called
 * by proc_run_init() and by the boot self-tests that use kernel processes. */
void scheduler(void);

/* Puts the current process to sleep on `chan` and gives up the CPU; returns
 * once wakeup(chan) has made it runnable and the scheduler picked it again.
 * The caller holds `lk`, the spinlock that protects the condition it waits
 * for (checked under `lk` before the call): sleep() takes proc_lock before
 * it lets go of `lk`, and wakeup() needs proc_lock too, so a wakeup that
 * comes after the check finds the process asleep and none is lost. `lk` is
 * held again when sleep() returns. Callers check their condition in a loop
 * around it: a wakeup only says it may have changed. Must be called from a
 * process's kernel code, holding no other spinlock than `lk`. */
void sleep(const void *chan, struct spinlock *lk);

/* Makes every process sleeping on `chan` runnable. Callable anywhere,
 * interrupt handlers included, except with proc_lock held; callers hold the
 * lock that protects what changed, the one the sleepers passed to
 * sleep(). */
void wakeup(const void *chan);

/* Gives the CPU back to the scheduler; the process stays runnable. */
void yield(void);

/* Ends the current process with `status`: it closes its files and lets go
 * of its working directory, becomes a zombie until its parent waits for it
 * (the scheduler frees it if it has none), its parent is woken, and its own
 * children are orphaned (zombies among them freed). Never returns. */
void proc_exit(int status);

/* Kills the current process for the trap in `f`, a fault in U-mode: prints
 * why (the cause, scause/sepc/stval and, for a page fault, how the process's
 * page table maps stval), then exits it with status -1. Never returns. */
void proc_kill(struct trap_frame *f);

/* kill(): marks the process `pid` killed (see `killed` in struct proc) and
 * wakes it if it sleeps. 0 (also for a zombie, which has already ended),
 * -ESRCH if there is no such process, -EPERM for a kernel process or the
 * first one (the shell). */
int proc_kill_pid(int pid);

/* Whether the current process has been killed. */
bool proc_killed(void);

/* procs(): describes up to `max` processes, in pid order, in `out` and
 * returns how many there are in all. */
int proc_list(struct procinfo *out, int max);

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

#endif
