/* Processes, threads and the scheduler (spec section 10, "Kernel
 * processes" and "Kernel threads"), after xv6-riscv's proc.c.
 *
 * A process owns an address space, open files and a working directory; a
 * thread is what runs. Each thread of a user process has two sides: its
 * code in U-mode, and a kernel side on its own kernel stack that runs
 * whenever the thread traps. A kernel process has one thread, which only
 * has the kernel side. Each hart's scheduler loop runs on the stack it
 * booted with; swtch() moves between it and the threads' kernel sides. A
 * thread gives up the CPU only from the kernel: in yield() at the end of
 * its time slice (one timer tick) or for the yield system call, in sleep()
 * when it has to wait, or when it ends.
 *
 * Why two tables rather than xv6's one: a thread needs only registers, a
 * kernel stack and a state, while a process's address space and files are
 * shared by all its threads and must outlive any one of them, so they live
 * in different slots. A thread points at its process; nothing points the
 * other way, since every walk over a process's threads is a walk over the
 * whole (small) thread table under proc_lock anyway.
 *
 * Several harts (spec section 10, "Kernel SMP"): every hart runs the same
 * scheduler loop over the one thread table, which is the run queue, rather
 * than a queue of its own. With 16 slots and at most 8 harts, a search of
 * the table is a few dozen instructions, so per-hart queues would buy
 * nothing but the work of balancing them; and with one table a runnable
 * thread is taken by the first hart that looks, so no hart idles while
 * another has work waiting. A hart with nothing to run waits in wfi; when
 * a thread becomes runnable (new, woken up or killed in its sleep), the
 * hart that made it so wakes one idle hart for it with an IPI (kick_idle),
 * rather than leave it waiting for the idle hart's next tick. A thread is
 * RUNNING on at most one hart: the scheduler that picks it marks it so
 * under proc_lock, and only that hart switches back from it. Two threads of
 * one process may run on two harts at once, each with the process's page
 * table in its satp.
 *
 * Ending. A thread ends with thread_exit(), or because its process is
 * ending. The last thread to end ends the process: it closes the files,
 * lets go of the working directory, and the process becomes a zombie for
 * its parent's wait(). exit() (and a fault, and kill()) ends the whole
 * process from whichever thread gets there first: it marks every other
 * thread killed, waits until they have all ended, and then does the above.
 * An ended thread's kernel stack is freed by the scheduler as soon as the
 * thread has switched away from it for the last time; its slot, with its
 * status, stays until a thread_join() (or the end of the process) frees it.
 *
 * Locking (spec section 10, "Kernel locking"): one spinlock, proc_lock,
 * protects both tables, as in the x86 xv6 (xv6-riscv has one per process).
 * It is held across every switch between a thread and the scheduler:
 * whoever gives up the CPU (yield, sleep, ending) takes it and calls
 * sched(), and whoever the scheduler switches to lets go of it, either on
 * its way back out of sched() or, for a new thread, first thing in
 * thread_start(). So a thread's state and the switch away from it change
 * together, as far as any other hart can see: none can pick a thread that
 * is still running on its own kernel stack, and none can see an ended
 * thread before the scheduler has switched satp away from its page table
 * and freed its stack. sleep() takes it before letting go of the caller's
 * lock, which is how no wakeup is lost. It comes after every other lock but
 * the allocator's, the UART's and the user copies' in the lock order. */
#include "proc.h"
#include "elf.h"
#include "exec.h"
#include "fb.h"
#include "file.h"
#include "kalloc.h"
#include "kprintf.h"
#include "log.h"
#include "memlayout.h"
#include "mmap.h"
#include "poll.h"
#include "riscv.h"
#include "sbi.h"
#include "smp.h"
#include "spinlock.h"
#include "string.h"
#include "syscall.h"
#include "timer.h"

static struct proc procs[NPROC];
static struct thread threads[NTHREAD];
/* The end of the slots ever used: every one from here on is unused. The
 * loops that run on every tick (pick_next, wakeup_locked) stop there, so
 * that a machine running a few threads pays for those, not for NTHREAD.
 * It only grows, under proc_lock. */
static struct thread *threads_top = threads;

/* Both tables, the pids, the first process's status, and every switch
 * between a thread and a scheduler. */
static struct spinlock proc_lock;

/* The first process's pid, as Linux's init has it, whatever kernel
 * processes the boot's self-tests ran before it. */
#define INIT_PID 1

/* The next pid or tid: one counter for both, so that no thread's id is
 * another process's pid. It starts after INIT_PID. */
static u32 next_pid = INIT_PID + 1;

/* Where the next round-robin search starts: the slot after the thread that
 * ran last (NULL: slot 0). A pointer rather than an index, like the loops
 * that run on every tick (pick_next, wakeup_locked): merlin's unoptimized
 * code would multiply and divide on every step of an indexed one, and
 * with many slots that shows on a tick's cost. */
static struct thread *next_thread;

/* The first process (proc_run_init), the program it runs, the one it falls
 * back on, and its exit status once it ended. */
static struct proc *init_proc;
static const char *init_path;
static const char *init_name;
static const char *init_fallback_path;
static const char *init_fallback_name;
static int init_status;
/* Whether the first process collects orphans: while it runs the program it
 * was started with (init), as Linux's init does. Once it runs the fallback
 * (the shell) or execs another program, orphans are freed as they end. */
static bool init_reaps;

void proc_init(void) {
    initlock(&proc_lock, "proc");
}

/* No push_off() around the two reads, as xv6 has: a kernel thread is never
 * preempted, so it can't move to another hart in between. */
struct proc *myproc(void) {
    return mycpu()->proc;
}

struct thread *mythread(void) {
    return mycpu()->thread;
}

const struct user_program *user_program_find(const char *name) {
    for (u32 i = 0; i < user_program_count; i++) {
        if (strcmp(user_programs[i].name, name) == 0) {
            return &user_programs[i];
        }
    }
    return NULL;
}

/* A new user thread starts here, from the scheduler's swtch(), which still
 * holds proc_lock, and goes straight to U-mode at its entry point (a
 * program's, or thread_create()'s), unless it was killed before it got its
 * first turn: then it ends before running any of its code, as user_trap()
 * would end it on any later way back. */
static void thread_start(void) {
    release(&proc_lock);
    if (proc_killed()) {
        proc_exit(-1);
    }
    user_trap_return(mythread()->frame);
}

/* A new kernel process starts here, holding proc_lock as thread_start
 * does: runs its function, then ends. It has no parent, so the scheduler
 * frees it. */
static void kproc_start(void) {
    release(&proc_lock);
    myproc()->kfn();
    proc_exit(0);
}

/* Frees thread slot `t` and its kernel stack if it still has one. It must
 * not be running. Called with proc_lock held, or on a slot nobody else
 * looks at yet. */
static void thread_free(struct thread *t) {
    if (t->kstack != NULL) {
        kstack_free(t->kstack);
        t->kstack = NULL;
    }
    t->frame = NULL;
    t->proc = NULL;
    t->chan = NULL;
    t->poll = NULL;
    t->state = THREAD_UNUSED;
}

/* Releases everything process `p` holds (any of it may be missing) and
 * frees its slot. Its threads must be gone, and its page table must not be
 * in any hart's satp. Called with proc_lock held. */
static void proc_free(struct proc *p) {
    if (p->pagetable != NULL) {
        /* The mappings first: uvm_free() frees every page still mapped,
         * and a shared object's or a device's pages aren't the process's. */
        mm_release(p->pagetable, p->areas);
        p->areas = NULL;
        uvm_free(p->pagetable);
        p->pagetable = NULL;
    }
    p->parent = NULL;
    p->nthreads = 0;
    p->state = PROC_UNUSED;
}

