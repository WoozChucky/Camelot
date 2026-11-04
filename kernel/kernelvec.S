# The S-mode trap vector. entry.S points stvec here. Saves x1-x31 and the
# trap CSRs on the stack as a struct trap_frame (include/trap.h), calls
# kernel_trap(frame), then restores everything and returns with sret. sepc
# and sstatus are restored from the frame, so kernel_trap may change them.
#
# stvec points here only while the hart is in S-mode, so the interrupted sp
# is a kernel stack and the frame goes just below it. Traps from U-mode go to
# user_vec (uservec.S) instead, which finds the process's kernel stack
# through sscratch.
#
# Every slot is a register wide (SZREG, asm.h): regs[i] at i*SZREG, then the
# CSRs. The same source builds for RV32 and RV64 (#193).
#
# On RV64 the kernel stacks are 16 KiB each with an unmapped guard page
# below (vm.h, "Kernel stacks"), and a stack that overflows faults into its
# guard page: a trap whose frame would have to go there is caught before
# anything is stored, as Linux's handle_exception does under CONFIG_VMAP_STACK
# (arch/riscv/kernel/entry.S). Each stack is the lower half of a slot of
# twice KSTACK_SIZE, aligned to that, so bit KSTACK_SHIFT of an address is 0
# on the stack and 1 just below it: one bit of sp - FRAME_SIZE says whether
# the frame still fits. Testing it needs a free register, and the only one
# is sscratch: in S-mode it holds nothing (user_return sets it again before
# every return to U-mode), so t0 goes there for the test. A frame that does
# not fit goes on this hart's overflow stack instead, and kstack_overflow()
# reports the overflow: storing it below sp would fault again, in the guard
# page, and again, until sp left the guard page for whatever lies below.

#include <asm.h>

#if __riscv_xlen == 64
    .equ FRAME_SIZE, 304        # TRAP_FRAME_SIZE, a multiple of 16
    .equ KSTACK_SHIFT, 14       # log2 of KSTACK_SIZE (16 KiB, vm.h)
    .equ OVERFLOW_STACK_SHIFT, 12 # log2 of OVERFLOW_STACK_SIZE (4 KiB, Linux's)
#else
    .equ FRAME_SIZE, 160
#endif
    .equ FRAME_SEPC, 32*SZREG
    .equ FRAME_SCAUSE, 33*SZREG
    .equ FRAME_STVAL, 34*SZREG
    .equ FRAME_SSTATUS, 35*SZREG

    .text
    .align 2
    .globl kernel_vec
kernel_vec:
#if __riscv_xlen == 64
    csrw  sscratch, t0
    addi  t0, sp, -FRAME_SIZE
    srli  t0, t0, KSTACK_SHIFT
    andi  t0, t0, 1
    bnez  t0, kernel_stack_overflow
    csrr  t0, sscratch
