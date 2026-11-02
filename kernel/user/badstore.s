# badstore: stores to its own read-only data. The loader maps .rodata
# without W, so the store must fault and the kernel must kill the program.

    .text
    .globl main
main:
    la    t0, msg
    sw    zero, 0(t0)
    li    a0, 1                 # not reached
    call  exit

    .section .rodata
msg:
    .asciz "read only\n"
