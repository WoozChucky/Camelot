# ping: prints "ping 1" to "ping 4", about half a timer tick apart, and exits
# with status 0. Run beside pong (`run ping pong`), the two take turns: each
# timer tick hands the CPU to the other one.

    .section .text.start
    .globl _start
_start:
    la    a0, name
    li    a1, 4
    call  count

    .section .rodata
name:
    .ascii "ping"
