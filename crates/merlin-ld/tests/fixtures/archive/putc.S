# putc: a byte to the UART, once its transmitter is free; counts the bytes
# in `written` (.bss).
    .equ UART_BASE, 0x10000000
    .equ UART_LSR,  5
    .equ LSR_THRE,  0x20

    .text
    .globl putc
putc:
    li    t0, UART_BASE
1:  lbu   t1, UART_LSR(t0)
    andi  t1, t1, LSR_THRE
    beqz  t1, 1b
    sb    a0, 0(t0)
    la    t0, written
    lw    t1, 0(t0)
    addi  t1, t1, 1
    sw    t1, 0(t0)
    ret

    .bss
    .balign 4
    .globl written
written:
    .zero 4