/* Copies at most PROC_NAME_MAX characters of `name`, the rest of the field
 * zeros, as Linux's __set_task_comm pads with strscpy_pad: a name an exec
 * shortens ("init" to "sh") keeps nothing of the old one, which procs()
 * copies whole. Called with proc_lock held, or on a slot nobody else looks
 * at yet. */
static void set_name(struct proc *p, const char *name) {
    u32 i = 0;
    while (name[i] != 0 && i < PROC_NAME_MAX) {
        p->name[i] = name[i];
        i++;
    }
    while (i <= PROC_NAME_MAX) {
        p->name[i++] = 0;
    }
}

/* ---- The program's start ----
 *
 * What a new program finds at the top of its stack page, as Linux's
 * create_elf_tables (fs/binfmt_elf.c) puts it there for an ELF program and
 * the RISC-V psABI describes it ("Process Initialization"); from USER_TOP
 * down:
 *
 *   USER_TOP     a pointer-sized zero, Linux's end marker
 *                the path the program was started with (AT_EXECFN)
 *                the environment's strings, envp[0]'s first, each
 *                NUL-terminated
 *                the argument strings, argv[0]'s first (lowest), each
 *                NUL-terminated
 *                padding to a multiple of 16
 *                16 random bytes (AT_RANDOM)
 *                padding
 *                the auxiliary vector: AUXV_ENTRIES pairs of a type and a
 *                value, a pointer's size each, the last one AT_NULL
 *                envp[0] ... envp[envc - 1], NULL
 *                argv[0] ... argv[argc - 1], NULL
 *   sp           argc, a long
 *
 * Linux copies the strings down from the top in that order (the path,
 * then envp's, then argv's: copy_string_kernel and copy_strings in
 * fs/exec.c), each array's in its own order, so argv[0]'s string is the
 * lowest; create_elf_tables then fills the pointers in going up the
 * strings. Here they are in kernel memory already, put in from the top
 * down in that same order (`args`, syscall.c's copy_args): one after
 * another, argv's then envp's, they go up in one copy.
 *
 * sp, the lowest of it, is 16-byte aligned. a0 holds 0: the psABI's
 * address of a function for the program to register with atexit, the
 * dynamic linker's (ELF_PLAT_INIT on Linux), and there is none. Every other
 * register is zero too. crt0 finds argc, argv and envp from sp. */

/* The ISA the kernel runs programs on, as AT_HWCAP gives it, Linux's way: a
 * bit for each single-letter extension, 'X' - 'A' (Zicsr and Zifencei have
 * no letter). RV64IMA: the base, M and A. */
#define HWCAP_ISA(letter) (1ul << ((letter) - 'A'))
#define HWCAP (HWCAP_ISA('I') | HWCAP_ISA('M') | HWCAP_ISA('A'))

/* The auxiliary vector's entries push_start writes, AT_NULL's included. */
#define AUXV_ENTRIES 17
/* The most pointer-sized words below the random bytes: argc, argv's MAXARG
 * pointers and NULL, envp's as many, and the vector. */
#define START_WORDS (1 + (MAXARG + 1) + (MAXARG + 1) + 2 * AUXV_ENTRIES)
/* All of it fits in the stack's first page, with room to spare for the
 * program's first frames: the marker, the path, the strings (argv's and
 * envp's, ARG_MAX bytes at most together), the padding and the random
 * bytes, then the words and their padding. */
_Static_assert(sizeof(uintptr) + MAXPATH + 1 + ARG_MAX + 15 + 16 + sizeof(uintptr) * START_WORDS + 15 <= PGSIZE / 2,
               "a program's start fills more than half its stack page");

/* MurmurHash3's finalizer, which TCP's initial sequence numbers are mixed
 * with too (net/tcp.c): every bit of the result depends on every bit of h. */
static u32 mix32(u32 h) {
    h ^= h >> 16;
    h *= 0x85ebca6bu;
    h ^= h >> 13;
    h *= 0xc2b2ae35u;
    h ^= h >> 16;
    return h;
}

/* AT_RANDOM's 16 bytes, into `buf`. Linux takes them from its entropy pool
 * (get_random_bytes); Camelot has no source of entropy (no RNG device, and
 * Grail without --clock runs the same way every time), so these are the
 * time and `salt` (the new stack page's address, which no other running
 * program has), mixed: different for every program started, well spread,
 * not secret. */
static void random_bytes(u8 *buf, uintptr salt) {
    u64 now = r_time();
    u32 h = mix32((u32)now ^ (u32)salt);
    for (u32 i = 0; i < 4; i++) {
        h = mix32(h + 0x9e3779b9u * (i + 1) + (u32)(now >> 32) + (u32)((u64)salt >> 32));
        memcpy(buf + 4 * i, &h, 4);
    }
}

/* Puts `args`, `path` and `prog`'s auxiliary vector at the top of the stack
 * page `stack` (mapped at USER_STACK), as above, and sets frame `f`'s sp. */
static void push_start(struct trap_frame *f, u8 *stack, const struct proc_args *args, const char *path,
                       const struct elf_loaded *prog) {
    /* The page holds the user addresses [USER_STACK, USER_TOP). It is
     * zeroed: the end marker, the padding and a0 are zero already. */
    uintptr at = USER_TOP - sizeof(uintptr);
    usize path_len = strlen(path) + 1;
    at -= path_len;
    uintptr execfn = at;
    memcpy(stack + (at - USER_STACK), path, path_len);
    at -= args->len;
    uintptr strings = at;
    memcpy(stack + (at - USER_STACK), proc_args_start(args), args->len);
    at = (at & ~(uintptr)15) - 16;
    uintptr random = at;
    random_bytes(stack + (at - USER_STACK), pa(stack));

    u32 words = 1 + (args->argc + 1) + (args->envc + 1) + 2 * AUXV_ENTRIES;
    uintptr sp = (random - sizeof(uintptr) * words) & ~(uintptr)15;
    uintptr *w = (uintptr *)(stack + (sp - USER_STACK));
    u32 n = 0;
    w[n++] = args->argc;
    /* argv's pointers, then envp's, up the strings: each string starts
     * where the one before it ended. */
    uintptr s = strings;
    for (u32 i = 0; i < args->argc; i++) {
        w[n++] = s;
        s += strlen(proc_args_start(args) + (s - strings)) + 1;
    }
    w[n++] = 0; /* argv's NULL */
    for (u32 i = 0; i < args->envc; i++) {
        w[n++] = s;
        s += strlen(proc_args_start(args) + (s - strings)) + 1;
    }
    w[n++] = 0; /* envp's */
    /* The vector, in Linux's order (the RISC-V entries it puts first,
     * AT_SYSINFO_EHDR for the vDSO, the caches' sizes and AT_MINSIGSTKSZ,
     * have nothing to say here: no vDSO, no cache information, no signals). */
    uintptr auxv[2 * AUXV_ENTRIES] = {
        AT_HWCAP,  HWCAP,
        AT_PAGESZ, PGSIZE,
        AT_CLKTCK, TICK_HZ,
        AT_PHDR,   prog->phdr,
        AT_PHENT,  sizeof(elf_phdr),
        AT_PHNUM,  prog->phnum,
        AT_BASE,   0, /* no interpreter: a static program */
        AT_FLAGS,  0,
        AT_ENTRY,  prog->entry,
        AT_UID,    0,
        AT_EUID,   0,
        AT_GID,    0,
        AT_EGID,   0,
        AT_SECURE, 0,
        AT_RANDOM, random,
        AT_EXECFN, execfn,
        AT_NULL,   0,
    };
    for (u32 i = 0; i < 2 * AUXV_ENTRIES; i++) {
        w[n++] = auxv[i];
    }
    f->regs[REG_SP] = sp;
}

