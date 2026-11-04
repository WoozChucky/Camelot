# CSR accessors for C (include/riscv.h). The kernel keeps CSR access in
# assembly instead of inline asm, which Merlin won't support (#15). Each is a
# leaf function: argument in a0, result in a0, a whole register (XLEN bits)
# on either width, so nothing here depends on it but r_time.

    .text

    .globl r_mstatus
r_mstatus:
    csrr  a0, mstatus
    ret

    .globl w_mstatus
w_mstatus:
    csrw  mstatus, a0
    ret

    .globl w_mepc
w_mepc:
    csrw  mepc, a0
    ret

    .globl w_mtvec
w_mtvec:
    csrw  mtvec, a0
    ret

    .globl w_mscratch
w_mscratch:
    csrw  mscratch, a0
    ret

    .globl r_medeleg
r_medeleg:
    csrr  a0, medeleg
    ret

    .globl w_medeleg
w_medeleg:
    csrw  medeleg, a0
    ret

    .globl r_mideleg
r_mideleg:
    csrr  a0, mideleg
    ret

    .globl w_mideleg
w_mideleg:
    csrw  mideleg, a0
    ret

    .globl w_mie
w_mie:
    csrw  mie, a0
    ret

    .globl w_mcounteren
w_mcounteren:
    csrw  mcounteren, a0
    ret

# u64 r_time(void). On RV64 one read. On RV32 the 64-bit counter is two
# CSRs, time and timeh, which can't be read at once: read the high half, the
# low half, then the high half again, and start over if it changed (the low
# half wrapped in between). The result is in a1:a0, as a u64 is returned on
# RV32.
    .globl r_time
r_time:
#if __riscv_xlen == 64
    csrr  a0, time
#else
1:  csrr  a1, timeh
    csrr  a0, time
    csrr  t0, timeh
    bne   a1, t0, 1b
#endif
    ret

    .globl r_sstatus
r_sstatus:
    csrr  a0, sstatus
    ret

    .globl w_sstatus
w_sstatus:
    csrw  sstatus, a0
    ret

    .globl r_sie
r_sie:
    csrr  a0, sie
    ret

    .globl w_sie
w_sie:
    csrw  sie, a0
    ret

    .globl r_sip
r_sip:
    csrr  a0, sip
    ret

    .globl w_sip
w_sip:
    csrw  sip, a0
    ret

    .globl r_stvec
r_stvec:
    csrr  a0, stvec
    ret

    .globl w_stvec
w_stvec:
    csrw  stvec, a0
    ret

    .globl r_satp
r_satp:
    csrr  a0, satp
    ret

    .globl w_satp
w_satp:
    csrw  satp, a0
    ret

    .globl sfence_vma
sfence_vma:
    sfence.vma zero, zero
    ret

    .globl wfi
wfi:
    wfi
    ret

# void intr_off(void), void intr_on(void): clear / set sstatus.SIE.
    .globl intr_off
intr_off:
    csrci sstatus, 2
    ret

    .globl intr_on
intr_on:
    csrsi sstatus, 2
    ret

# Atomic read-modify-write of one word, for the counters and flags harts
# share outside any lock (smp.c, trap.c). Each is a single AMO, so nothing,
# another hart or a trap on this one, can come between the read and the
# write; .aqrl orders it against the loads and stores around it, as a
# fence on each side would. Each returns the word as it was.

# u32 atomic_swap(volatile u32 *p, u32 v): *p = v
    .globl atomic_swap
atomic_swap:
    amoswap.w.aqrl a0, a1, (a0)
    ret

# u32 atomic_add(volatile u32 *p, u32 v): *p += v
    .globl atomic_add
atomic_add:
    amoadd.w.aqrl a0, a1, (a0)
    ret

# u32 atomic_or(volatile u32 *p, u32 v): *p |= v
    .globl atomic_or
atomic_or:
    amoor.w.aqrl a0, a1, (a0)
    ret

# void fence_rw(void): orders every load and store before it before every
# load and store after it, as other harts see them.
    .globl fence_rw
fence_rw:
    fence rw, rw
    ret
