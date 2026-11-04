# Spinlocks and nested interrupt disabling (include/spinlock.h; spec section
# 10, "Kernel locking"), after xv6-riscv's spinlock.c. In assembly because
# they need amoswap.w, fence and sstatus, which C reaches only through
# inline assembly (Merlin has none), and because they run on every kalloc,
# kprintf, sleep and wakeup: merlin's C would make each call several times
# longer. The C each function stands for is in its comment.
#
# struct spinlock: locked (a u32) at LOCK_LOCKED, cpu (a pointer) at
# LOCK_CPU (name after it, only read by C). struct cpu: proc at 0, the u32s
# noff and intena at CPU_NOFF and CPU_INTENA, CPU_SIZE bytes per entry of
# cpus[], indexed by tp, the hart id. spinlock.h defines them, and its
# _Static_asserts check them against the structures: on RV64 the pointers
# are 8 bytes, 8-aligned.
#
# The C each function stands for is in a C comment, not a # one: this file
# goes through the C preprocessor, which would take `#  if (...)` for a
# directive.

#include <asm.h>
#include "riscv.h"              /* SSTATUS_SIE */
#include "spinlock.h"           /* LOCK_LOCKED, LOCK_CPU, CPU_NOFF, CPU_INTENA, CPU_SIZE */

    .text

/* u32 r_tp(void)
 *
 * A u32 result is returned sign-extended from bit 31 on RV64, as the ABI
 * keeps every 32-bit value in a register (and the compiler's comparisons
 * rely on it): ADDIW (asm.h) makes one of tp, whatever its upper half holds
 * (illegal_instruction() fills tp with a pattern before it faults). */
    .globl r_tp
r_tp:
    ADDIW a0, tp, 0
    ret

/* struct cpu *mycpu(void) { return &cpus[tp]; }
 *
 * The leaf functions below inline these three instructions, into t1. */
    .globl mycpu
mycpu:
    la    a0, cpus
    li    t0, CPU_SIZE
    mul   t0, tp, t0
    add   a0, a0, t0
    ret

/* void push_off(void)
 * {
 *     u32 old = r_sstatus() & SSTATUS_SIE;
 *     intr_off();
 *     struct cpu *c = mycpu();
 *     if (c->noff == 0)
 *         c->intena = old;
 *     c->noff += 1;
 * }
 *
 * Interrupts go off before the cpu's fields are touched, so an interrupt
 * handler's own push_off() can't land between the read and the write. */
    .globl push_off
push_off:
    csrrci t0, sstatus, SSTATUS_SIE     # t0 = sstatus as it was; SIE now clear
    la    t1, cpus
    li    t2, CPU_SIZE
    mul   t2, tp, t2
    add   t1, t1, t2                    # t1 = mycpu()
    lw    t2, CPU_NOFF(t1)
    bnez  t2, 1f
    andi  t0, t0, SSTATUS_SIE
    sw    t0, CPU_INTENA(t1)            # the outermost push_off remembers SIE
1:  addi  t2, t2, 1
    sw    t2, CPU_NOFF(t1)
    ret

/* void pop_off(void)
 * {
 *     struct cpu *c = mycpu();
 *     if (r_sstatus() & SSTATUS_SIE)
 *         panic("pop_off: interrupts are on");
 *     if (c->noff < 1)
 *         panic("pop_off: no push_off to match");
 *     c->noff -= 1;
 *     if (c->noff == 0 && c->intena)
 *         intr_on();
 * } */
    .globl pop_off
pop_off:
    csrr  t0, sstatus
    andi  t0, t0, SSTATUS_SIE
    bnez  t0, pop_off_interruptible
    la    t1, cpus
    li    t2, CPU_SIZE
    mul   t2, tp, t2
    add   t1, t1, t2                    # t1 = mycpu()
    lw    t2, CPU_NOFF(t1)
    beqz  t2, pop_off_unbalanced
    addi  t2, t2, -1
    sw    t2, CPU_NOFF(t1)
    bnez  t2, 1f
    lw    t0, CPU_INTENA(t1)
    beqz  t0, 1f
    csrsi sstatus, SSTATUS_SIE
