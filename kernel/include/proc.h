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
#define NPROC 32

/* Thread slots, for every process together: each process's main thread,
 * and as many more. A thread costs a slot and, while it runs, a page (its
 * kernel stack and trap frame); an ended one waiting for thread_join() only
 * its slot. */
#define NTHREAD 64

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
struct poll_table;

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
    /* Its kernel stack while the thread can run (KSTACK_SIZE bytes from
     * kstack_alloc(), vm.h): the trap frame takes its top TRAP_FRAME_SIZE
     * bytes, and the kernel's stack for this thread grows down from the
     * frame (a kernel process's thread has no frame: its stack starts at
     * the top). Freed by the scheduler as soon as the thread has ended and
     * switched away for good, so an ended thread waiting to be joined costs
     * no memory. */
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
    uintptr futex;
    /* thread_exit()'s status, for thread_join(), once it is a zombie. */
    int xstatus;
    /* A socket call's reference to the open file it works on (socket.c),
     * which syscall() lets go of once the call is done. */
    struct file *net_file;
    /* Its poll table (poll.h) while it is in poll(): wakeup() of any of
     * its channels wakes it. NULL otherwise. */
    struct poll_table *poll;
    /* Set when another thread ends the process, or replaces its program:
     * the thread ends (proc_killed() says so) the next time it would return
     * to U-mode, and the long waits give up at once, as for kill(). */
    bool killed;
};

/* The kernel tests walk threads[] by THREAD_SIZE and read the fields up to
 * `kstack` at their offsets: state 0, tid 4, then the pointers, a
 * register wide (proc at THREAD_PROC, kstack at THREAD_KSTACK). */
#define THREAD_PROC 8
#define THREAD_KSTACK 16
#define THREAD_SIZE 192
_Static_assert(sizeof(struct thread) == THREAD_SIZE, "the tests index threads[] by THREAD_SIZE");
_Static_assert(offsetof(struct thread, proc) == THREAD_PROC, "the tests read proc there");
_Static_assert(offsetof(struct thread, kstack) == THREAD_KSTACK, "the tests read kstack there");

/* The fields up to `parent` are at fixed offsets (state 0, pid 4, name 8,
 * parent 24), and the page table at PROC_PAGETABLE: the kernel tests read
 * them.
 *
 * Locking: `proc_lock` protects every slot's state, pid, name, parent,
 * xstatus, nthreads, killed and exiting. `files_lock` protects the
 * descriptors and the working directory, which any of the process's
 * threads may change (the fd_ functions below). `heap_lock`, a sleep lock,
 * makes sbrk(), the stack's growth, mmap, munmap and mprotect one thread at
 * a time; `vm_lock`, a spinlock, guards the mappings for the page faults
 * that map their pages (mmap.h). The heap's bounds and the mappings change
 * only under heap_lock, or while the process has one thread (exec); so does
 * the page table, but for a fault adding a page of a mapping (under vm_lock
 * alone, every page the kernel maps for the process being under it);
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
     * Every page from heap_base up to brk is mapped. User addresses, a
     * register wide (Linux's mm_struct start_brk and brk). */
    uintptr heap_base;
    uintptr brk;
    /* The stack's lowest mapped address: every page from here up to
     * USER_TOP is mapped. It starts at USER_STACK and only moves down
     * (proc_grow_stack), never below USER_STACK_LIMIT. */
    uintptr stack_low;
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
    /* The descriptors that close on exec (FD_CLOEXEC), bit fd for ofile[fd]:
     * Linux's close_on_exec bitmap. A free descriptor's bit is clear. */
    u64 cloexec;
    /* Its memory mappings (mmap.h), in address order: changed under both
     * heap_lock and vm_lock, read under either. */
    struct vm_area *areas;
    /* The area list, and every page the kernel maps in user space while
     * the process runs: a page fault maps an area's page with only this
     * spinlock, which keeps it from making a page table another mapping
     * makes at the same time (mmap.h, "Locking"). */
    struct spinlock vm_lock;
    /* Who it runs as (docs/camelot-syscalls.md, "Users and permissions"):
     * the real user and group ids, who started it, and the effective ones,
     * which the permission checks use (fs.c's inode_permission). They are
     * the same but in a program whose file has the set-user-ID or
     * set-group-ID bit, which exec() gives the file's owner's or group's
     * as its effective ids. 0 is root. The first process and the kernel's
     * are root; spawn() gives the child its parent's. setuid() and
     * setgid() change them, from any of the process's threads: each is a
     * word that the others read without a lock, so a check made at the
     * same moment sees the ids from just before or just after. */
    u32 uid;
    u32 euid;
    u32 gid;
    u32 egid;
};

