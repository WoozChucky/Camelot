/* Spinlocks, interrupt nesting and per-CPU state (spec section 10, "Kernel
 * locking"), after xv6-riscv's spinlock.c and the cpu half of its proc.c.
 *
 *   acquire(&lk);      interrupts off on this hart, then spin until lk is ours
 *   ... the data lk protects ...
 *   release(&lk);      lk free, interrupts back as they were
 *
 * A spinlock is held for a short stretch of code that never sleeps. Holding
 * one keeps interrupts off on the hart (push_off), so an interrupt handler
 * that wants the same lock can't arrive in the middle and spin forever
 * against the code it interrupted. Code that has to wait for something
 * while holding a lock sleeps with sleep(chan, lk) (proc.h), which lets go
 * of the lock while it sleeps; data held across a sleep is protected by a
 * sleep lock instead (sleeplock.h).
 *
 * The locks are taken in one global order, so no two harts can wait for
 * each other in a cycle; spec section 10, "Kernel locking", lists it.
 *
 * acquire, release, push_off and pop_off are in spinlock.S: they need
 * amoswap.w, fence and the CSRs, which C can't reach without inline
 * assembly, and they run often enough (every kalloc, every kprintf) that
 * the few instructions of hand-written assembly matter. */
#ifndef CAMELOT_SPINLOCK_H
#define CAMELOT_SPINLOCK_H

#include "context.h"
#include "types.h"
#include "vm.h"

/* The most harts the kernel can run on: Grail's limit (spec section 7,
 * "Harts"). cpus[] has one entry per hart, indexed by mhartid. */
#define NCPU 8

struct proc;
struct thread;

/* What the kernel keeps per hart. `tp` holds the hart's id (its mhartid,
 * set by _start in M-mode and never changed by the kernel; user_vec puts it
 * back after a trap from U-mode, where programs may use tp as they like), so
 * mycpu() is &cpus[tp]. spinlock.S reaches the fields at fixed offsets
 * (CPU_NOFF, CPU_INTENA) and the entries at CPU_SIZE apart, which it defines
 * for each width as below: keep them in sync (the _Static_asserts check
 * this side). */
struct cpu {
    /* The process of the thread running on this hart (thread->proc), or
     * NULL while the hart runs the scheduler loop or kmain. */
    struct proc *proc;
    /* How deep push_off() calls nest: the number of spinlocks held, plus
     * any push_off() without a lock. */
    u32 noff;
    /* Whether interrupts were on (sstatus.SIE) before the outermost
     * push_off(): the last pop_off() turns them back on only then. */
    u32 intena;
    /* The hart's id, = tp, = the entry's index. */
    u32 hartid;
    /* The scheduler loop's registers while a process runs on this hart:
     * sched() swtch()es here. */
    struct context context;
    /* The page table in satp: the running process's, or the kernel's. The
     * scheduler and exec set it, under proc_lock or for their own hart;
     * tlb_shootdown() reads it (without a lock: smp.c says why that is
     * enough) to find the harts that may hold stale translations. */
    pagetable_t pagetable;
    /* Timer ticks this hart has taken, and processes it has switched to,
     * for the harts system call. Written only by the hart itself. */
    u32 ticks;
    u32 switches;
    /* Set by the scheduler, under proc_lock, when it found nothing to run
     * and is about to wait in wfi; cleared when it looks again, or by the
     * hart that wakes it with an IPI because a process became runnable
     * (proc.c). */
    bool idle;
    /* The thread running on this hart, or NULL with `proc`. */
    struct thread *thread;
};

/* The layout spinlock.S and the kernel tests (crates/xtask/tests/common)
 * use. RV64's pointers and saved registers are 8 bytes, and the pointers
 * after a u32 start on the next 8-byte boundary. */
#if __riscv_xlen == 64
#define CPU_NOFF 8
#define CPU_INTENA 12
#define CPU_CONTEXT 24
#define CPU_PAGETABLE 136
#define CPU_THREAD 160
#define CPU_SIZE 168
#else
#define CPU_NOFF 4
#define CPU_INTENA 8
#define CPU_CONTEXT 16
#define CPU_PAGETABLE 72
#define CPU_THREAD 88
#define CPU_SIZE 92
#endif
_Static_assert(sizeof(struct cpu) == CPU_SIZE, "spinlock.S indexes cpus[] by CPU_SIZE");
_Static_assert(offsetof(struct cpu, noff) == CPU_NOFF, "spinlock.S reads noff at CPU_NOFF");
_Static_assert(offsetof(struct cpu, intena) == CPU_INTENA, "spinlock.S reads intena at CPU_INTENA");
_Static_assert(offsetof(struct cpu, context) == CPU_CONTEXT, "the tests read the context there");
_Static_assert(offsetof(struct cpu, pagetable) == CPU_PAGETABLE, "the tests read pagetable there");
_Static_assert(offsetof(struct cpu, thread) == CPU_THREAD, "the tests read thread there");

extern struct cpu cpus[NCPU];

/* This hart's entry of cpus[]. Only meaningful while the code can't move
 * to another hart, which in this kernel is always: kernel threads are never
 * preempted, and a thread changes harts only through the scheduler. */
struct cpu *mycpu(void);

/* This hart's id (tp). */
u32 cpuid(void);

/* Sets up this hart's entry of cpus[]: no process, no push_off(). kmain
 * calls it first thing, before anything takes a lock. */
void cpu_init(void);

/* The fields are at fixed offsets (locked at 0, cpu at LOCK_CPU: 4 on RV32,
 * 8 on RV64): spinlock.S uses them. */
struct spinlock {
    /* 0 when free, 1 when held: acquire() swaps a 1 in with amoswap.w. */
    u32 locked;
    /* The hart holding it, for holding() and the double-acquire check. */
    struct cpu *cpu;
    /* For the panic messages. */
    const char *name;
};

#if __riscv_xlen == 64
#define LOCK_CPU 8
#else
#define LOCK_CPU 4
#endif
_Static_assert(offsetof(struct spinlock, cpu) == LOCK_CPU, "spinlock.S reads cpu at LOCK_CPU");

/* Sets up `lk`, unlocked. A lock in .bss that is all zero works unlocked
 * too, but without a name in its panic messages. */
void initlock(struct spinlock *lk, const char *name);

/* Turns interrupts off on this hart and spins until `lk` is free, then
 * takes it. Panics if this hart already holds it: a spinlock can't be
 * taken twice, and waiting for ourselves would hang the hart. */
void acquire(struct spinlock *lk);

/* Lets go of `lk`, which this hart must hold (else it panics), and turns
 * interrupts back on if this was the last lock and they were on before the
 * first. */
void release(struct spinlock *lk);

/* Whether this hart holds `lk`. Must be called with interrupts off (with a
 * lock held, say), or the answer may be stale by the time it is used. */
bool holding(struct spinlock *lk);

/* Interrupts off, nested: push_off() turns them off and counts; the
 * matching pop_off() turns them back on when the count drops to zero, if
 * they were on before the first push_off(). acquire() and release() use
 * them, so code holding a lock keeps interrupts off. pop_off() panics if
 * interrupts are on (someone turned them on inside) or if there was no
 * push_off(). */
void push_off(void);
void pop_off(void);

/* spinlock.S's error path: panics with a message that names the lock, e.g.
 * `acquire: lock "kmem" already held by hart 0`. */
void spin_panic(const struct spinlock *lk, const char *what);

/* Reads tp (spinlock.S): the hart's id. */
u32 r_tp(void);

#endif