1:  ret

pop_off_interruptible:
    la    a0, msg_pop_on
    tail  panic
pop_off_unbalanced:
    la    a0, msg_pop_none
    tail  panic

/* void acquire(struct spinlock *lk)
 * {
 *     push_off();
 *     if (holding(lk))
 *         spin_panic(lk, "acquire");
 *     while (amoswap.w.aq(&lk->locked, 1) != 0)
 *         ;
 *     fence();
 *     lk->cpu = mycpu();
 * }
 *
 * amoswap.w writes 1 and returns what was there in one indivisible step:
 * 0 means the lock was free and is now ours; 1 means another hart has it,
 * so try again. `.aq` (acquire ordering) keeps the critical section's
 * loads and stores from moving before the swap, and the fence says the same
 * for any hart or compiler that ignores the bit, as xv6's
 * __sync_synchronize() does. (Grail runs its harts one instruction at a
 * time, so it needs neither; real hardware does.)
 *
 * push_off() is written out in the first lines rather than called, which
 * saves a stack frame on the hot path. */
    .globl acquire
acquire:
    csrrci t0, sstatus, SSTATUS_SIE     # push_off(), as above
    la    t1, cpus
    li    t2, CPU_SIZE
    mul   t2, tp, t2
    add   t1, t1, t2                    # t1 = mycpu()
    lw    t2, CPU_NOFF(t1)
    bnez  t2, 1f
    andi  t0, t0, SSTATUS_SIE
    sw    t0, CPU_INTENA(t1)
1:  addi  t2, t2, 1
    sw    t2, CPU_NOFF(t1)
    lw    t0, LOCK_LOCKED(a0)           # holding(lk): locked, by this hart?
    beqz  t0, 2f
    REG_L t0, LOCK_CPU(a0)
    beq   t0, t1, acquire_twice
2:  li    t0, 1
3:  amoswap.w.aq t2, t0, (a0)
    bnez  t2, 3b                        # spin while another hart holds it
    fence rw, rw
    REG_S t1, LOCK_CPU(a0)
    ret

acquire_twice:
    la    a1, msg_acquire
    tail  spin_panic                    # spin_panic(lk, "acquire")

/* void release(struct spinlock *lk)
 * {
 *     if (!holding(lk))
 *         spin_panic(lk, "release");
 *     lk->cpu = NULL;
 *     fence();
 *     amoswap.w.rl(&lk->locked, 0);
 *     pop_off();
 * }
 *
 * The fence and `.rl` (release ordering) make every store of the critical
 * section visible before the lock reads free; the swap is a plain store of
 * 0 that the ordering bit can be put on. */
    .globl release
release:
    la    t1, cpus
    li    t2, CPU_SIZE
    mul   t2, tp, t2
    add   t1, t1, t2                    # t1 = mycpu()
    lw    t0, LOCK_LOCKED(a0)
    beqz  t0, release_unheld
    REG_L t0, LOCK_CPU(a0)
    bne   t0, t1, release_unheld
    REG_S zero, LOCK_CPU(a0)
    fence rw, rw
    amoswap.w.rl zero, zero, (a0)
    tail  pop_off

release_unheld:
    la    a1, msg_release
    tail  spin_panic                    # spin_panic(lk, "release")

/* bool holding(struct spinlock *lk) { return lk->locked && lk->cpu == mycpu(); } */
    .globl holding
holding:
    lw    t0, LOCK_LOCKED(a0)
    beqz  t0, 1f
    la    t1, cpus
    li    t2, CPU_SIZE
    mul   t2, tp, t2
    add   t1, t1, t2                    # t1 = mycpu()
    REG_L t0, LOCK_CPU(a0)
    sub   t0, t0, t1
    seqz  a0, t0
    ret
1:  li    a0, 0
    ret

    .section .rodata
msg_acquire:
    .asciz "acquire"
msg_release:
    .asciz "release"
msg_pop_on:
    .asciz "pop_off: interrupts are on"
msg_pop_none:
    .asciz "pop_off: no push_off to match"
