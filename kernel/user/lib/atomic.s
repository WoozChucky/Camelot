# Atomics and spinlocks for threads (lib/README.md, "Threads"). merlin has
# no inline assembly, and C has no way to ask for an AMO or lr/sc, so they
# are here, in assembly, called like any C function: the call itself also
# keeps the compiler from moving loads and stores of shared data across
# them.
#
# Each read-modify-write is .aqrl: no load or store before it may be seen
# after it, nor any after it before it, as a fence on each side would
# order them. Each returns the word as it was before.

    .text

# int atomic_swap(int *p, int v): *p = v
    .globl atomic_swap
atomic_swap:
    amoswap.w.aqrl a0, a1, (a0)
    ret

# int atomic_add(int *p, int v): *p += v
    .globl atomic_add
atomic_add:
    amoadd.w.aqrl a0, a1, (a0)
    ret

# int atomic_cas(int *p, int expect, int desired): *p = desired if *p is
# expect. The word as it was, so expect if it was stored. A load-reserved
# and a store-conditional: the store fails (and the loop tries again) if
# anything wrote the word in between.
    .globl atomic_cas
atomic_cas:
1:  lr.w.aqrl t0, (a0)
    bne   t0, a1, 2f
    sc.w.aqrl t1, a2, (a0)
    bnez  t1, 1b
2:  mv    a0, t0
    ret

# int atomic_load(int *p): *p, ordered after every load and store before
# it and before every one after it.
    .globl atomic_load
atomic_load:
    fence rw, rw
    lw    a0, 0(a0)
    fence rw, rw
    ret

# void atomic_store(int *p, int v): *p = v, ordered as atomic_load.
    .globl atomic_store
atomic_store:
    amoswap.w.aqrl zero, a1, (a0)
    ret

# void spin_lock(struct spinlock *l): the kernel's acquire() without the
# interrupts: swap a 1 in until the swap gives back 0, the lock free. While
# it is held, wait with plain loads, which only read the cache line, and
# try the swap again once it looks free. .aq: nothing the lock protects is
# read or written before the lock is ours.
    .globl spin_lock
spin_lock:
    li    t0, 1
1:  amoswap.w.aq t1, t0, (a0)
    beqz  t1, 3f
2:  lw    t1, 0(a0)
    bnez  t1, 2b
    j     1b
3:  ret

# void spin_unlock(struct spinlock *l): .rl, so every store made under the
# lock is seen before the lock is free.
    .globl spin_unlock
spin_unlock:
    amoswap.w.rl zero, zero, (a0)
    ret