_Static_assert(NOFILE <= 64, "a bit of proc.cloexec for each descriptor");

/* The tests walk procs[] by PROC_SIZE, and read the page table at
 * PROC_PAGETABLE. */
#define PROC_PAGETABLE 40
#define PROC_SIZE 736
_Static_assert(sizeof(struct proc) == PROC_SIZE, "the tests index procs[] by PROC_SIZE");
_Static_assert(offsetof(struct proc, parent) == 24, "the tests read parent there");
_Static_assert(offsetof(struct proc, pagetable) == PROC_PAGETABLE, "the tests read pagetable there");

/* A user program embedded in the kernel image as an ELF executable
 * (userprogs.s, generated from kernel/user/ by `cargo xtask kernel`):
 * what spawn and exec run when there is no root filesystem (exec.c). Its
 * entries are 24 bytes (two pointers and the size, padded to a pointer's
 * alignment): the generator writes them so. */
struct user_program {
    const char *name;
    const u8 *elf;
    u32 size;
};
_Static_assert(sizeof(struct user_program) == 24, "userprogs.s lays out 24-byte entries");

extern const struct user_program user_programs[];
extern const u32 user_program_count;

/* The embedded program called `name`, or NULL. */
const struct user_program *user_program_find(const char *name);

/* A new program's arguments and environment, in kernel memory, as Linux's
 * struct linux_binprm gathers them for execve: `argc` NUL-terminated
 * argument strings one after another, then `envc` environment strings
 * (`NAME=value`), `len` bytes in all, at the end of `strings`. They are
 * put there as Linux's copy_strings puts them on the new stack, from the
 * top down (bprm->p): each string below the ones before it, envp's last
 * first and argv's first last (proc_args_push()), so that they end up in
 * the order they will have on the program's stack. proc_create() and
 * proc_exec_load() copy them to the top of it. */
struct proc_args {
    u32 argc;
    u32 envc;
    u32 len;
    char strings[ARG_MAX];
};

/* The lowest of `args`'s strings, argv[0]'s: they take the last `len`
 * bytes of `strings`. */
static inline const char *proc_args_start(const struct proc_args *args) {
    return args->strings + ARG_MAX - args->len;
}

/* Makes room for a string of `n` bytes, its NUL included, below those
 * `args` holds, which leave room for it, and returns where it goes. */
static inline char *proc_args_push(struct proc_args *args, u32 n) {
    args->len += n;
    return args->strings + ARG_MAX - args->len;
}

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
 * program loaded, a stack holding `args` (its arguments and environment)
 * with sp below them, a kernel stack, a pid, copies of the parent's
 * descriptors 0, 1 and 2 (the others are not passed on) and its working
 * directory. It starts at the program's entry point, sp pointing at argc,
 * argv, envp and the auxiliary vector (proc.c, "The program's start"),
 * `path` the vector's AT_EXECFN. Returns its pid, or -EAGAIN (no
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
    uintptr heap;
    u32 euid; /* the effective ids it runs with (elf_file's) */
    u32 egid;
    struct trap_frame frame;
};

/* Loads `prog` into a new address space as proc_create() does, for the
 * current process: registers all zero but pc and sp, and the program's
 * start on its stack (argc, argv, envp, the auxiliary vector: proc.c). Returns 0, or -ENOMEM or -ENOEXEC ("exec: PATH: REASON") with
 * nothing allocated; the process is untouched either way. Reading `prog`
 * may sleep. */
int proc_exec_load(const struct elf_file *prog, const char *path, const struct proc_args *args,
                   struct exec_image *img);

