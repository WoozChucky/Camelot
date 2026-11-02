# regs: checks that being interrupted and switched out leaves every register
# as it was. Sets each register xN (but t6) to N in every byte, spins for
# about three timer ticks with t6 as the counter, then compares. Exits with
# status 0 if all of them still hold their value, else with the number of
# the first one that changed. Run it beside other programs (`regs & ding &
# dong`) so that their values pass through the same registers.

    .equ SPIN, 1500000

    .text
    .globl main
main:
    li    ra, 0x01010101
    li    sp, 0x02020202        # nothing here uses the stack
    li    gp, 0x03030303
    li    tp, 0x04040404
    li    t0, 0x05050505
    li    t1, 0x06060606
    li    t2, 0x07070707
    li    s0, 0x08080808
    li    s1, 0x09090909
    li    a0, 0x0a0a0a0a
    li    a1, 0x0b0b0b0b
    li    a2, 0x0c0c0c0c
    li    a3, 0x0d0d0d0d
    li    a4, 0x0e0e0e0e
    li    a5, 0x0f0f0f0f
    li    a6, 0x10101010
    li    a7, 0x11111111
    li    s2, 0x12121212
    li    s3, 0x13131313
    li    s4, 0x14141414
    li    s5, 0x15151515
    li    s6, 0x16161616
    li    s7, 0x17171717
    li    s8, 0x18181818
    li    s9, 0x19191919
    li    s10, 0x1a1a1a1a
    li    s11, 0x1b1b1b1b
    li    t3, 0x1c1c1c1c
    li    t4, 0x1d1d1d1d
    li    t5, 0x1e1e1e1e
    li    t6, SPIN
1:  addi  t6, t6, -1
    bnez  t6, 1b

# t6 holds each expected value in turn; once sp is checked, it holds the
# number of the register being checked, for the exit status.
    li    t6, 0x02020202
    bne   sp, t6, bad_sp
    li    sp, 1
    li    t6, 0x01010101
    bne   ra, t6, bad
    li    sp, 3
    li    t6, 0x03030303
    bne   gp, t6, bad
    li    sp, 4
    li    t6, 0x04040404
    bne   tp, t6, bad
    li    sp, 5
    li    t6, 0x05050505
    bne   t0, t6, bad
    li    sp, 6
    li    t6, 0x06060606
    bne   t1, t6, bad
    li    sp, 7
    li    t6, 0x07070707
    bne   t2, t6, bad
    li    sp, 8
    li    t6, 0x08080808
    bne   s0, t6, bad
    li    sp, 9
    li    t6, 0x09090909
    bne   s1, t6, bad
    li    sp, 10
    li    t6, 0x0a0a0a0a
    bne   a0, t6, bad
    li    sp, 11
    li    t6, 0x0b0b0b0b
    bne   a1, t6, bad
    li    sp, 12
    li    t6, 0x0c0c0c0c
    bne   a2, t6, bad
    li    sp, 13
    li    t6, 0x0d0d0d0d
    bne   a3, t6, bad
    li    sp, 14
    li    t6, 0x0e0e0e0e
    bne   a4, t6, bad
    li    sp, 15
    li    t6, 0x0f0f0f0f
    bne   a5, t6, bad
    li    sp, 16
    li    t6, 0x10101010
    bne   a6, t6, bad
    li    sp, 17
    li    t6, 0x11111111
    bne   a7, t6, bad
    li    sp, 18
    li    t6, 0x12121212
    bne   s2, t6, bad
    li    sp, 19
    li    t6, 0x13131313
    bne   s3, t6, bad
    li    sp, 20
    li    t6, 0x14141414
    bne   s4, t6, bad
    li    sp, 21
    li    t6, 0x15151515
    bne   s5, t6, bad
    li    sp, 22
    li    t6, 0x16161616
    bne   s6, t6, bad
    li    sp, 23
    li    t6, 0x17171717
    bne   s7, t6, bad
    li    sp, 24
    li    t6, 0x18181818
    bne   s8, t6, bad
    li    sp, 25
    li    t6, 0x19191919
    bne   s9, t6, bad
    li    sp, 26
    li    t6, 0x1a1a1a1a
    bne   s10, t6, bad
    li    sp, 27
    li    t6, 0x1b1b1b1b
    bne   s11, t6, bad
    li    sp, 28
    li    t6, 0x1c1c1c1c
    bne   t3, t6, bad
    li    sp, 29
    li    t6, 0x1d1d1d1d
    bne   t4, t6, bad
    li    sp, 30
    li    t6, 0x1e1e1e1e
    bne   t5, t6, bad
    li    a0, 0
    call  exit

bad_sp:
    li    sp, 2
bad:
    mv    a0, sp
    call  exit
