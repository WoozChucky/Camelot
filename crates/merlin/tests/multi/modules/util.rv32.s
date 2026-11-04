# Hand-written assembly linked with the C files on RV32 (util.rv64.s on
# RV64): functions C calls, one that calls back into C, and data C reads.

    .text
    .globl triple
# int triple(int x)
triple:
    slli  t0, a0, 1
    add   a0, a0, t0
    ret

    .globl square_plus_one
# int square_plus_one(int x): square(x) + 1, with square in main.c.
square_plus_one:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    call  square
    addi  a0, a0, 1
    lw    ra, 12(sp)
    addi  sp, sp, 16
    ret

    .section .rodata
    .globl table
    .balign 4
# const int table[4]
table:
    .word 1, 2, 3, 4
