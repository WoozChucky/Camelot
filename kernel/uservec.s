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

    .equ FRAME_SEPC, 128
    .equ FRAME_SCAUSE, 132
    .equ FRAME_STVAL, 136
    .equ FRAME_SSTATUS, 140
    .equ FRAME_KERNEL_TP, 144

    .text
    .align 2
    .globl user_vec
user_vec:
    csrrw a0, sscratch, a0      # a0 = the frame, sscratch = the user's a0
    sw    ra, 4(a0)
    sw    sp, 8(a0)
    sw    gp, 12(a0)
    sw    tp, 16(a0)
    sw    t0, 20(a0)
    sw    t1, 24(a0)
    sw    t2, 28(a0)
    sw    s0, 32(a0)
    sw    s1, 36(a0)
    sw    a1, 44(a0)
    sw    a2, 48(a0)
    sw    a3, 52(a0)
    sw    a4, 56(a0)
    sw    a5, 60(a0)
    sw    a6, 64(a0)
    sw    a7, 68(a0)
    sw    s2, 72(a0)
    sw    s3, 76(a0)
    sw    s4, 80(a0)
    sw    s5, 84(a0)
    sw    s6, 88(a0)
    sw    s7, 92(a0)
    sw    s8, 96(a0)
    sw    s9, 100(a0)
    sw    s10, 104(a0)
    sw    s11, 108(a0)
    sw    t3, 112(a0)
    sw    t4, 116(a0)
    sw    t5, 120(a0)
    sw    t6, 124(a0)
    sw    zero, 0(a0)
    csrr  t0, sscratch
    sw    t0, 40(a0)            # the user's a0
    csrr  t0, sepc
    sw    t0, FRAME_SEPC(a0)
    csrr  t0, scause
    sw    t0, FRAME_SCAUSE(a0)
    csrr  t0, stval
    sw    t0, FRAME_STVAL(a0)
    csrr  t0, sstatus
    sw    t0, FRAME_SSTATUS(a0)

    # The program may have used tp for anything (it is saved above): the
    # kernel's tp is the hart's id, which mycpu() and the locks rely on.
    lw    tp, FRAME_KERNEL_TP(a0)
    mv    sp, a0                # the kernel stack grows down from the frame
    call  user_trap             # returns to U-mode through user_return
1:  j     1b

# void user_return(struct trap_frame *f): back to U-mode with the registers,
# sepc and sstatus in f. Interrupts must be off and stvec = user_vec
# (user_trap_return does both).
    .globl user_return
user_return:
    csrw  sscratch, a0          # where user_vec saves the next trap
    lw    t0, FRAME_SEPC(a0)
    csrw  sepc, t0
    lw    t0, FRAME_SSTATUS(a0)
    csrw  sstatus, t0
    lw    ra, 4(a0)
    lw    sp, 8(a0)
    lw    gp, 12(a0)
    lw    tp, 16(a0)
    lw    t0, 20(a0)
    lw    t1, 24(a0)
    lw    t2, 28(a0)
    lw    s0, 32(a0)
    lw    s1, 36(a0)
    lw    a1, 44(a0)
    lw    a2, 48(a0)
    lw    a3, 52(a0)
    lw    a4, 56(a0)
    lw    a5, 60(a0)
    lw    a6, 64(a0)
    lw    a7, 68(a0)
    lw    s2, 72(a0)
    lw    s3, 76(a0)
    lw    s4, 80(a0)
    lw    s5, 84(a0)
    lw    s6, 88(a0)
    lw    s7, 92(a0)
    lw    s8, 96(a0)
    lw    s9, 100(a0)
    lw    s10, 104(a0)
    lw    s11, 108(a0)
    lw    t3, 112(a0)
    lw    t4, 116(a0)
    lw    t5, 120(a0)
    lw    t6, 124(a0)
    lw    a0, 40(a0)            # last: it held the frame's address
    sret
