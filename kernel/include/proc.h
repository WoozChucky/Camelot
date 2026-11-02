/* Processes and threads (spec section 10, "Kernel processes" and "Kernel
 * threads").
 *
 * A process is what a program owns: its address space, its heap, its open
 * files and working directory, its pid, parent and exit status. A thread is
 * what the scheduler runs: registers (a trap frame and a saved kernel
 * context), a kernel stack and a state. Every process has one thread when
 * it starts, its main thread, and thread_create() adds more, which share
 * everything the process owns; the scheduler switches between threads round
 * robin on every timer tick, on every hart. A kernel process runs a kernel
 * function on its one thread's kernel stack. Any thread can sleep in the
 * kernel until an event wakes it (input, a child ending, the clock, a disk,
 * a futex); the system calls are in syscall.c. */
#ifndef CAMELOT_PROC_H
#define CAMELOT_PROC_H

#include "context.h"
#include "elf.h"
#include "file.h"
#include "sleeplock.h"
#include "spinlock.h"
#include "syscall.h"
#include "trap.h"
#include "types.h"
#include "vm.h"

/* Process slots. */
#define NPROC 8

/* Thread slots, for every process together: each process's main thread,
 * and as many more. A thread costs a slot and, while it runs, a page (its
 * kernel stack and trap frame); an ended one waiting for thread_join() only
 * its slot. */
#define NTHREAD 16

/* Longest process name, without the NUL. */
#define PROC_NAME_MAX 15

/* Process states. procs reports a live process's state as its threads':
 * running if one of them runs, else runnable if one is, else sleeping. */
#define PROC_UNUSED 0   /* a free slot */
#define PROC_RUNNABLE 1 /* (reported) a thread waits for the scheduler */
#define PROC_RUNNING 2  /* (reported) a thread is on a CPU */
#define PROC_ZOMBIE 3   /* exited or killed; its parent (or the scheduler) frees it */
#define PROC_SLEEPING 4 /* (reported) every thread waits in sleep() */
#define PROC_NEW 5      /* taken by proc_create(), which may sleep reading the program */
#define PROC_LIVE 6     /* running its program: it has threads that haven't ended */

/* Thread states: the same numbers as the processes' that procs reports. */
#define THREAD_UNUSED 0   /* a free slot */
#define THREAD_RUNNABLE 1 /* waiting for the scheduler */
#define THREAD_RUNNING 2  /* on a CPU */
#define THREAD_ZOMBIE 3   /* ended; keeps its status for thread_join() */
#define THREAD_SLEEPING 4 /* waiting in sleep() for a wakeup() on its channel */
#define THREAD_NEW 5      /* taken, not runnable yet */

struct proc;

/* One thread of a process.
 *
 * Locking: `proc_lock` (proc.c) protects every slot's state, tid, proc,
 * chan, futex, xstatus and killed, and the scheduler's choices. The trap
 * frame and the context belong to the thread itself while it runs. */
struct thread {
    u32 state;
    /* Thread ids come from the pids' counter, so no thread has another
     * process's pid as its id; a main thread's is its process's pid. 0
     * until the thread is made runnable. */
    u32 tid;
    /* The process it belongs to. */
    struct proc *proc;
    /* One page while the thread can run: the trap frame takes its top
     * TRAP_FRAME_SIZE bytes, and the kernel's stack for this thread grows
     * down from the frame (a kernel process's thread has no frame: its
     * stack starts at the top of the page). Freed by the scheduler as soon
     * as the thread has ended and switched away for good, so an ended
     * thread waiting to be joined costs no page. */
    void *kstack;
    /* The user registers while the thread is in the kernel: user_vec saves
     * them here (sscratch points at it in U-mode) and user_return loads them
     * back. NULL for a kernel process. */
    struct trap_frame *frame;
    /* Where swtch() resumes the thread. */
    struct context context;
    /* What a THREAD_SLEEPING thread waits for: any address that stands for
     * the event, such as the disk or the buffer it wants, a child's parent,
     * the tick counter, a thread it joins, or its own `futex` field while it
     * waits on a futex (wakeup() compares them). */
    const void *chan;
    /* The user address futex_wait() waits on, while it does. */
    u32 futex;
    /* thread_exit()'s status, for thread_join(), once it is a zombie. */
    int xstatus;
    /* A socket call's reference to the open file it works on (socket.c),
     * which syscall() lets go of once the call is done. */
    struct file *net_file;
    /* Set when another thread ends the process, or replaces its program:
     * the thread ends (proc_killed() says so) the next time it would return
     * to U-mode, and the long waits give up at once, as for kill(). */
    bool killed;
};

