# CSR accessors for C (include/riscv.h). The kernel keeps CSR access in
# assembly instead of inline asm, which Merlin won't support (#15). Each is a
# leaf function: argument in a0, result in a0.

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
