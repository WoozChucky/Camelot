# The second object of examples/multiobj (see main.S): a puts that counts what
# it writes, and the strings main.S prints.

    .equ UART_BASE, 0x10000000
    .equ UART_LSR,  5
    .equ LSR_THRE,  0x20

    .text
    .globl puts
# puts(a0 = NUL-terminated string) -> a0 = bytes written, also added to
# bytes_written
puts:
    li    t0, UART_BASE
    mv    t1, a0
1:  lbu   t2, 0(t1)
    beqz  t2, 3f
2:  lbu   t3, UART_LSR(t0)      # wait until the transmitter is free
    andi  t3, t3, LSR_THRE
    beqz  t3, 2b
    sb    t2, 0(t0)
    addi  t1, t1, 1
    j     1b
3:  sub   a0, t1, a0
    la    t4, bytes_written
    lw    t5, 0(t4)
    add   t5, t5, a0
    sw    t5, 0(t4)
    ret

    .section .rodata
    .globl greeting, farewell
greeting:
    .asciz "Hello from two objects!\n"
farewell:
    .asciz "puts lives in print.S.\n"

    .bss
    .balign 4
    .globl bytes_written
bytes_written:
    .zero 4