/* A free thread slot, cleared and THREAD_NEW, with a kernel stack, for
 * process `p`; NULL with *err set to -EAGAIN (no free slot) or -ENOMEM if
 * it can't. Nobody else looks at a THREAD_NEW slot: the caller sets it up
 * without proc_lock. */
static struct thread *thread_alloc(struct proc *p, int *err) {
    struct thread *t = NULL;
    acquire(&proc_lock);
    for (u32 i = 0; i < NTHREAD; i++) {
        if (threads[i].state == THREAD_UNUSED) {
            t = &threads[i];
            break;
        }
    }
    if (t == NULL) {
        release(&proc_lock);
        *err = -EAGAIN;
        return NULL;
    }
    if (t >= threads_top) {
        threads_top = t + 1;
    }
    memset(t, 0, sizeof *t);
    t->state = THREAD_NEW;
    t->proc = p;
    release(&proc_lock);
    t->kstack = kstack_alloc();
    if (t->kstack == NULL) {
        acquire(&proc_lock);
        thread_free(t);
        release(&proc_lock);
        *err = -ENOMEM;
        return NULL;
    }
    return t;
}

/* A free process slot, cleared, and its first thread (thread_alloc()), in
 * *tp; NULL with *err set to -EAGAIN (no free slot of either kind) or
 * -ENOMEM if it can't. The slot is PROC_NEW until proc_ready(), so that
 * nobody else takes it while proc_create() sleeps reading the program from
 * the disk, and nobody else looks at its fields: the caller sets them up
 * without proc_lock. */
static struct proc *proc_alloc(struct thread **tp, int *err) {
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
    initsleeplock(&p->heap_lock, "heap");
    initlock(&p->files_lock, "files");
    initlock(&p->vm_lock, "vm");
    *tp = thread_alloc(p, err);
    if (*tp == NULL) {
        acquire(&proc_lock);
        proc_free(p);
        release(&proc_lock);
        return NULL;
    }
    return p;
}

/* proc_free() and thread_free() for a process the caller took and never
 * made runnable, and its thread. */
static void proc_unalloc(struct proc *p, struct thread *t) {
    acquire(&proc_lock);
    thread_free(t);
    proc_free(p);
    release(&proc_lock);
}

/* `n` threads have just become runnable: wakes up to `n` of the harts
 * that wait in the scheduler with nothing to run (not this one, which will
 * look again itself), with an IPI each. A hart woken for nothing (another
 * got there first) just waits again. Called with proc_lock held. */
static void kick_idle(u32 n) {
    u32 me = cpuid();
    u32 up = harts_running() & ~(1u << me);
    /* No other hart: the usual case on a one-hart machine, made cheap. */
    if (up == 0) {
        return;
    }
    u32 kick = 0;
    for (u32 i = 0; i < NCPU && n > 0; i++) {
        if ((up & (1u << i)) != 0 && cpus[i].idle) {
            /* Woken once: the next thread doesn't count on it too. */
            cpus[i].idle = false;
            kick |= 1u << i;
            n--;
        }
    }
    if (kick != 0) {
        sbi_send_ipi(kick, 0);
    }
}

/* Gives a fully set up `p` its pid (INIT_PID for the first process, the
 * next one for any other), name and parent, and its first thread `t` the
 * pid as its tid, and makes the thread runnable. Returns the pid. */
static int proc_ready(struct proc *p, struct thread *t, const char *name, struct proc *parent) {
    acquire(&proc_lock);
    u32 id = INIT_PID;
    if (p != init_proc) {
        id = next_pid;
        next_pid++;
    }
    int pid = (int)id;
    p->pid = id;
    t->tid = id;
    set_name(p, name);
    p->parent = parent;
    p->nthreads = 1;
    p->state = PROC_LIVE;
    t->state = THREAD_RUNNABLE;
    kick_idle(1);
    release(&proc_lock);
    return pid;
}

/* The trap frame at the top of `t`'s kernel stack page. */
static struct trap_frame *frame_of(struct thread *t) {
    return (struct trap_frame *)((u8 *)t->kstack + KSTACK_SIZE - TRAP_FRAME_SIZE);
}

/* A new user address space for `prog`: its segments and a zeroed stack
 * page with its start at the top (push_start: `args`, `path` and the
 * auxiliary vector). Returns its page table and sets *heap (the page after
 * the last segment) and frame `f`: all zero but sepc (the entry point) and
 * sp. Or returns NULL with *err = -ENOMEM, or -ENOEXEC
 * with "WHO: PATH: REASON" printed, leaving `f` alone and nothing
 * allocated. */
static pagetable_t build_space(const struct elf_file *prog, const char *who, const char *path,
                               const struct proc_args *args, struct trap_frame *f, uintptr *heap, int *err) {
    pagetable_t pt = uvm_create();
    if (pt == NULL) {
        *err = -ENOMEM;
        return NULL;
    }
    struct elf_loaded loaded;
    const char *load_err = elf_load(pt, prog, &loaded);
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
        map_pages(pt, USER_STACK, pa(stack), PGSIZE, PTE_U | PTE_R | PTE_W) != 0) {
        if (stack != NULL) {
            kfree(stack);
        }
        *err = -ENOMEM;
        uvm_free(pt);
        return NULL;
    }
    *heap = loaded.end;
    /* The user registers: all zero but pc and sp. */
    memset(f, 0, sizeof *f);
    f->sepc = loaded.entry;
    push_start(f, stack, args, path, &loaded);
    return pt;
}

int proc_create(const struct elf_file *prog, const char *path, const char *name, const struct proc_args *args) {
    int err;
    struct thread *t;
    struct proc *p = proc_alloc(&t, &err);
    if (p == NULL) {
        return err;
    }
    t->frame = frame_of(t);
    uintptr heap;
    p->pagetable = build_space(prog, "spawn", path, args, t->frame, &heap, &err);
    if (p->pagetable == NULL) {
        proc_unalloc(p, t);
        return err;
    }
    p->heap_base = heap;
    p->brk = heap;
    p->stack_low = USER_STACK;

    /* The thread's kernel side: its first swtch() lands in thread_start, on
     * the empty kernel stack below the frame. */
    t->context.ra = (uintptr)thread_start;
    t->context.sp = (uintptr)t->frame;
    /* What the child inherits: the standard descriptors, sharing their open
     * files (and offsets), and the working directory. A descriptor above 2
     * stays the parent's own, so a pipe end the parent keeps for itself
     * doesn't keep the pipe open in every child; so does one that closes on
     * exec, as spawn is a fork and an exec in one (posix_spawn closes it in
     * the child). Under the parent's files_lock, as another of its threads
     * may be closing them. */
    struct proc *parent = myproc();
    if (parent != NULL) {
        acquire(&parent->files_lock);
        for (u32 fd = 0; fd <= FD_STDERR; fd++) {
            if (parent->ofile[fd] != NULL && (parent->cloexec & (1ul << fd)) == 0) {
                p->ofile[fd] = filedup(parent->ofile[fd]);
            }
        }
        if (parent->cwd != NULL) {
            p->cwd = idup(parent->cwd);
        }
        release(&parent->files_lock);
    }
    return proc_ready(p, t, name, parent);
}

