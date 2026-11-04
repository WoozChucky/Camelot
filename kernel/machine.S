# The M-mode trap handler, the same on every hart. start() points mtvec here
# and mscratch at the hart's block of machine_scratch. Every exception except
# ecall-from-S and every S-level interrupt is delegated to S-mode, so only
# these get here:
#
# * The machine timer (mcause 0x8000_0007 on RV32, 0x8000_0000_0000_0007 on
#   RV64: the interrupt bit is the top one, XLEN-1). Rearm the hart's mtimecmp one
#   interval later, note the tick in the scratch block and raise the S-mode
#   software interrupt (mip.SSIP), which the kernel counts as a tick.
#   Excalibur has no Sstc, so S-mode can't program the timer itself; this is
#   the usual forwarding trampoline.
# * A machine software interrupt (mcause 3 with the interrupt bit): an IPI, which another
#   hart sent with MCALL_IPI. Acknowledge it (msip = 0) and raise SSIP too,
#   without the tick: the kernel looks at what the other hart asked for
#   (spec section 10, "Kernel SMP"). SSIP is the kernel's one doorbell, so
#   the tick word says whether a timer tick rang it.
# * A machine call: ecall from S-mode (mcause 9), function number in a7.
#     a7 = 0 (MCALL_HALT): stop the machine with exit code a0.
#     a7 = 1 (MCALL_IPI): set msip of every hart whose bit is set in a0;
#                         returns 0.
#   Unknown functions return -1 in a0.
# * Anything else is a kernel bug and goes to early_trap (exit code 0xff).
#
# Runs with no stack: a1-a3 are saved in the scratch block, a0 in mscratch.
# M-mode does not translate, so everything here is at its physical address:
# the scratch block, the CLINT, the code itself (the RV64 kernel is linked
# high, but machine_vec is entered at the physical address start() put in
# mtvec). The scratch block's layout (include/machine.h), in slots a
# register wide (SZREG, asm.h: 4 bytes on RV32, 8 on RV64):
#   0, 1, 2   saved a1, a2, a3
#   3         timer interval in mtime ticks
#   4         the address of this hart's mtimecmp
#   5         1 when the last SSIP came with a timer tick (a u32)

#include <asm.h>

    .equ CLINT_MSIP, 0x02000000
#if __riscv_xlen == 64
    .equ MCAUSE_MSI, 0x8000000000000003
    .equ MCAUSE_MTI, 0x8000000000000007
#else
    .equ MCAUSE_MSI, 0x80000003
    .equ MCAUSE_MTI, 0x80000007
#endif
    .equ MCAUSE_ECALL_S, 9
    .equ MCALL_HALT, 0
    .equ MCALL_IPI, 1
    .equ SCRATCH_A1, 0*SZREG
    .equ SCRATCH_A2, 1*SZREG
    .equ SCRATCH_A3, 2*SZREG
    .equ SCRATCH_INTERVAL, 3*SZREG
    .equ SCRATCH_MTIMECMP, 4*SZREG
    .equ SCRATCH_TICK, 5*SZREG
    .equ MIP_SSIP, 2

    .text
    .align 2
    .globl machine_vec
machine_vec:
    csrrw a0, mscratch, a0      # a0 = scratch block, mscratch = caller's a0
    REG_S a1, SCRATCH_A1(a0)
    REG_S a2, SCRATCH_A2(a0)
    REG_S a3, SCRATCH_A3(a0)
    csrr  a1, mcause
    li    a2, MCAUSE_MTI
    beq   a1, a2, machine_timer
    li    a2, MCAUSE_MSI
    beq   a1, a2, machine_soft
    li    a2, MCAUSE_ECALL_S
    beq   a1, a2, machine_call
    j     early_trap

# mtimecmp += interval: on RV64 one 8-byte load and store, on RV32 a 64-bit
# add done in two halves. M-mode runs with mstatus.MIE = 0, so the
# half-written compare value can't fire.
machine_timer:
    REG_L a1, SCRATCH_MTIMECMP(a0)
#if __riscv_xlen == 64
    ld    a2, 0(a1)
    REG_L a3, SCRATCH_INTERVAL(a0)
    add   a2, a2, a3
    sd    a2, 0(a1)
#else
    lw    a2, 0(a1)             # low word
    lw    a3, SCRATCH_INTERVAL(a0)
    add   a3, a2, a3
    sltu  a2, a3, a2            # carry into the high word
    sw    a3, 0(a1)
    lw    a3, 4(a1)
    add   a3, a3, a2
    sw    a3, 4(a1)
#endif
    li    a1, 1
    sw    a1, SCRATCH_TICK(a0)  # this SSIP is a tick (trap.c swaps it back to 0)
    csrsi mip, MIP_SSIP         # forward the tick to S-mode
    j     machine_vec_return

# msip[mhartid] = 0, then pass the IPI on to S-mode.
machine_soft:
    csrr  a1, mhartid
    slli  a1, a1, 2
    li    a2, CLINT_MSIP
    add   a1, a1, a2
    sw    zero, 0(a1)
    csrsi mip, MIP_SSIP
    j     machine_vec_return

machine_call:
    csrr  a1, mepc
    addi  a1, a1, 4             # resume after the ecall
    csrw  mepc, a1
    li    a2, MCALL_HALT
    beq   a7, a2, machine_call_halt
    li    a2, MCALL_IPI
    bne   a7, a2, machine_call_unknown
    # msip[i] = 1 for every bit i of the caller's a0, from bit 0 up.
    csrr  a1, mscratch          # the caller's a0: the harts
    li    a2, CLINT_MSIP
1:  beqz  a1, 3f
    andi  a3, a1, 1
    beqz  a3, 2f
    sw    a3, 0(a2)             # a3 = 1
2:  srli  a1, a1, 1
    addi  a2, a2, 4
    j     1b
3:  REG_L a1, SCRATCH_A1(a0)
    REG_L a2, SCRATCH_A2(a0)
    REG_L a3, SCRATCH_A3(a0)
    csrrw a0, mscratch, a0
    li    a0, 0
    mret

machine_call_halt:
    csrr  a0, mscratch          # the caller's a0: the exit code
    .word 0x0005100b            # xcal.halt a0
1:  j     1b

machine_call_unknown:
    REG_L a1, SCRATCH_A1(a0)
    REG_L a2, SCRATCH_A2(a0)
    REG_L a3, SCRATCH_A3(a0)
    csrrw a0, mscratch, a0
    li    a0, -1
    mret

machine_vec_return:
    REG_L a1, SCRATCH_A1(a0)
    REG_L a2, SCRATCH_A2(a0)
    REG_L a3, SCRATCH_A3(a0)
    csrrw a0, mscratch, a0
    mret

# void machine_halt(u32 code): MCALL_HALT from S-mode. Never returns.
    .globl machine_halt
machine_halt:
    li    a7, MCALL_HALT
    ecall
1:  j     1b

# void machine_ipi(u32 harts): MCALL_IPI from S-mode. The ecall changes a0
# (the result, 0) and nothing else.
    .globl machine_ipi
machine_ipi:
    li    a7, MCALL_IPI
    ecall
    ret
