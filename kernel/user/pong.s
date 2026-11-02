# pong: prints "pong 1" to "pong 4", about half a timer tick apart, and exits
# with status 0. See ping.

    .section .text.start
    .globl _start
_start:
    la    a0, name
    li    a1, 4
    call  count

    .section .rodata
name:
    .ascii "pong"