/* Marks every thread of `p` but `me` killed, and makes the sleeping ones
 * runnable so that they see it: each ends the next time it would return to
 * U-mode, and the long waits give up at once. Every sleep() is in a loop
 * that checks what it waits for, so an early wakeup is harmless. Called
 * with proc_lock held. */
static void kill_threads(struct proc *p, struct thread *me) {
    u32 woken = 0;
    for (u32 i = 0; i < NTHREAD; i++) {
        struct thread *t = &threads[i];
        if (t == me || t->proc != p || t->state == THREAD_UNUSED || t->state == THREAD_ZOMBIE) {
            continue;
        }
        t->killed = true;
        if (t->state == THREAD_SLEEPING) {
            t->state = THREAD_RUNNABLE;
            woken++;
        }
    }
    if (woken != 0) {
        kick_idle(woken);
    }
}

/* Has every other thread of the current process end, for proc_exit() and
 * proc_exec_commit(): marks the process `exiting`, so no new thread
 * starts, kills them, waits until the caller is the only one left, and
 * frees the slots of the ended ones (those that ended earlier and were
 * never joined too).
 * Returns false at once if another thread is already doing this: the
 * caller is about to be killed by it. Called with proc_lock held, which
 * the wait lets go of meanwhile, and with no sleep lock or transaction:
 * the other threads may need one to finish the calls they are in. */
static bool end_other_threads(void) {
    struct thread *me = mythread();
    struct proc *p = me->proc;
    if (p->exiting) {
        return false;
    }
    p->exiting = true;
    kill_threads(p, me);
    /* Each thread that ends wakes us (thread_end). */
    while (p->nthreads > 1) {
        sleep(&p->nthreads, &proc_lock);
    }
    /* An ended thread is only seen here once the scheduler has switched
     * away from it for good, under proc_lock: its stack is freed, and its
     * hart's satp is the kernel's again. */
    for (u32 i = 0; i < NTHREAD; i++) {
        struct thread *t = &threads[i];
        if (t != me && t->proc == p && t->state == THREAD_ZOMBIE) {
            thread_free(t);
        }
    }
    return true;
}

int proc_exec_load(const struct elf_file *prog, const char *path, const struct proc_args *args,
                   struct exec_image *img) {
    int err;
    /* The new registers go to the image: the old program's stay in the
     * frame until it is certain the new one replaces it. */
    img->pagetable = build_space(prog, "exec", path, args, &img->frame, &img->heap, &err);
    return img->pagetable == NULL ? err : 0;
}

void proc_exec_abandon(struct exec_image *img) {
    uvm_free(img->pagetable);
    img->pagetable = NULL;
}

int proc_exec_commit(struct exec_image *img, const char *name) {
    struct proc *p = myproc();
    /* No other thread runs the new program, and none may be using the old
     * address space when it goes. The others end on their way back to
     * U-mode, which one in the middle of a call reaches once the call is
     * done; a call may wait for a sleep lock or for room in the log, so the
     * caller holds neither (exec.c has closed the executable). */
    acquire(&proc_lock);
    bool alone = end_other_threads();
    if (alone) {
        set_name(p, name);
        p->exiting = false;
        if (p == init_proc) {
            init_reaps = false; /* init_start sets it again for init itself */
        }
    }
    release(&proc_lock);
    if (!alone) {
        proc_exec_abandon(img);
        return -EINTR;
    }
    pagetable_t old = p->pagetable;
    /* The old program's mappings go with its address space (Linux's exec
     * starts from an empty mm). */
    struct vm_area *old_areas = p->areas;
    p->areas = NULL;
    p->pagetable = img->pagetable;
    p->heap_base = img->heap;
    p->brk = img->heap;
    p->stack_low = USER_STACK;
    *mythread()->frame = img->frame;
    /* The new table holds the kernel's mappings too: switch to it before
     * freeing the old one, which satp points at. No other hart uses either:
     * the process has one thread now, this one. */
    mycpu()->pagetable = img->pagetable;
    fence_rw(); /* as in the scheduler */
    w_satp(MAKE_SATP(img->pagetable));
    sfence_vma();
    img->pagetable = NULL;
    if (old != NULL) {
        mm_release(old, old_areas);
        uvm_free(old);
    }
    /* The old program is gone: the descriptors it marked close-on-exec go
     * with it, as on Linux once exec is past its point of no return. The
     * caller holds no lock, so closing one may free an inode. */
    fd_close_on_exec();
    return 0;
}

struct proc *kproc_create(const char *name, void (*fn)(void), const char **err) {
    int alloc_err;
    struct thread *t;
    struct proc *p = proc_alloc(&t, &alloc_err);
    if (p == NULL) {
        *err = alloc_err == -EAGAIN ? "too many processes" : "out of memory";
        return NULL;
    }
    /* No user space, so no page table of its own and no trap frame: its first
     * swtch() lands in kproc_start, on the empty kernel stack. */
    p->kfn = fn;
    t->context.ra = (uintptr)kproc_start;
    t->context.sp = (uintptr)t->kstack + KSTACK_SIZE;
    proc_ready(p, t, name, NULL);
    return p;
}

/* The first process's thread starts here, from the scheduler (holding
 * proc_lock, as for thread_start): it gets the console as its descriptors
 * and the root as its working directory, loads its program (which may
 * sleep on the disk, so it has to be done by a process) and goes to
 * U-mode. */
static void init_start(void) {
    release(&proc_lock);
    struct proc *p = myproc();
    p->ofile[FD_STDIN] = file_console(true, false);
    p->ofile[FD_STDOUT] = file_console(false, true);
    p->ofile[FD_STDERR] = file_console(false, true);
    if (fs_mounted) {
        p->cwd = iget(fs_dev, ROOTINO);
    }
    /* Init from the filesystem only, when there is one: the embedded init
     * would look for its rc and its shell there, and not find them. */
    const char *path = init_path;
    int err = exec_init(path, init_name, init_fallback_path == NULL);
    if (err < 0 && init_fallback_path != NULL) {
        /* As Linux's kernel_init tries init, then sh: a missing init is no
         * error, one that is there and won't start is said. */
        if (err != -ENOENT) {
            kprintf("cannot start %s: error %d\n", path, -err);
        }
        path = init_fallback_path;
        err = exec_init(path, init_fallback_name, true);
    } else if (err == 0) {
        acquire(&proc_lock);
        init_reaps = true;
        release(&proc_lock);
    }
    if (err < 0) {
        kprintf("cannot start %s: error %d\n", path, -err);
        if (user_program_count == 0) {
            /* The disk kernel (kernel/kernel-disk.elf) has no programs of its
             * own to fall back on, like Linux without an initramfs: without
             * the shell on the root filesystem there is nothing to run. */
            if (!fs_mounted)
                panic("no root filesystem, and no programs built into the kernel");
            if (err == -ENOENT)
                panic("no shell in /bin, and no programs built into the kernel");
            /* The shell is there but did not start (not an ELF, out of
             * memory, ...): the line above gave the error. */
            panic("the shell in /bin cannot start, and no programs built into the kernel");
        }
        proc_exit(-1);
    }
    user_trap_return(mythread()->frame);
}