/* Replaces the current process's program with `img`, called `name`. Every
 * other thread of the process ends first; the descriptors, the working
 * directory, the pid and the parent stay. Returns 0 (the new program's
 * a0, which the system call leaves there: the psABI's atexit function,
 * none); or -EINTR, with the old program
 * untouched and `img` freed, if another thread is ending the process
 * meanwhile. Must be called holding no sleep lock and no transaction: it
 * waits for threads that may need them to end. */
int proc_exec_commit(struct exec_image *img, const char *name);

/* Frees a loaded `img` that won't run. */
void proc_exec_abandon(struct exec_image *img);

/* Makes a runnable kernel process called `name` that runs `fn` in S-mode on
 * its own kernel stack, with the kernel page table and interrupts off (like
 * any thread's kernel code), and ends quietly when `fn` returns. It has no
 * parent: the scheduler frees it. Returns NULL with *err saying why when it
 * can't (no free slot, no memory). */
struct proc *kproc_create(const char *name, void (*fn)(void), const char **err);

/* Starts the program at `path` (init) as the first process, with argv =
 * { name } and the console as its descriptors 0 (for reading), 1 and 2
 * (for writing), in the root directory if there is a filesystem, and runs
 * the scheduler until no process is left: the first process has exited and
 * so has everything still running when it did. The process loads its
 * program itself (exec.c's exec_init(), which may sleep on the disk); if
 * it can't, it runs `fallback_path` (the shell) as `fallback_name`, saying
 * why unless `path` is missing, as Linux falls back on /bin/sh. While the
 * first process runs `path`, it adopts the orphans: a process whose parent
 * ends becomes its child, for its wait() to collect. Returns the first
 * process's exit status in *status, or false if it couldn't be started
 * (having said why). */
bool proc_run_init(const char *path, const char *name, const char *fallback_path, const char *fallback_name,
                   int *status);

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

/* poll()'s waiting (poll.h, poll.c), on the table `pt` on the caller's
 * stack. poll_begin() makes it the thread's table, empty; poll_sleep()
 * sleeps until a wakeup() of one of its channels, unless one came since
 * the table was last emptied or the process was killed, then empties it
 * for the next scan; poll_end() takes it away. The table's channels go
 * in with poll_wait(). */
void poll_begin(struct poll_table *pt);
void poll_sleep(struct poll_table *pt);
void poll_end(void);

/* Threads in poll() now: whoever skips a wakeup() while nobody sleeps on
 * its channel (net_wakeup) must not skip it while this is non-zero. */
extern volatile u32 polling_threads;

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

/* Whether a process `pid` is running (neither ended nor being made). */
bool proc_exists(int pid);

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
int proc_wait(int pid, uintptr status);

/* Grows the current process's stack down to the page holding `va`, mapping
 * zeroed pages from its lowest mapped one (stack_low) down: the stack is
 * mapped on demand, as Linux's is, so a program pays in RAM only for the
 * stack it uses. Called on a page fault in the stack's range (trap.c) and at
 * every system call down to the caller's sp (syscall.c), since the kernel
 * reads and writes user memory through the page table, never by faulting.
 * 0 when the page is mapped (already, or now), -EFAULT if `va` is not in
 * [USER_STACK_LIMIT, USER_TOP), -ENOMEM if a page can't be had (the pages
 * mapped so far stay). */
int proc_grow_stack(uintptr va);

/* sbrk(): moves the end of the current process's heap by `increment` bytes
 * and returns where it was, a user address (a pointer, in the program). New
 * heap pages read zero; pages wholly above the new end are freed when it
 * shrinks. -ENOMEM if the heap would reach USER_LOAD_TOP or a page can't be
 * had (nothing changes then), -EINVAL if it would shrink below its start.
 * An address and an error can't be confused: user addresses are below
 * USER_TOP, and the errors are the last 4095 values of a long (as Linux's
 * IS_ERR_VALUE). */
long proc_sbrk(long increment);

