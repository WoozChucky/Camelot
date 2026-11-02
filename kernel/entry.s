# Camelot kernel entry. The boot ROM jumps here (0x8000_0000) in M-mode with
# interrupts off: hart 0 at reset, every other hart when hart 0 wakes it with
# an IPI (spec section 7, "Boot"; smp.c). Set up a trap vector and a stack,
# clear .bss (hart 0 only), let start() configure the hart, drop to S-mode
# and run kmain() there (hart_main() on the other harts), then halt with its
# return value as the exit code.
#
# Kept to the merlin-as dialect (no .insn, no preprocessor), so our assembler
# can take over from clang for this file. The Xcal instructions are written as
# .word because stock assemblers don't know them:
#   xcal.dbg  a0 = 0x0005000b   (rs1 = a0 = x10, funct3 = 000, custom-0)
#   xcal.halt a0 = 0x0005100b   (rs1 = a0 = x10, funct3 = 001, custom-0)

    .equ EXIT_UNEXPECTED_TRAP, 0xff

    .section .text.start
    .globl _start
_start:
    la    t0, early_trap
    csrw  mtvec, t0
    # tp = the hart's id, for good: mycpu() is &cpus[tp] (spinlock.h). mret
    # keeps it, and only user programs change it, which user_vec undoes.
    csrr  tp, mhartid
    bnez  tp, other_hart
    la    sp, __stack_top

    # Zero .bss. The linker script aligns both ends to 4 bytes.
    la    t0, __bss_start
    la    t1, __bss_end
1:  bgeu  t0, t1, 2f
    sw    zero, 0(t0)
    addi  t0, t0, 4
    j     1b

    # start() (start.c) sets up delegation, the trap vectors and the timer,
    # and leaves mepc = supervisor_entry and mstatus.MPP = S.
2:  call  start
    mret

# Any other hart: hart 0 has booted the kernel, put the top of a stack page
# for this hart in hart_stacks[tp] (smp.c) and sent the IPI that brought the
# hart here from the boot ROM's park loop. .bss is hart 0's by now: leave it.
other_hart:
    la    t0, hart_stacks
    slli  t1, tp, 2
    add   t0, t0, t1
    lw    sp, 0(t0)
    call  start
    mret

    .text

# S-mode from here on, on the same stack. xcal.halt is M-only, so the exit
# code goes to M-mode as a machine call. The other harts never return from
# hart_main(): they run the scheduler until hart 0 stops the machine.
    .globl supervisor_entry
supervisor_entry:
    bnez  tp, 2f
    call  kmain
    call  machine_halt
1:  j     1b
2:  call  hart_main
    j     1b

# void xcal_dbg(u32 val)
    .globl xcal_dbg
xcal_dbg:
    .word 0x0005000b            # xcal.dbg a0
    ret

# void xcal_halt(u32 code), M-mode only
    .globl xcal_halt
xcal_halt:
    .word 0x0005100b            # xcal.halt a0
1:  j     1b

# A trap in M-mode is a bug: before start() installs machine_vec, or one that
# machine_vec doesn't handle. Show mcause, mepc and mtval on the debug console
# and stop with exit code 0xff.
    .align 2
    .globl early_trap
early_trap:
    csrr  a0, mcause
    .word 0x0005000b            # xcal.dbg a0
    csrr  a0, mepc
    .word 0x0005000b            # xcal.dbg a0
    csrr  a0, mtval
    .word 0x0005000b            # xcal.dbg a0
    li    a0, EXIT_UNEXPECTED_TRAP
    .word 0x0005100b            # xcal.halt a0
1:  j     1b
