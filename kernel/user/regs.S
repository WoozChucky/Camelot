# regs: checks that being interrupted and switched out leaves every register
# as it was. Sets each register xN (but t6) to a pattern of its own (see
# PATTERN), spins for about three timer ticks with t6 as the counter, then
# compares. Exits with status 0 if all of them still hold their value, else
# with the number of the first one that changed. Run it beside other
# programs (`regs & ding & dong`) so that their values pass through the same
# registers.

#include <asm.h>

# Register xN's pattern: N in each byte, and N + 0x80 in each byte of the
# upper half (x1's is 0x8181818101010101), so that every bit of the
# register counts. An upper half that is neither zero nor the low half's
# sign extended: a register the kernel saved with sw and restored with ld,
# or saved whole and restored with lw, comes back different.
#define PATTERN(n, high) 0x##high##high##high##high##n##n##n##n

    .equ SPIN, 1500000

    .text
    .globl main
main:
    li    ra,  PATTERN(01, 81)
    li    sp,  PATTERN(02, 82)  # nothing here uses the stack
    li    gp,  PATTERN(03, 83)
    li    tp,  PATTERN(04, 84)
    li    t0,  PATTERN(05, 85)
    li    t1,  PATTERN(06, 86)
    li    t2,  PATTERN(07, 87)
    li    s0,  PATTERN(08, 88)
    li    s1,  PATTERN(09, 89)
    li    a0,  PATTERN(0a, 8a)
    li    a1,  PATTERN(0b, 8b)
    li    a2,  PATTERN(0c, 8c)
    li    a3,  PATTERN(0d, 8d)
    li    a4,  PATTERN(0e, 8e)
    li    a5,  PATTERN(0f, 8f)
    li    a6,  PATTERN(10, 90)
    li    a7,  PATTERN(11, 91)
    li    s2,  PATTERN(12, 92)
    li    s3,  PATTERN(13, 93)
    li    s4,  PATTERN(14, 94)
    li    s5,  PATTERN(15, 95)
    li    s6,  PATTERN(16, 96)
    li    s7,  PATTERN(17, 97)
    li    s8,  PATTERN(18, 98)
    li    s9,  PATTERN(19, 99)
    li    s10, PATTERN(1a, 9a)
    li    s11, PATTERN(1b, 9b)
    li    t3,  PATTERN(1c, 9c)
    li    t4,  PATTERN(1d, 9d)
    li    t5,  PATTERN(1e, 9e)
    li    t6, SPIN
1:  addi  t6, t6, -1
    bnez  t6, 1b

# t6 holds each expected value in turn; once sp is checked, it holds the
# number of the register being checked, for the exit status.
    li    t6, PATTERN(02, 82)
    bne   sp, t6, bad_sp
    li    sp, 1
    li    t6, PATTERN(01, 81)
    bne   ra, t6, bad
    li    sp, 3
    li    t6, PATTERN(03, 83)
    bne   gp, t6, bad
    li    sp, 4
    li    t6, PATTERN(04, 84)
    bne   tp, t6, bad
    li    sp, 5
    li    t6, PATTERN(05, 85)
    bne   t0, t6, bad
    li    sp, 6
    li    t6, PATTERN(06, 86)
    bne   t1, t6, bad
    li    sp, 7
    li    t6, PATTERN(07, 87)
    bne   t2, t6, bad
    li    sp, 8
    li    t6, PATTERN(08, 88)
    bne   s0, t6, bad
    li    sp, 9
    li    t6, PATTERN(09, 89)
    bne   s1, t6, bad
    li    sp, 10
    li    t6, PATTERN(0a, 8a)
    bne   a0, t6, bad
    li    sp, 11
    li    t6, PATTERN(0b, 8b)
    bne   a1, t6, bad
    li    sp, 12
    li    t6, PATTERN(0c, 8c)
    bne   a2, t6, bad
    li    sp, 13
    li    t6, PATTERN(0d, 8d)
    bne   a3, t6, bad
    li    sp, 14
    li    t6, PATTERN(0e, 8e)
    bne   a4, t6, bad
    li    sp, 15
    li    t6, PATTERN(0f, 8f)
    bne   a5, t6, bad
    li    sp, 16
    li    t6, PATTERN(10, 90)
    bne   a6, t6, bad
    li    sp, 17
    li    t6, PATTERN(11, 91)
    bne   a7, t6, bad
    li    sp, 18
    li    t6, PATTERN(12, 92)
    bne   s2, t6, bad
    li    sp, 19
    li    t6, PATTERN(13, 93)
    bne   s3, t6, bad
    li    sp, 20
    li    t6, PATTERN(14, 94)
    bne   s4, t6, bad
    li    sp, 21
    li    t6, PATTERN(15, 95)
    bne   s5, t6, bad
    li    sp, 22
    li    t6, PATTERN(16, 96)
    bne   s6, t6, bad
    li    sp, 23
    li    t6, PATTERN(17, 97)
    bne   s7, t6, bad
    li    sp, 24
    li    t6, PATTERN(18, 98)
    bne   s8, t6, bad
    li    sp, 25
    li    t6, PATTERN(19, 99)
    bne   s9, t6, bad
    li    sp, 26
    li    t6, PATTERN(1a, 9a)
    bne   s10, t6, bad
    li    sp, 27
    li    t6, PATTERN(1b, 9b)
    bne   s11, t6, bad
    li    sp, 28
    li    t6, PATTERN(1c, 9c)
    bne   t3, t6, bad
    li    sp, 29
    li    t6, PATTERN(1d, 9d)
    bne   t4, t6, bad
    li    sp, 30
    li    t6, PATTERN(1e, 9e)
    bne   t5, t6, bad
    li    a0, 0
    call  exit

bad_sp:
    li    sp, 2
bad:
    mv    a0, sp
    call  exit
