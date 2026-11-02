# Camelot kernel entry. The boot ROM jumps here (0x8000_0000) in M-mode with
# interrupts off. Set up a trap vector and a stack, clear .bss, run kmain(),
# then halt with its return value as the exit code.
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
    la    sp, __stack_top

    # Zero .bss. The linker script aligns both ends to 4 bytes.
    la    t0, __bss_start
    la    t1, __bss_end
1:  bgeu  t0, t1, 2f
    sw    zero, 0(t0)
    addi  t0, t0, 4
    j     1b

2:  call  kmain
    .word 0x0005100b            # xcal.halt a0
3:  j     3b

    .text

# void xcal_dbg(u32 val)
    .globl xcal_dbg
xcal_dbg:
    .word 0x0005000b            # xcal.dbg a0
    ret

# void xcal_halt(u32 code)
    .globl xcal_halt
xcal_halt:
    .word 0x0005100b            # xcal.halt a0
1:  j     1b

# Any trap before the kernel installs real handlers (#17) is a bug: show
# mcause, mepc and mtval on the debug console and stop with exit code 0xff.
    .align 2
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