#endif
    addi  sp, sp, -FRAME_SIZE
    # regs[0] is x0 (always 0) and regs[2] the sp from before the trap;
    # both are stored once a temporary is free.
    REG_S ra, 1*SZREG(sp)
    REG_S gp, 3*SZREG(sp)
    REG_S tp, 4*SZREG(sp)
    REG_S t0, 5*SZREG(sp)
    REG_S t1, 6*SZREG(sp)
    REG_S t2, 7*SZREG(sp)
    REG_S s0, 8*SZREG(sp)
    REG_S s1, 9*SZREG(sp)
    REG_S a0, 10*SZREG(sp)
    REG_S a1, 11*SZREG(sp)
    REG_S a2, 12*SZREG(sp)
    REG_S a3, 13*SZREG(sp)
    REG_S a4, 14*SZREG(sp)
    REG_S a5, 15*SZREG(sp)
    REG_S a6, 16*SZREG(sp)
    REG_S a7, 17*SZREG(sp)
    REG_S s2, 18*SZREG(sp)
    REG_S s3, 19*SZREG(sp)
    REG_S s4, 20*SZREG(sp)
    REG_S s5, 21*SZREG(sp)
    REG_S s6, 22*SZREG(sp)
    REG_S s7, 23*SZREG(sp)
    REG_S s8, 24*SZREG(sp)
    REG_S s9, 25*SZREG(sp)
    REG_S s10, 26*SZREG(sp)
    REG_S s11, 27*SZREG(sp)
    REG_S t3, 28*SZREG(sp)
    REG_S t4, 29*SZREG(sp)
    REG_S t5, 30*SZREG(sp)
    REG_S t6, 31*SZREG(sp)
    REG_S zero, 0*SZREG(sp)
    addi  t0, sp, FRAME_SIZE
    REG_S t0, 2*SZREG(sp)
    csrr  t0, sepc
    REG_S t0, FRAME_SEPC(sp)
    csrr  t0, scause
    REG_S t0, FRAME_SCAUSE(sp)
    csrr  t0, stval
    REG_S t0, FRAME_STVAL(sp)
    csrr  t0, sstatus
    REG_S t0, FRAME_SSTATUS(sp)

    mv    a0, sp
    call  kernel_trap

    REG_L t0, FRAME_SEPC(sp)
    csrw  sepc, t0
    REG_L t0, FRAME_SSTATUS(sp)
    csrw  sstatus, t0
    REG_L ra, 1*SZREG(sp)
    REG_L gp, 3*SZREG(sp)
    REG_L tp, 4*SZREG(sp)
    REG_L t0, 5*SZREG(sp)
    REG_L t1, 6*SZREG(sp)
    REG_L t2, 7*SZREG(sp)
    REG_L s0, 8*SZREG(sp)
    REG_L s1, 9*SZREG(sp)
    REG_L a0, 10*SZREG(sp)
    REG_L a1, 11*SZREG(sp)
    REG_L a2, 12*SZREG(sp)
    REG_L a3, 13*SZREG(sp)
    REG_L a4, 14*SZREG(sp)
    REG_L a5, 15*SZREG(sp)
    REG_L a6, 16*SZREG(sp)
    REG_L a7, 17*SZREG(sp)
    REG_L s2, 18*SZREG(sp)
    REG_L s3, 19*SZREG(sp)
    REG_L s4, 20*SZREG(sp)
    REG_L s5, 21*SZREG(sp)
    REG_L s6, 22*SZREG(sp)
    REG_L s7, 23*SZREG(sp)
    REG_L s8, 24*SZREG(sp)
    REG_L s9, 25*SZREG(sp)
    REG_L s10, 26*SZREG(sp)
    REG_L s11, 27*SZREG(sp)
    REG_L t3, 28*SZREG(sp)
    REG_L t4, 29*SZREG(sp)
    REG_L t5, 30*SZREG(sp)
    REG_L t6, 31*SZREG(sp)
    addi  sp, sp, FRAME_SIZE
    sret

#if __riscv_xlen == 64
# The frame doesn't fit: t0 is in sscratch and sp is the stack pointer that
# ran out, both as the trap found them. The frame goes at the top of this
# hart's overflow stack, overflow_stacks + (tp + 1) * OVERFLOW_STACK_SIZE
# (vm.c). Every register but t0 holds a value to save, so tp, the hart's id,
# is scaled in place and put back, as nothing can interrupt this (the trap
# turned interrupts off).
kernel_stack_overflow:
    mv    t0, sp
    la    sp, overflow_stacks
    addi  tp, tp, 1
    slli  tp, tp, OVERFLOW_STACK_SHIFT
    add   sp, sp, tp
    srli  tp, tp, OVERFLOW_STACK_SHIFT
    addi  tp, tp, -1
    addi  sp, sp, -FRAME_SIZE
    REG_S ra, 1*SZREG(sp)
    REG_S gp, 3*SZREG(sp)
    REG_S tp, 4*SZREG(sp)
    REG_S t1, 6*SZREG(sp)
    REG_S t2, 7*SZREG(sp)
    REG_S s0, 8*SZREG(sp)
    REG_S s1, 9*SZREG(sp)
    REG_S a0, 10*SZREG(sp)
    REG_S a1, 11*SZREG(sp)
    REG_S a2, 12*SZREG(sp)
    REG_S a3, 13*SZREG(sp)
    REG_S a4, 14*SZREG(sp)
    REG_S a5, 15*SZREG(sp)
    REG_S a6, 16*SZREG(sp)
    REG_S a7, 17*SZREG(sp)
    REG_S s2, 18*SZREG(sp)
    REG_S s3, 19*SZREG(sp)
    REG_S s4, 20*SZREG(sp)
    REG_S s5, 21*SZREG(sp)
    REG_S s6, 22*SZREG(sp)
    REG_S s7, 23*SZREG(sp)
    REG_S s8, 24*SZREG(sp)
    REG_S s9, 25*SZREG(sp)
    REG_S s10, 26*SZREG(sp)
    REG_S s11, 27*SZREG(sp)
    REG_S t3, 28*SZREG(sp)
    REG_S t4, 29*SZREG(sp)
    REG_S t5, 30*SZREG(sp)
    REG_S t6, 31*SZREG(sp)
    REG_S zero, 0*SZREG(sp)
    REG_S t0, 2*SZREG(sp)       # the sp that overflowed
    csrr  t0, sscratch
    REG_S t0, 5*SZREG(sp)       # t0
    csrr  t0, sepc
    REG_S t0, FRAME_SEPC(sp)
    csrr  t0, scause
    REG_S t0, FRAME_SCAUSE(sp)
    csrr  t0, stval
    REG_S t0, FRAME_STVAL(sp)
    csrr  t0, sstatus
    REG_S t0, FRAME_SSTATUS(sp)
    mv    a0, sp
    call  kstack_overflow       # panics