/* thread_create(): a new thread in the current process, starting at user
 * address `entry` with a0 = `arg`, sp = `stack` and every other register
 * zero. Returns its tid; -EFAULT if `entry` is not executable user memory
 * or the 16 bytes below `stack` are not writable; -EINVAL if `stack` is not
 * 16-byte aligned; -EAGAIN if every thread slot is taken (or the process is
 * ending), -ENOMEM. */
int thread_create(uintptr entry, uintptr arg, uintptr stack);

/* thread_exit(): ends the current thread with `status`, which a
 * thread_join() collects. The last thread of a process ends the process
 * too, with that status, as exit() would. Never returns. */
void thread_exit(int status);

/* thread_join(): waits for thread `tid` of the current process to end,
 * frees its slot and stores its status at user address `status` unless
 * that is 0. 0; -ESRCH if the process has no such thread (or it has been
 * joined), -EINVAL for the caller itself, -EFAULT if `status` is not
 * writable, -EINTR if the caller is killed while it waits. */
int thread_join(int tid, uintptr status);

/* futex_wait(): if the 32-bit word at user address `addr` still holds `val`
 * (a futex is 32 bits, as Linux's),
 * sleeps until a futex_wake() on the same address (0), else returns
 * -EAGAIN at once. The check and the sleep are one step as far as
 * futex_wake() can see, so a wake that comes after the value changed is
 * never lost. -EINVAL if `addr` is not 4-byte aligned, -EFAULT if it is not
 * readable, -EINTR if the caller is killed while it waits. */
int futex_wait(uintptr addr, u32 val);

/* futex_wake(): wakes up to `n` threads of the current process waiting in
 * futex_wait() on user address `addr`, and returns how many it woke.
 * -EINVAL if `addr` is not 4-byte aligned, -EFAULT if it is not readable. */
int futex_wake(uintptr addr, i32 n);

/* The current process's descriptors, which any of its threads may change
 * meanwhile (spec section 10, "Kernel threads"). */

/* The open file behind descriptor `fd`, with a reference of the caller's
 * own (filedup), so that it stays open even if another thread closes `fd`;
 * the caller lets go of it with fileclose(). NULL if `fd` is not open. */
struct file *fd_get(u32 fd);

/* Gives `f` the lowest free descriptor: the descriptor, which now holds the
 * caller's reference to `f`, or -EMFILE when there is none (the caller
 * still holds its reference). The descriptor stays open across exec. */
int fd_install(struct file *f);

/* fd_install() at the lowest free descriptor from `lowest` up (below
 * NOFILE), closing on exec if `cloexec`: open with O_CLOEXEC, fcntl's
 * F_DUPFD and F_DUPFD_CLOEXEC. */
int fd_install_from(struct file *f, u32 lowest, bool cloexec);

/* fd_install() for two files at once, or neither: the lowest two free
 * descriptors, in fds[0] (for `a`) and fds[1] (for `b`). 0 or -EMFILE. */
int fd_install2(struct file *a, struct file *b, int *fds);

/* Descriptor `fd`'s flags, FD_CLOEXEC or 0, or -EBADF if it is not open:
 * fcntl's F_GETFD. */
int fd_flags(u32 fd);

/* Sets descriptor `fd`'s flags to `flags` (FD_CLOEXEC or 0): 0, or -EBADF
 * if it is not open. fcntl's F_SETFD. */
int fd_set_flags(u32 fd, u32 flags);

/* Closes every descriptor that closes on exec: what exec does once the new
 * program replaces the old one (proc_exec_commit()). */
void fd_close_on_exec(void);

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

/* setuid(id) and setgid(id), Linux's without the saved ids: root (an
 * effective uid of 0) sets both the real and the effective id; any other
 * process may only set its effective id to its real one (giving up what a
 * set-user-ID program gave it). 0 or -EPERM; -EINVAL for -1, which is no
 * id. */
long proc_setuid(u32 id);
long proc_setgid(u32 id);

/* The working directory, with a reference of the caller's own (idup), or
 * NULL if there is none. */
struct inode *proc_cwd(void);

/* Makes `ip` (whose reference passes to the process) the working directory
 * and returns the old one, whose reference passes to the caller, or NULL. */
struct inode *proc_set_cwd(struct inode *ip);

#endif