bool proc_run_init(const char *path, const char *name, const char *fallback_path, const char *fallback_name,
                   int *status) {
    int err;
    struct thread *t;
    struct proc *p = proc_alloc(&t, &err);
    if (p == NULL) {
        kprintf("cannot start %s: error %d\n", path, -err);
        return false;
    }
    /* No address space yet: until its exec, it runs on the kernel page
     * table, like a kernel process. */
    t->frame = frame_of(t);
    memset(t->frame, 0, sizeof *t->frame);
    t->context.ra = (uintptr)init_start;
    t->context.sp = (uintptr)t->frame;
    acquire(&proc_lock);
    init_path = path;
    init_name = name;
    init_fallback_path = fallback_path;
    init_fallback_name = fallback_name;
    init_proc = p;
    init_reaps = false;
    release(&proc_lock);
    proc_ready(p, t, name, NULL);
    scheduler();
    acquire(&proc_lock);
    *status = init_status;
    release(&proc_lock);
    return true;
}

/* The next runnable thread after the last one that ran, or NULL. Called
 * with proc_lock held. */
static struct thread *pick_next(void) {
    struct thread *end = threads_top;
    struct thread *t = next_thread == NULL || next_thread >= end ? threads : next_thread;
    for (struct thread *n = threads; n != end; n++) {
        if (t->state == THREAD_RUNNABLE) {
            next_thread = t + 1 == end ? threads : t + 1;
            return t;
        }
        t++;
        if (t == end) {
            t = threads;
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

/* After thread `t` switched back to the scheduler for the last time, ended:
 * its kernel stack can go now that nothing runs on it. If it was the last
 * thread of its process, the process has ended too, and nobody will join
 * the thread: its slot goes, and so does the process if it is an orphan
 * (a zombie with a parent waits for it). Called with proc_lock held. */
static void reap_thread(struct thread *t) {
    kstack_free(t->kstack);
    t->kstack = NULL;
    t->frame = NULL;
    struct proc *p = t->proc;
    if (p->state == PROC_ZOMBIE) {
        thread_free(t);
        if (p->parent == NULL) {
            proc_free(p);
        }
    }
}

void scheduler(void) {
    struct cpu *c = mycpu();
    /* proc_create() fills the lowest free slots, so the processes start in
     * the order they were created. (Only hart 0 starts the loop with
     * processes to run, kmain's; the others start it once, before any.) */
    if (c->hartid == 0) {
        acquire(&proc_lock);
        next_thread = threads;
        release(&proc_lock);
    }
    for (;;) {
        /* Let pending interrupts in (a byte typed on the console, a tick
         * that came while the last thread was in the kernel, a disk's
         * completion) before going on with interrupts off. Their handlers
         * may wake threads up. Interrupts stay off from here on: the
         * acquire() below finds them off, so its release() leaves them
         * off. */
        intr_on();
        intr_off();
        acquire(&proc_lock);
        c->idle = false;
        struct thread *t = pick_next();
        if (t == NULL) {
            bool left = any_process();
            /* From here to the wfi, a thread that becomes runnable gets
             * this hart an IPI, which ends the wait (or keeps it from
             * starting: wfi doesn't wait with an interrupt pending). */
            c->idle = true;
            release(&proc_lock);
            /* Hart 0 goes back to kmain once every process has ended; the
             * others stay for good, as there may be processes again. */
            if (!left && c->hartid == 0) {
                c->idle = false;
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
        t->state = THREAD_RUNNING;
        c->thread = t;
        c->proc = t->proc;
        c->switches = c->switches + 1;
        /* Its page table holds the kernel's mappings too, so the kernel
         * carries on as before; from the next sret on, U-mode sees the
         * process's memory. */
        c->pagetable = proc_pagetable(t->proc);
        /* Seen by a hart shooting this table down before this hart walks
         * it (smp.c). */
        fence_rw();
        w_satp(MAKE_SATP(c->pagetable));
        sfence_vma();
        /* proc_lock stays held across the switch: the thread lets go of
         * it (thread_start, or the end of yield, sleep), and takes it again
         * before it switches back here (sched). */
        swtch(&c->context, &t->context);
        /* Back when it yields, sleeps or ends. */
        c->pagetable = kernel_pagetable;
        w_satp(MAKE_SATP(kernel_pagetable));
        sfence_vma();
        c->thread = NULL;
        c->proc = NULL;
        if (t->state == THREAD_ZOMBIE) {
            reap_thread(t);
        }
        release(&proc_lock);
        /* The kernel stacks freed holding proc_lock, here or elsewhere, that
         * this hart's cache had no room for: unmapped now that no lock is
         * held (vm.h). And a framebuffer's buffer in RAM, let go of by a
         * process reaped with it mapped (fb.h). */
        kstack_reclaim();
        fb_reclaim();
    }
    intr_on();
}

/* Back to the scheduler, which resumes this thread here when its turn
 * comes again. The caller holds proc_lock and no other spinlock, and has
 * changed the thread's state. `intena` belongs to this thread rather than
 * to the hart, so it is saved and restored around the switch (another
 * thread may run on the hart meanwhile). */
static void sched(void) {
    struct thread *t = mythread();
    struct cpu *c = mycpu();
    if (!holding(&proc_lock)) {
        panic("sched: proc_lock not held");
    }
    if (c->noff != 1) {
        panic("sched: holding a lock other than proc_lock");
    }
    if (t->state == THREAD_RUNNING) {
        panic("sched: the thread is still running");
    }
    if ((r_sstatus() & SSTATUS_SIE) != 0) {
        panic("sched: interrupts are on");
    }
    u32 intena = c->intena;
    swtch(&t->context, &c->context);
    mycpu()->intena = intena;
}

void yield(void) {
    acquire(&proc_lock);
    mythread()->state = THREAD_RUNNABLE;
    sched();
    release(&proc_lock);
}

void sleep(const void *chan, struct spinlock *lk) {
    struct thread *t = mythread();
    /* proc_lock first, then let go of lk: a wakeup(chan) needs proc_lock,
     * so it can't run between the caller's check (under lk) and the
     * thread being marked asleep. */
    if (lk != &proc_lock) {
        acquire(&proc_lock);
        release(lk);
    }
    t->chan = chan;
    t->state = THREAD_SLEEPING;
    sched();
    t->chan = NULL;
    if (lk != &proc_lock) {
        release(&proc_lock);
        acquire(lk);
    }
}

/* Whether `chan` is one of poll table `pt`'s channels. */
static bool poll_has(const struct poll_table *pt, const void *chan) {
    for (u32 i = 0; i < pt->n; i++) {
        if (pt->chans[i] == chan) {
            return true;
        }
    }
    return false;
}

/* wakeup(), with proc_lock held. A thread in poll() is woken by any of its
 * table's channels: asleep, it runs again (it sleeps on the table itself,
 * which no one else wakes); still scanning its files, it is told to scan
 * them again (poll_sleep). */
static void wakeup_locked(const void *chan) {
    u32 woken = 0;
    struct thread *end = threads_top;
    for (struct thread *t = threads; t != end; t++) {
        if (t->state == THREAD_SLEEPING && t->chan == chan) {
            t->state = THREAD_RUNNABLE;
            woken++;
        } else if (t->poll != NULL && poll_has(t->poll, chan)) {
            /* Asleep in poll_sleep(), or busy: a hook that sleeps for
             * something else (a lock) is not woken for this. */
            if (t->state == THREAD_SLEEPING && t->chan == t->poll) {
                t->state = THREAD_RUNNABLE;
                woken++;
            } else {
                t->poll->woken = true;
            }
        }
    }
    if (woken != 0) {
        kick_idle(woken);
    }
}

void wakeup(const void *chan) {
    acquire(&proc_lock);
    wakeup_locked(chan);
    release(&proc_lock);
}

volatile u32 polling_threads;

void poll_begin(struct poll_table *pt) {
    acquire(&proc_lock);
    pt->n = 0;
    pt->woken = false;
    mythread()->poll = pt;
    polling_threads++;
    release(&proc_lock);
}

void poll_wait(struct poll_table *pt, const void *chan) {
    acquire(&proc_lock);
    if (!poll_has(pt, chan)) {
        if (pt->n == POLL_CHANS) {
            panic("poll_wait: too many channels");
        }
        pt->chans[pt->n] = chan;
        pt->n++;
    }
    release(&proc_lock);
}

void poll_sleep(struct poll_table *pt) {
    struct thread *t = mythread();
    acquire(&proc_lock);
    /* A wakeup since the scan began, or a kill, and the scan is done
     * again at once: the file it was for may be ready. */
    if (!pt->woken && !t->killed && !t->proc->killed) {
        t->chan = pt;
        t->state = THREAD_SLEEPING;
        sched();
        t->chan = NULL;
    }
    pt->n = 0;
    pt->woken = false;
    release(&proc_lock);
}

void poll_end(void) {
    acquire(&proc_lock);
    mythread()->poll = NULL;
    polling_threads--;
    release(&proc_lock);
}

/* Ends the current thread, which is not its process's last (or whose
 * process is being ended by another thread), with `status`: it becomes a
 * zombie for thread_join(), wakes whoever joins it and whoever waits for
 * the process's threads to end, and switches away for good. The scheduler
 * frees its stack (reap_thread). Called with proc_lock held. */
static void thread_end(struct thread *t, int status) {
    struct proc *p = t->proc;
    t->xstatus = status;
    p->nthreads--;
    wakeup_locked(t);
    wakeup_locked(&p->nthreads);
    t->state = THREAD_ZOMBIE;
    sched();
    panic("zombie ran again");
}

void proc_exit(int status) {
    struct thread *t = mythread();
    struct proc *p = t->proc;
    acquire(&proc_lock);
    if (!end_other_threads()) {
        /* Another thread is ending the process, or replacing its program,
         * and waits for this one. */
        thread_end(t, status);
    }
    release(&proc_lock);
    /* The process's files and working directory, while it can still sleep:
     * closing a file or letting go of a directory may free an unlinked
     * inode, a transaction. No other thread is left to use them. */
    for (u32 fd = 0; fd < NOFILE; fd++) {
        struct file *f = fd_take(fd);
        if (f != NULL) {
            fileclose(f);
        }
    }
    struct inode *cwd = proc_set_cwd(NULL);
    if (cwd != NULL) {
        begin_op();
        iput(cwd);
        end_op();
    }
    acquire(&proc_lock);
    for (u32 i = 0; i < NPROC; i++) {
        struct proc *child = &procs[i];
        if (child->state == PROC_UNUSED || child->parent != p) {
            continue;
        }
        /* Init adopts it, as on Linux, and collects it with wait(); a
         * zombie wakes it. */
        if (init_reaps && init_proc != NULL && p != init_proc) {
            child->parent = init_proc;
            if (child->state == PROC_ZOMBIE) {
                wakeup_locked(init_proc);
            }
            continue;
        }
        /* No one will wait for it now. A zombie can go at once (none of its
         * threads runs, and its page table is in no hart's satp); the
         * scheduler frees the others when they end. */
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
    /* The scheduler sees the process a zombie when this thread switches
     * away, and frees the thread's slot (and the process, if it is an
     * orphan). */
    p->nthreads = 0;
    p->state = PROC_ZOMBIE;
    t->xstatus = status;
    t->state = THREAD_ZOMBIE;
    sched();
    panic("zombie ran again");
}

void proc_kill(struct trap_frame *f) {
    struct proc *p = myproc();
    kprintf("pid %u (%s) killed: %s\n", p->pid, p->name, trap_cause_name(f->scause));
    kprintf("  scause " REG_FMT "  sepc " REG_FMT "  stval " REG_FMT "\n", f->scause, f->sepc, f->stval);
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
        /* Neither the kernel's own processes nor the first one, init (or
         * the shell), whose end is Camelot's. */
        if (p->kfn != NULL || p == init_proc) {
            r = -EPERM;
            break;
        }
        if (p->state != PROC_ZOMBIE) {
            p->killed = true;
            /* Its sleeping threads wake to see it; the first one back on
             * its way to U-mode ends the process (proc_exit). */
            kill_threads(p, NULL);
        }
        r = 0;
        break;
    }
    release(&proc_lock);
    return r;
}

bool proc_exists(int pid) {
    bool found = false;
    acquire(&proc_lock);
    for (u32 i = 0; i < NPROC && !found; i++) {
        const struct proc *p = &procs[i];
        found = p->state != PROC_UNUSED && p->state != PROC_NEW && p->state != PROC_ZOMBIE &&
                p->pid == (u32)pid;
    }
    release(&proc_lock);
    return found;
}

bool proc_killed(void) {
    struct thread *t = mythread();
    if (t == NULL) {
        return false;
    }
    acquire(&proc_lock);
    bool killed = t->killed || t->proc->killed;
    release(&proc_lock);
    return killed;
}

/* The state procs reports for live process `p`: its threads' "busiest"
 * state. Called with proc_lock held. */
static u32 reported_state(struct proc *p) {
    u32 state = PROC_SLEEPING;
    for (u32 i = 0; i < NTHREAD; i++) {
        struct thread *t = &threads[i];
        if (t->proc != p) {
            continue;
        }
        if (t->state == THREAD_RUNNING) {
            return PROC_RUNNING;
        }
        if (t->state == THREAD_RUNNABLE) {
            state = PROC_RUNNABLE;
        }
    }
    return state;
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
            info->state = next->state == PROC_ZOMBIE ? PROC_ZOMBIE : reported_state(next);
            info->size = next->pagetable != NULL ? next->brk : 0;
            memcpy(info->name, next->name, sizeof info->name);
        }
        n++;
        last = next->pid;
    }
}

int proc_harts(struct hartinfo *out, int max) {
    u32 up = harts_running();
    int n = 0;
    acquire(&proc_lock);
    for (u32 i = 0; i < NCPU; i++) {
        if ((up & (1u << i)) == 0) {
            continue;
        }
        if (n < max) {
            struct cpu *c = &cpus[i];
            struct hartinfo *info = &out[n];
            info->hart = i;
            info->pid = c->proc != NULL ? c->proc->pid : 0;
            info->ticks = c->ticks;
            info->switches = c->switches;
            info->idle = c->idle_ticks;
        }
        n++;
    }
    release(&proc_lock);
    return n;
}

int proc_wait(int pid, uintptr status) {
    struct proc *p = myproc();
    struct thread *t = mythread();
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
                /* copyout() walks the page table and never sleeps, so it
                 * may run under the lock. It checks again: another thread
                 * may have shrunk the heap since the check above. */
                if (status != 0 && copyout(p->pagetable, status, &xstatus, sizeof xstatus) != 0) {
                    release(&proc_lock);
                    return -EFAULT;
                }
                proc_free(child);
                release(&proc_lock);
                /* A child that still mapped /dev/fb0 may have left the
                 * display's buffer to free: free it before the caller goes
                 * on to make another process. (proc_exit's reaps of its
                 * zombies are freed by the scheduler it switches to.) */
                fb_reclaim();
                return child_pid;
            }
        }
        if (!found || p->killed || t->killed) {
            /* Killed: never seen, the caller ends on the way out. */
            release(&proc_lock);
            return -ECHILD;
        }
        /* proc_exit() wakes the parent. */
        sleep(p, &proc_lock);
    }
}