1:  j     1b
#endif

# void illegal_instruction(void): for testing the panic path. Fills every
# register but sp with its own number in each byte (x1 = 0x01010101, ...,
# x31 = 0x1f1f1f1f on RV32; x1 = 0x0101010101010101 on RV64), then executes
# unimp, which is `csrrw zero, cycle, zero`: a write to a read-only CSR, so
# illegal in every mode.
    .globl illegal_instruction
illegal_instruction:
#if __riscv_xlen == 64
    li    ra, 0x0101010101010101
    li    gp, 0x0303030303030303
    li    t0, 0x0505050505050505
    li    t1, 0x0606060606060606
    li    t2, 0x0707070707070707
    li    s0, 0x0808080808080808
    li    s1, 0x0909090909090909
    li    a0, 0x0a0a0a0a0a0a0a0a
    li    a1, 0x0b0b0b0b0b0b0b0b
    li    a2, 0x0c0c0c0c0c0c0c0c
    li    a3, 0x0d0d0d0d0d0d0d0d
    li    a4, 0x0e0e0e0e0e0e0e0e
    li    a5, 0x0f0f0f0f0f0f0f0f
    li    a6, 0x1010101010101010
    li    a7, 0x1111111111111111
    li    s2, 0x1212121212121212
    li    s3, 0x1313131313131313
    li    s4, 0x1414141414141414
    li    s5, 0x1515151515151515
    li    s6, 0x1616161616161616
    li    s7, 0x1717171717171717
    li    s8, 0x1818181818181818
    li    s9, 0x1919191919191919
    li    s10, 0x1a1a1a1a1a1a1a1a
    li    s11, 0x1b1b1b1b1b1b1b1b
    li    t3, 0x1c1c1c1c1c1c1c1c
    li    t4, 0x1d1d1d1d1d1d1d1d
    li    t5, 0x1e1e1e1e1e1e1e1e
    li    t6, 0x1f1f1f1f1f1f1f1f
    # tp last: the kernel's tp is the hart's id, which an interrupt handler
    # taken in between (interrupts are on) would need for its locks.
    li    tp, 0x0404040404040404
#else
    li    ra, 0x01010101
    li    gp, 0x03030303
    li    t0, 0x05050505
    li    t1, 0x06060606
    li    t2, 0x07070707
    li    s0, 0x08080808
    li    s1, 0x09090909
    li    a0, 0x0a0a0a0a
    li    a1, 0x0b0b0b0b
    li    a2, 0x0c0c0c0c
    li    a3, 0x0d0d0d0d
    li    a4, 0x0e0e0e0e
    li    a5, 0x0f0f0f0f
    li    a6, 0x10101010
    li    a7, 0x11111111
    li    s2, 0x12121212
    li    s3, 0x13131313
    li    s4, 0x14141414
    li    s5, 0x15151515
    li    s6, 0x16161616
    li    s7, 0x17171717
    li    s8, 0x18181818
    li    s9, 0x19191919
    li    s10, 0x1a1a1a1a
    li    s11, 0x1b1b1b1b
    li    t3, 0x1c1c1c1c
    li    t4, 0x1d1d1d1d
    li    t5, 0x1e1e1e1e
    li    t6, 0x1f1f1f1f
    li    tp, 0x04040404
#endif
    .word 0xc0001073            # unimp
1:  j     1b
