# The S-mode trap vector. start() points stvec here. Saves x1-x31 and the trap
# CSRs on the stack as a struct trap_frame (include/trap.h), calls
# kernel_trap(frame), then restores everything and returns with sret. sepc and
# sstatus are restored from the frame, so kernel_trap may change them.
#
# stvec points here only while the hart is in S-mode, so the interrupted sp
# is a kernel stack and the frame goes just below it. Traps from U-mode go to
# user_vec (uservec.s) instead, which finds the process's kernel stack
# through sscratch.

    .equ FRAME_SIZE, 144        # TRAP_FRAME_SIZE, a multiple of 16
    .equ FRAME_SEPC, 128
    .equ FRAME_SCAUSE, 132
    .equ FRAME_STVAL, 136
    .equ FRAME_SSTATUS, 140

    .text
    .align 2
    .globl kernel_vec
kernel_vec:
    addi  sp, sp, -FRAME_SIZE
    # regs[i] at 4*i. regs[0] is x0 (always 0) and regs[2] the sp from
    # before the trap; both are stored once a temporary is free.
    sw    ra, 4(sp)
    sw    gp, 12(sp)
    sw    tp, 16(sp)
    sw    t0, 20(sp)
    sw    t1, 24(sp)
    sw    t2, 28(sp)
    sw    s0, 32(sp)
    sw    s1, 36(sp)
    sw    a0, 40(sp)
    sw    a1, 44(sp)
    sw    a2, 48(sp)
    sw    a3, 52(sp)
    sw    a4, 56(sp)
    sw    a5, 60(sp)
    sw    a6, 64(sp)
    sw    a7, 68(sp)
    sw    s2, 72(sp)
    sw    s3, 76(sp)
    sw    s4, 80(sp)
    sw    s5, 84(sp)
    sw    s6, 88(sp)
    sw    s7, 92(sp)
    sw    s8, 96(sp)
    sw    s9, 100(sp)
    sw    s10, 104(sp)
    sw    s11, 108(sp)
    sw    t3, 112(sp)
    sw    t4, 116(sp)
    sw    t5, 120(sp)
    sw    t6, 124(sp)
    sw    zero, 0(sp)
    addi  t0, sp, FRAME_SIZE
    sw    t0, 8(sp)
    csrr  t0, sepc
    sw    t0, FRAME_SEPC(sp)
    csrr  t0, scause
    sw    t0, FRAME_SCAUSE(sp)
    csrr  t0, stval
    sw    t0, FRAME_STVAL(sp)
    csrr  t0, sstatus
    sw    t0, FRAME_SSTATUS(sp)

    mv    a0, sp
    call  kernel_trap

    lw    t0, FRAME_SEPC(sp)
    csrw  sepc, t0
    lw    t0, FRAME_SSTATUS(sp)
    csrw  sstatus, t0
    lw    ra, 4(sp)
    lw    gp, 12(sp)
    lw    tp, 16(sp)
    lw    t0, 20(sp)
    lw    t1, 24(sp)
    lw    t2, 28(sp)
    lw    s0, 32(sp)
    lw    s1, 36(sp)
    lw    a0, 40(sp)
    lw    a1, 44(sp)
    lw    a2, 48(sp)
    lw    a3, 52(sp)
    lw    a4, 56(sp)
    lw    a5, 60(sp)
    lw    a6, 64(sp)
    lw    a7, 68(sp)
    lw    s2, 72(sp)
    lw    s3, 76(sp)
    lw    s4, 80(sp)
    lw    s5, 84(sp)
    lw    s6, 88(sp)
    lw    s7, 92(sp)
    lw    s8, 96(sp)
    lw    s9, 100(sp)
    lw    s10, 104(sp)
    lw    s11, 108(sp)
    lw    t3, 112(sp)
    lw    t4, 116(sp)
    lw    t5, 120(sp)
    lw    t6, 124(sp)
    addi  sp, sp, FRAME_SIZE
    sret

# void illegal_instruction(void): for testing the panic path. Fills every
# register but sp with its own number in each byte (x1 = 0x01010101, ...,
# x31 = 0x1f1f1f1f), then executes unimp, which is `csrrw zero, cycle, zero`:
# a write to a read-only CSR, so illegal in every mode.
    .globl illegal_instruction
illegal_instruction:
    li    ra, 0x01010101
    li    gp, 0x03030303
    li    tp, 0x04040404
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
    .word 0xc0001073            # unimp
1:  j     1b
