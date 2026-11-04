# util.rv32.s for RV64: the same functions and data, with `ra` saved as a
# doubleword and the int arithmetic in W instructions, which keep a 32-bit
# result sign-extended as the calling convention wants it.

    .text
    .globl triple
# int triple(int x)
triple:
    slliw t0, a0, 1
    addw  a0, a0, t0
    ret

    .globl square_plus_one
# int square_plus_one(int x): square(x) + 1, with square in main.c.
square_plus_one:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    call  square
    addiw a0, a0, 1
    ld    ra, 8(sp)
    addi  sp, sp, 16
    ret

    .section .rodata
    .globl table
    .balign 4
# const int table[4]
table:
    .word 1, 2, 3, 4