/* The kernel tests walk threads[] by this size and read the fields up to
 * `kstack` at their offsets (state 0, tid 4, proc 8, kstack 12). */
#define THREAD_SIZE 96
_Static_assert(sizeof(struct thread) == THREAD_SIZE, "the tests index threads[] by THREAD_SIZE");

/* The fields up to `parent` are at fixed offsets (state 0, pid 4, name 8,
 * parent 24): the kernel tests read them.
 *
 * Locking: `proc_lock` protects every slot's state, pid, name, parent,
 * xstatus, nthreads, killed and exiting. `files_lock` protects the
 * descriptors and the working directory, which any of the process's
 * threads may change (the fd_ functions below). `heap_lock`, a sleep lock,
 * makes sbrk() one thread at a time. The page table and the heap's bounds
 * change only under heap_lock, or while the process has one thread (exec);
 * others (ps) read them under proc_lock and may see them just before or
 * after a change. */
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
     * for a kernel process, which runs on the kernel page table. Every
     * thread of the process runs on it, possibly on several harts at once
     * (cpus[i].pagetable), which is why taking a mapping away shoots it down
     * on every hart that has it in satp (smp.h). */
    pagetable_t pagetable;
    /* The heap, from the page after the program's last segment: sbrk() moves
     * `brk` (one past its last byte) between `heap_base` and USER_LOAD_TOP.
     * Every page from heap_base up to brk is mapped. */
    u32 heap_base;
    u32 brk;
    /* The stack's lowest mapped address: every page from here up to
     * USER_TOP is mapped. It starts at USER_STACK and only moves down
     * (proc_grow_stack), never below USER_STACK_LIMIT. */
    u32 stack_low;
    /* sbrk() and stack growth one at a time: the heap's and the stack's
     * bounds and the page table's user mappings. */
    struct sleeplock heap_lock;
    /* A kernel process's function (NULL for a user process). */
    void (*kfn)(void);
    /* Its threads that haven't ended: 1 for a new process, + 1 for each
     * thread_create(), - 1 for each thread that ends. The process ends
     * with the last one. */
    u32 nthreads;
    /* Set by kill(): every thread of the process ends (exit status -1)
     * when it next would return to U-mode, and the system calls that sleep
     * for long (sleep, wait, thread_join, futex_wait, reading the console or
     * a pipe, writing a pipe, the sockets) give up at once. */
    bool killed;
    /* Set while one thread ends the process (exit, a fault, kill) or
     * replaces its program (exec) and waits for the others to end: they are
     * marked killed, and no new thread may start. */
    bool exiting;
    struct spinlock files_lock;
    /* Its file descriptors: ofile[fd] is the open file, or NULL (file.h). */
    struct file *ofile[NOFILE];
    /* Its working directory, where paths that don't start with `/` are
     * looked up from: an inode with a reference, or NULL when there is no
     * filesystem (and for kernel processes, which use the root). */
    struct inode *cwd;
};

/* The tests walk procs[] by this size (and read pagetable at 32). */
#define PROC_SIZE 168
_Static_assert(sizeof(struct proc) == PROC_SIZE, "the tests index procs[] by PROC_SIZE");

/* A user program embedded in the kernel image as an ELF executable
 * (userprogs.s, generated from kernel/user/ by `cargo xtask kernel`):
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
 * proc_exec_load() copy them to the top of the program's stack. */
struct proc_args {
    u32 argc;
    u32 len;
    char strings[ARG_MAX];
};

/* Sets up the process table's lock. kmain calls it before anything starts a
 * process. */
void proc_init(void);

/* The process of the thread on this hart (mycpu()->proc), or NULL while
 * the kernel runs for itself (kmain, the scheduler loop). */
struct proc *myproc(void);

/* The thread on this hart (mycpu()->thread), or NULL. */
struct thread *mythread(void);

/* Makes a runnable process called `name` from the executable `prog`, a
 * child of the current process, with one thread: a page table with the
 * program loaded, a stack holding `args` with sp below them, a kernel
 * stack, a pid, copies of the parent's descriptors 0, 1 and 2 (the others
 * are not passed on) and its working directory. It starts at the program's
 * entry point with a0 = argc, a1 = argv. Returns its pid, or -EAGAIN (no
 * free process or thread slot), -ENOMEM or -ENOEXEC (a bad ELF image: the
 * loader's reason is printed as "spawn: PATH: REASON", `path` being what
 * the caller asked for) when it can't; nothing is left allocated then.
 * Reading `prog` may sleep. */
int proc_create(const struct elf_file *prog, const char *path, const char *name, const struct proc_args *args);