/* Maps a new zeroed page at user address `a` of `p`, readable and
 * writable, for the stack or the heap: 0, or -ENOMEM (nothing mapped).
 * Under vm_lock, as a page fault in a mapping maps its pages: the two may
 * need the same new page table (mmap.h, "Locking"). Called holding
 * p->heap_lock. */
static int map_user_page(struct proc *p, uintptr a) {
    void *mem = kzalloc();
    if (mem == NULL) {
        return -ENOMEM;
    }
    acquire(&p->vm_lock);
    int r = map_pages(p->pagetable, a, pa(mem), PGSIZE, PTE_U | PTE_R | PTE_W);
    release(&p->vm_lock);
    if (r != 0) {
        kfree(mem);
        return -ENOMEM;
    }
    return 0;
}

int proc_grow_stack(uintptr va) {
    struct proc *p = myproc();
    if (va < USER_STACK_LIMIT || va >= USER_TOP) {
        return -EFAULT;
    }
    uintptr low = PGROUNDDOWN(va);
    /* stack_low only moves down, so a stale look can only send us to the
     * lock for nothing. */
    if (low >= p->stack_low) {
        return 0;
    }
    acquiresleep(&p->heap_lock);
    int r = 0;
    while (p->stack_low > low) {
        uintptr a = p->stack_low - PGSIZE;
        if (map_user_page(p, a) != 0) {
            /* What was mapped stays: the stack is just shorter. */
            r = -ENOMEM;
            break;
        }
        p->stack_low = a;
    }
    releasesleep(&p->heap_lock);
    /* New mappings only, so no other hart has a stale one; this hart may have
     * cached the fault's missing translation. */
    sfence_vma();
    return r;
}

