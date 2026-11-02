# The M-mode trap handler. start() points mtvec here and mscratch at
# machine_scratch. Every exception except ecall-from-S and every S-level
# interrupt is delegated to S-mode, so only these get here:
#
# * The machine timer (mcause 0x8000_0007). Rearm mtimecmp one interval later
#   and raise the S-mode software interrupt (mip.SSIP), which the kernel
#   counts as a tick. Excalibur has no Sstc, so S-mode can't program the timer
#   itself; this is the usual forwarding trampoline.
# * A machine call: ecall from S-mode (mcause 9), function number in a7.
#     a7 = 0 (MCALL_HALT): stop the machine with exit code a0.
#   Unknown functions return -1 in a0.
# * Anything else is a kernel bug and goes to early_trap (exit code 0xff).
#
# Runs with no stack: a1-a3 are saved in machine_scratch, a0 in mscratch.
# machine_scratch layout (include/machine.h):
#   0, 4, 8   saved a1, a2, a3
#   12        timer interval in mtime ticks

    .equ CLINT_MTIMECMP, 0x02004000
    .equ MCAUSE_MTI, 0x80000007
    .equ MCAUSE_ECALL_S, 9
    .equ MCALL_HALT, 0
    .equ SCRATCH_INTERVAL, 12
    .equ MIP_SSIP, 2

    .text
    .align 2
    .globl machine_vec
machine_vec:
    csrrw a0, mscratch, a0      # a0 = scratch area, mscratch = caller's a0
    sw    a1, 0(a0)
    sw    a2, 4(a0)
    sw    a3, 8(a0)
    csrr  a1, mcause
    li    a2, MCAUSE_MTI
    beq   a1, a2, machine_timer
    li    a2, MCAUSE_ECALL_S
    beq   a1, a2, machine_call
    j     early_trap

# mtimecmp += interval, a 64-bit add done in two halves. M-mode runs with
# mstatus.MIE = 0, so the half-written compare value can't fire.
machine_timer:
    li    a1, CLINT_MTIMECMP
    lw    a2, 0(a1)             # low word
    lw    a3, SCRATCH_INTERVAL(a0)
    add   a3, a2, a3
    sltu  a2, a3, a2            # carry into the high word
    sw    a3, 0(a1)
    lw    a3, 4(a1)
    add   a3, a3, a2
    sw    a3, 4(a1)
    csrsi mip, MIP_SSIP         # forward the tick to S-mode
    j     machine_vec_return

machine_call:
    csrr  a1, mepc
    addi  a1, a1, 4             # resume after the ecall
    csrw  mepc, a1
    li    a2, MCALL_HALT
    bne   a7, a2, machine_call_unknown
    csrr  a0, mscratch          # the caller's a0: the exit code
    .word 0x0005100b            # xcal.halt a0
1:  j     1b

machine_call_unknown:
    lw    a1, 0(a0)
    lw    a2, 4(a0)
    lw    a3, 8(a0)
    csrrw a0, mscratch, a0
    li    a0, -1
    mret

machine_vec_return:
    lw    a1, 0(a0)
    lw    a2, 4(a0)
    lw    a3, 8(a0)
    csrrw a0, mscratch, a0
    mret

# void machine_halt(u32 code): MCALL_HALT from S-mode. Never returns.
    .globl machine_halt
machine_halt:
    li    a7, MCALL_HALT
    ecall
1:  j     1b
