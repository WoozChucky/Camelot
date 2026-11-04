# Interrupt-driven UART echo for Grail: every byte typed on stdin raises a machine
# external interrupt (MEI) through the PLIC, and the handler echoes it. The main
# loop only sleeps in wfi, so nothing is echoed unless the interrupt path works.
#
# One byte per interrupt: claim, read RBR, complete, echo. If more bytes are waiting,
# the UART's line is still high after the complete, so the PLIC raises it again.
# Receiving EOT (0x04, not echoed) prints the interrupt count with xcal.dbg and halts
# with exit code 0. Any unexpected trap halts with exit code 1.
#
# uart-echo-rv64.S is the same program for RV64 machines.
#
# Built with our own tools by `cargo xtask examples uart-echo`, which does the same as
#
#   merlin-as examples/uart-echo/uart-echo.S -o uart-echo.o
#   merlin-ld -T examples/uart-echo/link.ld uart-echo.o -o examples/uart-echo/uart-echo.elf
#
# Reference build with LLVM, only for comparison:
#
#   clang --target=riscv32 -march=rv32im_zicsr -mabi=ilp32 -mno-relax -nostdlib -static \
#         -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -T examples/uart-echo/link.ld examples/uart-echo/uart-echo.S -o uart-echo.elf
#
# Try it: printf 'hello\n\004' | grail examples/uart-echo/uart-echo.elf

    .equ UART_BASE,     0x10000000
    .equ UART_RBR,      0               # receive buffer (read)
    .equ UART_THR,      0               # transmit holding (write)
    .equ UART_IER,      1
    .equ UART_LSR,      5
    .equ IER_ERBFI,     0x01            # received data available interrupt
    .equ LSR_THRE,      0x20

    .equ PLIC_BASE,     0x0c000000
    .equ PLIC_ENABLE0,  0x0c002000      # context 0 = hart 0 M-mode
    .equ PLIC_THRESH0,  0x0c200000
    .equ PLIC_CLAIM0,   0x0c200004
    .equ UART_IRQ,      1               # PLIC source of the UART

    .equ MIE_MEIE,      0x800           # mie bit 11
    .equ MSTATUS_MIE,   0x8
    .equ CAUSE_MEI,     0x8000000b

    .section .text.start
    .globl _start
_start:
    la    sp, __stack_top
    la    t0, trap
    csrw  mtvec, t0
    li    s0, 0                         # s0 = interrupts taken

    # PLIC: source 1 priority 1, enabled for context 0, threshold 0.
    li    t0, PLIC_BASE
    li    t1, 1
    sw    t1, 4*UART_IRQ(t0)
    li    t0, PLIC_ENABLE0
    li    t1, 1 << UART_IRQ
    sw    t1, 0(t0)
    li    t0, PLIC_THRESH0
    sw    zero, 0(t0)

    # UART: interrupt when a received byte is waiting.
    li    t0, UART_BASE
    li    t1, IER_ERBFI
    sb    t1, UART_IER(t0)

    # Hart: take machine external interrupts.
    li    t0, MIE_MEIE
    csrs  mie, t0
    csrsi mstatus, MSTATUS_MIE

idle:
    wfi
    j     idle

    .p2align 2
trap:
    csrr  t0, mcause
    li    t1, CAUSE_MEI
    bne   t0, t1, fail
    addi  s0, s0, 1

    li    t2, PLIC_CLAIM0
    lw    t3, 0(t2)                     # claim
    li    t1, UART_IRQ
    bne   t3, t1, fail

    li    t4, UART_BASE
    lbu   a0, UART_RBR(t4)              # read the byte (drops the UART's line)
    sw    t3, 0(t2)                     # complete

    li    t1, 4
    beq   a0, t1, done

1:  lbu   t1, UART_LSR(t4)              # echo it
    andi  t1, t1, LSR_THRE
    beqz  t1, 1b
    sb    a0, UART_THR(t4)
    mret

done:
    # How many interrupts were taken (one per byte, EOT included).
    xcal.dbg s0
    xcal.halt zero

fail:
    # Show the unexpected mcause, then halt with exit code 1.
    xcal.dbg t0
    li    t0, 1
    xcal.halt t0