long proc_sbrk(long increment) {
    struct proc *p = myproc();
    /* One sbrk() at a time: two threads mapping the same new page, or one
     * freeing what the other maps, would break the page table. */
    acquiresleep(&p->heap_lock);
    uintptr old = p->brk;
    if (increment > 0) {
        uintptr grow = (uintptr)increment;
        if (grow > USER_LOAD_TOP - old) {
            releasesleep(&p->heap_lock);
            return -ENOMEM;
        }
        /* More than the whole of RAM can never be had: that fails at once,
         * as Linux's default overcommit check fails it (mm/util.c,
         * __vm_enough_memory with OVERCOMMIT_GUESS: more pages than RAM and
         * swap hold), rather than after taking and zeroing every free
         * page. The pages counted are those the growth maps, from the old
         * end's page boundary to the new one's, as Linux's brk counts
         * PAGE_ALIGN(newbrk) - PAGE_ALIGN(oldbrk) (mm/mmap.c). */
        if ((PGROUNDUP(old + grow) - PGROUNDUP(old)) / PGSIZE > ram_size / PGSIZE) {
            releasesleep(&p->heap_lock);
            return -ENOMEM;
        }
        uintptr end = old + grow;
        uintptr first = PGROUNDUP(old);
        /* The heap can't grow into a memory mapping (Linux's brk checks
         * the same): mmap() puts them above it, from the top down. */
        if (mm_overlaps(p, first, PGROUNDUP(end))) {
            releasesleep(&p->heap_lock);
            return -ENOMEM;
        }
        /* Map the pages from the first one not yet mapped up to the new
         * end; on failure, unmap them again. */
        for (uintptr a = first; a < end; a += PGSIZE) {
            if (map_user_page(p, a) != 0) {
                if (a > first) {
                    unmap_pages(p->pagetable, first, a - first, true);
                }
                releasesleep(&p->heap_lock);
                return -ENOMEM;
            }
        }
        p->brk = end;
    } else if (increment < 0) {
        /* -increment as an unsigned number: right even for LONG_MIN. */
        uintptr shrink = 0 - (uintptr)increment;
        if (shrink > old - p->heap_base) {
            releasesleep(&p->heap_lock);
            return -EINVAL;
        }
        uintptr end = old - shrink;
        uintptr keep = PGROUNDUP(end);
        /* The end moves first, so that nothing takes the pages for heap
         * any more; then they go, shot down on every hart that runs one of
         * the process's threads (unmap_pages). */
        p->brk = end;
        if (keep < PGROUNDUP(old)) {
            unmap_pages(p->pagetable, keep, PGROUNDUP(old) - keep, true);
        }
        /* The rest of the last page kept reads zero again, so that growing
         * later hands out zeroed memory, as for a new page. */
        if (end < keep) {
            pte_t *pte = walk(p->pagetable, end, false);
            u8 *page = va(PTE2PA(*pte));
            memset(page + (end % PGSIZE), 0, keep - end);
        }
    }
    releasesleep(&p->heap_lock);
    /* The old break, a user address: below USER_TOP, so never negative. */
    return (long)old;
}

int thread_create(uintptr entry, uintptr arg, uintptr stack) {
    struct proc *p = myproc();
    /* The pointers first: the code must be there to run, and the new
     * thread's first stack frame must have room below `stack`. */
    if (!uvm_range_ok(p->pagetable, entry, 4, PTE_X) || stack < 16 ||
        !uvm_range_ok(p->pagetable, stack - 16, 16, PTE_W)) {
        return -EFAULT;
    }
    if (stack % 16 != 0 || entry % 4 != 0) {
        return -EINVAL;
    }
    int err;
    struct thread *t = thread_alloc(p, &err);
    if (t == NULL) {
        return err;
    }
    /* The registers: all zero but pc, sp and a0. The thread's kernel side
     * starts in thread_start, as a new process's does. */
    t->frame = frame_of(t);
    memset(t->frame, 0, sizeof *t->frame);
    t->frame->sepc = entry;
    t->frame->regs[REG_SP] = stack;
    t->frame->regs[REG_A0] = arg;
    t->context.ra = (uintptr)thread_start;
    t->context.sp = (uintptr)t->frame;
    acquire(&proc_lock);
    /* A process being ended, or replaced by exec, takes no new thread: the
     * thread ending it waits for the ones it knows of. */
    if (p->exiting || p->killed || mythread()->killed) {
        thread_free(t);
        release(&proc_lock);
        return -EAGAIN;
    }
    int tid = (int)next_pid;
    t->tid = next_pid;
    next_pid++;
    p->nthreads++;
    t->state = THREAD_RUNNABLE;
    kick_idle(1);
    release(&proc_lock);
    return tid;
}

void thread_exit(int status) {
    struct thread *t = mythread();
    struct proc *p = t->proc;
    acquire(&proc_lock);
    if (p->nthreads == 1 && !p->exiting) {
        /* The last thread: the process ends with it. Only this thread
         * could start another, so it stays the last until proc_exit()
         * takes the lock again. */
        release(&proc_lock);
        proc_exit(status);
    }
    thread_end(t, status);
}