/* A program loaded for exec but not yet running: its address space, where
 * its heap starts and its first registers. proc_exec_load() makes one and
 * proc_exec_commit() puts it in place of the current program (or
 * proc_exec_abandon() frees it). In two steps so that the caller can let go
 * of the executable (its inode lock and transaction, exec.c) in between:
 * the commit waits for the process's other threads to end, and one of them
 * may be waiting for that inode, or for room in the log, meanwhile. */
struct exec_image {
    pagetable_t pagetable;
    u32 heap;
    struct trap_frame frame;
};

/* Loads `prog` into a new address space as proc_create() does, for the
 * current process: registers all zero but pc, sp, a0 (argc) and a1
 * (argv). Returns 0, or -ENOMEM or -ENOEXEC ("exec: PATH: REASON") with
 * nothing allocated; the process is untouched either way. Reading `prog`
 * may sleep. */
int proc_exec_load(const struct elf_file *prog, const char *path, const struct proc_args *args,
                   struct exec_image *img);

/* Replaces the current process's program with `img`, called `name`. Every
 * other thread of the process ends first; the descriptors, the working
 * directory, the pid and the parent stay. Returns argc (the new program's
 * a0, which the system call leaves there); or -EINTR, with the old program
 * untouched and `img` freed, if another thread is ending the process
 * meanwhile. Must be called holding no sleep lock and no transaction: it
 * waits for threads that may need them to end. */
int proc_exec_commit(struct exec_image *img, const char *name, const struct proc_args *args);

/* Frees a loaded `img` that won't run. */
void proc_exec_abandon(struct exec_image *img);

/* Makes a runnable kernel process called `name` that runs `fn` in S-mode on
 * its own kernel stack, with the kernel page table and interrupts off (like
 * any thread's kernel code), and ends quietly when `fn` returns. It has no
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

/* Runs the threads round robin, one timer tick each for user threads,
 * taking them from the thread table that every hart's scheduler shares.
 * On hart 0, returns with interrupts on once no process is left; on the
 * other harts it never returns. While no thread is runnable it idles in
 * wfi. Called by proc_run_init() and by the boot self-tests that use kernel
 * processes, on hart 0, and by hart_main() on the others. */
void scheduler(void);

/* Puts the current thread to sleep on `chan` and gives up the CPU; returns
 * once wakeup(chan) has made it runnable and the scheduler picked it again.
 * The caller holds `lk`, the spinlock that protects the condition it waits
 * for (checked under `lk` before the call): sleep() takes proc_lock before
 * it lets go of `lk`, and wakeup() needs proc_lock too, so a wakeup that
 * comes after the check finds the thread asleep and none is lost. `lk` is
 * held again when sleep() returns. Callers check their condition in a loop
 * around it: a wakeup only says it may have changed. Must be called from a
 * thread's kernel code, holding no other spinlock than `lk`. */
void sleep(const void *chan, struct spinlock *lk);

/* Makes every thread sleeping on `chan` runnable. Callable anywhere,
 * interrupt handlers included, except with proc_lock held; callers hold the
 * lock that protects what changed, the one the sleepers passed to
 * sleep(). */
void wakeup(const void *chan);

/* Gives the CPU back to the scheduler; the thread stays runnable. */
void yield(void);

/* Ends the current process with `status`, from any of its threads: every
 * other thread is told to end and the caller waits until they have; then
 * it closes the files, lets go of the working directory, becomes a zombie
 * until its parent waits for it (the scheduler frees it if it has none),
 * its parent is woken, and its own children are orphaned (zombies among
 * them freed). If another thread is already ending the process, the caller
 * just ends. Never returns. */
void proc_exit(int status);

/* Kills the current process for the trap in `f`, a fault in U-mode in one
 * of its threads: prints why (the cause, scause/sepc/stval and, for a page
 * fault, how the process's page table maps stval), then exits it with
 * status -1. Never returns. */
void proc_kill(struct trap_frame *f);

/* kill(): marks the process `pid` killed (see `killed` in struct proc) and
 * wakes its threads that sleep. 0 (also for a zombie, which has already
 * ended), -ESRCH if there is no such process, -EPERM for a kernel process
 * or the first one (the shell). */
int proc_kill_pid(int pid);

/* Whether the current thread has been killed, or its process. */
bool proc_killed(void);

/* procs(): describes up to `max` processes, in pid order, in `out` and
 * returns how many there are in all. */
int proc_list(struct procinfo *out, int max);

/* harts(): describes up to `max` of the harts running the kernel, in hart
 * order, in `out` and returns how many there are in all. */
int proc_harts(struct hartinfo *out, int max);

/* wait(): waits for a child of the current process to end, any child if
 * `pid` is -1, frees it and returns its pid, storing its exit status at user
 * address `status` unless that is 0. -ECHILD if there is no such child,
 * -EFAULT if `status` is not writable user memory. */
