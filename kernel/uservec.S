# Traps from U-mode, and the way back (spec section 10, "Kernel processes").
#
# Every process's page table maps the kernel too (without U), so this code
# and the kernel stacks are reachable as they are: unlike xv6, there is no
# trampoline page and no satp switch on a trap. What a user trap needs is a
# kernel stack, since the user's sp can't be trusted. user_return leaves
# sscratch pointing at the process's trap frame, which sits at the top of its
# kernel stack page; user_vec swaps it into a0, saves every register there
# (the same struct trap_frame as kernel_vec's, include/trap.h), and runs
# user_trap() with sp just below the frame. So each trap from U-mode starts
# on an empty kernel stack. The user's tp is saved like the others and tp is
# set to the frame's kernel_tp, the hart's id (as xv6's kernel_hartid).
#
# user_trap_return() points stvec here, with interrupts off, right before
# user_return; user_trap() points it back at kernel_vec first thing.
#
# Every slot is a register wide (SZREG, asm.h), as in kernelvec.S.

#include <asm.h>

    .equ FRAME_SEPC, 32*SZREG
    .equ FRAME_SCAUSE, 33*SZREG
    .equ FRAME_STVAL, 34*SZREG
    .equ FRAME_SSTATUS, 35*SZREG
    .equ FRAME_KERNEL_TP, 36*SZREG

    .text
    .align 2
    .globl user_vec
user_vec:
    csrrw a0, sscratch, a0      # a0 = the frame, sscratch = the user's a0
    REG_S ra, 1*SZREG(a0)
    REG_S sp, 2*SZREG(a0)
    REG_S gp, 3*SZREG(a0)
    REG_S tp, 4*SZREG(a0)
    REG_S t0, 5*SZREG(a0)
    REG_S t1, 6*SZREG(a0)
    REG_S t2, 7*SZREG(a0)
    REG_S s0, 8*SZREG(a0)
    REG_S s1, 9*SZREG(a0)
    REG_S a1, 11*SZREG(a0)
    REG_S a2, 12*SZREG(a0)
    REG_S a3, 13*SZREG(a0)
    REG_S a4, 14*SZREG(a0)
    REG_S a5, 15*SZREG(a0)
    REG_S a6, 16*SZREG(a0)
    REG_S a7, 17*SZREG(a0)
    REG_S s2, 18*SZREG(a0)
    REG_S s3, 19*SZREG(a0)
    REG_S s4, 20*SZREG(a0)
    REG_S s5, 21*SZREG(a0)
    REG_S s6, 22*SZREG(a0)
    REG_S s7, 23*SZREG(a0)
    REG_S s8, 24*SZREG(a0)
    REG_S s9, 25*SZREG(a0)
    REG_S s10, 26*SZREG(a0)
    REG_S s11, 27*SZREG(a0)
    REG_S t3, 28*SZREG(a0)
    REG_S t4, 29*SZREG(a0)
    REG_S t5, 30*SZREG(a0)
    REG_S t6, 31*SZREG(a0)
    REG_S zero, 0*SZREG(a0)
    csrr  t0, sscratch
    REG_S t0, 10*SZREG(a0)            # the user's a0
    csrr  t0, sepc
    REG_S t0, FRAME_SEPC(a0)
    csrr  t0, scause
    REG_S t0, FRAME_SCAUSE(a0)
    csrr  t0, stval
    REG_S t0, FRAME_STVAL(a0)
    csrr  t0, sstatus
    REG_S t0, FRAME_SSTATUS(a0)

    # The program may have used tp for anything (it is saved above): the
    # kernel's tp is the hart's id, which mycpu() and the locks rely on.
    REG_L tp, FRAME_KERNEL_TP(a0)
    mv    sp, a0                # the kernel stack grows down from the frame
    call  user_trap             # returns to U-mode through user_return
1:  j     1b

# void user_return(struct trap_frame *f): back to U-mode with the registers,
# sepc and sstatus in f. Interrupts must be off and stvec = user_vec
# (user_trap_return does both).
    .globl user_return
user_return:
    csrw  sscratch, a0          # where user_vec saves the next trap
    REG_L t0, FRAME_SEPC(a0)
    csrw  sepc, t0
    REG_L t0, FRAME_SSTATUS(a0)
    csrw  sstatus, t0
    REG_L ra, 1*SZREG(a0)
    REG_L sp, 2*SZREG(a0)
    REG_L gp, 3*SZREG(a0)
    REG_L tp, 4*SZREG(a0)
    REG_L t0, 5*SZREG(a0)
    REG_L t1, 6*SZREG(a0)
    REG_L t2, 7*SZREG(a0)
    REG_L s0, 8*SZREG(a0)
    REG_L s1, 9*SZREG(a0)
    REG_L a1, 11*SZREG(a0)
    REG_L a2, 12*SZREG(a0)
    REG_L a3, 13*SZREG(a0)
    REG_L a4, 14*SZREG(a0)
    REG_L a5, 15*SZREG(a0)
    REG_L a6, 16*SZREG(a0)
    REG_L a7, 17*SZREG(a0)
    REG_L s2, 18*SZREG(a0)
    REG_L s3, 19*SZREG(a0)
    REG_L s4, 20*SZREG(a0)
    REG_L s5, 21*SZREG(a0)
    REG_L s6, 22*SZREG(a0)
    REG_L s7, 23*SZREG(a0)
    REG_L s8, 24*SZREG(a0)
    REG_L s9, 25*SZREG(a0)
    REG_L s10, 26*SZREG(a0)
    REG_L s11, 27*SZREG(a0)
    REG_L t3, 28*SZREG(a0)
    REG_L t4, 29*SZREG(a0)
    REG_L t5, 30*SZREG(a0)
    REG_L t6, 31*SZREG(a0)
    REG_L a0, 10*SZREG(a0)            # last: it held the frame's address
    sret