int thread_join(int tid, uintptr status) {
    struct proc *p = myproc();
    struct thread *me = mythread();
    if (status != 0 && !uvm_range_ok(p->pagetable, status, sizeof(int), PTE_W)) {
        return -EFAULT;
    }
    if (tid == (int)me->tid) {
        return -EINVAL;
    }
    acquire(&proc_lock);
    for (;;) {
        struct thread *t = NULL;
        for (u32 i = 0; i < NTHREAD; i++) {
            struct thread *x = &threads[i];
            /* A new thread has no tid yet (0, which no thread has). */
            if (x->proc == p && x->state != THREAD_UNUSED && x->state != THREAD_NEW && x->tid == (u32)tid) {
                t = x;
                break;
            }
        }
        if (t == NULL) {
            release(&proc_lock);
            return -ESRCH;
        }
        if (t->state == THREAD_ZOMBIE) {
            int xstatus = t->xstatus;
            /* As in proc_wait(). */
            if (status != 0 && copyout(p->pagetable, status, &xstatus, sizeof xstatus) != 0) {
                release(&proc_lock);
                return -EFAULT;
            }
            thread_free(t);
            release(&proc_lock);
            return 0;
        }
        if (p->killed || me->killed) {
            release(&proc_lock);
            return -EINTR;
        }
        /* thread_end() wakes the thread's joiners. */
        sleep(t, &proc_lock);
    }
}

int futex_wait(uintptr addr, u32 val) {
    struct proc *p = myproc();
    struct thread *t = mythread();
    if (addr % 4 != 0) {
        return -EINVAL;
    }
    /* Under proc_lock, which futex_wake() takes too: from the read below
     * to the sleep, no wake can come in between. A thread that changes the
     * word and then calls futex_wake() either changed it before the read
     * (and this returns -EAGAIN) or finds this thread asleep. copyin()
     * never sleeps, so it may run under the lock. */
    acquire(&proc_lock);
    u32 now;
    if (copyin(p->pagetable, &now, addr, sizeof now) != 0) {
        release(&proc_lock);
        return -EFAULT;
    }
    if (now != val) {
        release(&proc_lock);
        return -EAGAIN;
    }
    if (p->killed || t->killed) {
        release(&proc_lock);
        return -EINTR;
    }
    /* The channel is the thread's own `futex` field, which nothing else
     * sleeps on; futex_wake() looks for it and the address. */
    t->futex = addr;
    sleep(&t->futex, &proc_lock);
    t->futex = 0;
    /* Woken by futex_wake(), or by kill. */
    int r = p->killed || t->killed ? -EINTR : 0;
    release(&proc_lock);
    return r;
}

int futex_wake(uintptr addr, i32 n) {
    struct proc *p = myproc();
    if (addr % 4 != 0) {
        return -EINVAL;
    }
    if (!uvm_range_ok(p->pagetable, addr, 4, PTE_R)) {
        return -EFAULT;
    }
    u32 woken = 0;
    acquire(&proc_lock);
    for (u32 i = 0; i < NTHREAD && (i32)woken < n; i++) {
        struct thread *t = &threads[i];
        if (t->proc == p && t->state == THREAD_SLEEPING && t->chan == &t->futex && t->futex == addr) {
            t->state = THREAD_RUNNABLE;
            woken++;
        }
    }
    if (woken != 0) {
        kick_idle(woken);
    }
    release(&proc_lock);
    return (int)woken;
}

struct file *fd_get(u32 fd) {
    struct proc *p = myproc();
    if (fd >= NOFILE) {
        return NULL;
    }
    acquire(&p->files_lock);
    struct file *f = p->ofile[fd];
    if (f != NULL) {
        filedup(f);
    }
    release(&p->files_lock);
    return f;
}

int fd_install(struct file *f) {
    return fd_install_from(f, 0, false);
}

int fd_install_from(struct file *f, u32 lowest, bool cloexec) {
    struct proc *p = myproc();
    int r = -EMFILE;
    acquire(&p->files_lock);
    for (u32 fd = lowest; fd < NOFILE; fd++) {
        if (p->ofile[fd] == NULL) {
            p->ofile[fd] = f;
            /* The bit of a free descriptor is clear (fd_take()). */
            if (cloexec) {
                p->cloexec |= 1ul << fd;
            }
            r = (int)fd;
            break;
        }
    }
    release(&p->files_lock);
    return r;
}

int fd_flags(u32 fd) {
    struct proc *p = myproc();
    if (fd >= NOFILE) {
        return -EBADF;
    }
    acquire(&p->files_lock);
    int r = -EBADF;
    if (p->ofile[fd] != NULL) {
        r = (p->cloexec & (1ul << fd)) != 0 ? FD_CLOEXEC : 0;
    }
    release(&p->files_lock);
    return r;
}

int fd_set_flags(u32 fd, u32 flags) {
    struct proc *p = myproc();
    if (fd >= NOFILE) {
        return -EBADF;
    }
    acquire(&p->files_lock);
    int r = -EBADF;
    if (p->ofile[fd] != NULL) {
        if ((flags & FD_CLOEXEC) != 0) {
            p->cloexec |= 1ul << fd;
        } else {
            p->cloexec &= ~(1ul << fd);
        }
        r = 0;
    }
    release(&p->files_lock);
    return r;
}

void fd_close_on_exec(void) {
    struct proc *p = myproc();
    for (u32 fd = 0; fd < NOFILE; fd++) {
        struct file *f = NULL;
        acquire(&p->files_lock);
        if ((p->cloexec & (1ul << fd)) != 0) {
            f = p->ofile[fd];
            p->ofile[fd] = NULL;
            p->cloexec &= ~(1ul << fd);
        }
        release(&p->files_lock);
        /* Not under the spinlock: closing may free an inode, which sleeps. */
        if (f != NULL) {
            fileclose(f);
        }
    }
}

int fd_install2(struct file *a, struct file *b, int *fds) {
    struct proc *p = myproc();
    int n = 0;
    acquire(&p->files_lock);
    for (u32 fd = 0; fd < NOFILE && n < 2; fd++) {
        if (p->ofile[fd] == NULL) {
            fds[n] = (int)fd;
            n++;
        }
    }
    if (n == 2) {
        p->ofile[fds[0]] = a;
        p->ofile[fds[1]] = b;
    }
    release(&p->files_lock);
    return n == 2 ? 0 : -EMFILE;
}

bool fd_available(void) {
    struct proc *p = myproc();
    bool r = false;
    acquire(&p->files_lock);
    for (u32 fd = 0; fd < NOFILE; fd++) {
        if (p->ofile[fd] == NULL) {
            r = true;
            break;
        }
    }
    release(&p->files_lock);
    return r;
}

struct file *fd_take(u32 fd) {
    struct proc *p = myproc();
    if (fd >= NOFILE) {
        return NULL;
    }
    acquire(&p->files_lock);
    struct file *f = p->ofile[fd];
    p->ofile[fd] = NULL;
    p->cloexec &= ~(1ul << fd);
    release(&p->files_lock);
    return f;
}

bool fd_take_if(u32 fd, struct file *f) {
    struct proc *p = myproc();
    if (fd >= NOFILE) {
        return false;
    }
    acquire(&p->files_lock);
    bool took = p->ofile[fd] == f;
    if (took) {
        p->ofile[fd] = NULL;
        p->cloexec &= ~(1ul << fd);
    }
    release(&p->files_lock);
    return took;
}

struct inode *proc_cwd(void) {
    struct proc *p = myproc();
    if (p == NULL) {
        return NULL;
    }
    acquire(&p->files_lock);
    struct inode *ip = p->cwd != NULL ? idup(p->cwd) : NULL;
    release(&p->files_lock);
    return ip;
}

struct inode *proc_set_cwd(struct inode *ip) {
    struct proc *p = myproc();
    acquire(&p->files_lock);
    struct inode *old = p->cwd;
    p->cwd = ip;
    release(&p->files_lock);
    return old;
}