int proc_wait(int pid, u32 status);

/* Grows the current process's stack down to the page holding `va`, mapping
 * zeroed pages from its lowest mapped one (stack_low) down: the stack is
 * mapped on demand, as Linux's is, so a program pays in RAM only for the
 * stack it uses. Called on a page fault in the stack's range (trap.c) and at
 * every system call down to the caller's sp (syscall.c), since the kernel
 * reads and writes user memory through the page table, never by faulting.
 * 0 when the page is mapped (already, or now), -EFAULT if `va` is not in
 * [USER_STACK_LIMIT, USER_TOP), -ENOMEM if a page can't be had (the pages
 * mapped so far stay). */
int proc_grow_stack(u32 va);

/* sbrk(): moves the end of the current process's heap by `increment` bytes
 * and returns where it was. New heap pages read zero; pages wholly above the
 * new end are freed when it shrinks. -ENOMEM if the heap would reach
 * USER_LOAD_TOP or a page can't be had (nothing changes then), -EINVAL if it
 * would shrink below its start. */
int proc_sbrk(i32 increment);

/* thread_create(): a new thread in the current process, starting at user
 * address `entry` with a0 = `arg`, sp = `stack` and every other register
 * zero. Returns its tid; -EFAULT if `entry` is not executable user memory
 * or the 16 bytes below `stack` are not writable; -EINVAL if `stack` is not
 * 16-byte aligned; -EAGAIN if every thread slot is taken (or the process is
 * ending), -ENOMEM. */
int thread_create(u32 entry, u32 arg, u32 stack);

/* thread_exit(): ends the current thread with `status`, which a
 * thread_join() collects. The last thread of a process ends the process
 * too, with that status, as exit() would. Never returns. */
void thread_exit(int status);

/* thread_join(): waits for thread `tid` of the current process to end,
 * frees its slot and stores its status at user address `status` unless
 * that is 0. 0; -ESRCH if the process has no such thread (or it has been
 * joined), -EINVAL for the caller itself, -EFAULT if `status` is not
 * writable, -EINTR if the caller is killed while it waits. */
int thread_join(int tid, u32 status);

/* futex_wait(): if the word at user address `addr` still holds `val`,
 * sleeps until a futex_wake() on the same address (0), else returns
 * -EAGAIN at once. The check and the sleep are one step as far as
 * futex_wake() can see, so a wake that comes after the value changed is
 * never lost. -EINVAL if `addr` is not 4-byte aligned, -EFAULT if it is not
 * readable, -EINTR if the caller is killed while it waits. */
int futex_wait(u32 addr, u32 val);

/* futex_wake(): wakes up to `n` threads of the current process waiting in
 * futex_wait() on user address `addr`, and returns how many it woke.
 * -EINVAL if `addr` is not 4-byte aligned, -EFAULT if it is not readable. */
int futex_wake(u32 addr, i32 n);

/* The current process's descriptors, which any of its threads may change
 * meanwhile (spec section 10, "Kernel threads"). */

/* The open file behind descriptor `fd`, with a reference of the caller's
 * own (filedup), so that it stays open even if another thread closes `fd`;
 * the caller lets go of it with fileclose(). NULL if `fd` is not open. */
struct file *fd_get(u32 fd);

/* Gives `f` the lowest free descriptor: the descriptor, which now holds the
 * caller's reference to `f`, or -EMFILE when there is none (the caller
 * still holds its reference). */
int fd_install(struct file *f);

/* fd_install() for two files at once, or neither: the lowest two free
 * descriptors, in fds[0] (for `a`) and fds[1] (for `b`). 0 or -EMFILE. */
int fd_install2(struct file *a, struct file *b, int *fds);

/* Whether a descriptor is free: the calls that make a file check first, so
 * that nothing is made for want of one. Another thread may take it before
 * fd_install(), which then fails. */
bool fd_available(void);

/* Takes descriptor `fd` away and returns its file, whose reference passes
 * to the caller; NULL if `fd` is not open. */
struct file *fd_take(u32 fd);

/* fd_take() only if `fd` still holds `f`: true if it did, its reference
 * now the caller's. For taking back a descriptor just installed, which
 * another thread may have closed (or reused) meanwhile. */
bool fd_take_if(u32 fd, struct file *f);

/* The working directory, with a reference of the caller's own (idup), or
 * NULL if there is none. */
struct inode *proc_cwd(void);

/* Makes `ip` (whose reference passes to the process) the working directory
 * and returns the old one, whose reference passes to the caller, or NULL. */
struct inode *proc_set_cwd(struct inode *ip);

#endif
