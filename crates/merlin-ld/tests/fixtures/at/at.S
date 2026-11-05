# A program for the AT(lma) scripts in this directory, which load sections
# somewhere else than where they run. Grail loads each segment at its load
# address (p_paddr) and starts at the entry point's, with paging off, so
# the program runs where it is loaded: every address it uses is
# pc-relative. Like an embedded program in ROM, _start copies .data from
# where it is loaded (__data_load) to where it runs (__data_start), and
# zeroes .bss, which the loader zeroes at its load address; then check()
# (in .text.more) reads all three. Prints "OK\n" and halts with 0, or halts
# with 1. Built by `cargo xtask ld-fixtures`.
    .equ UART, 0x10000000

    .section .text.start, "ax"
    .globl _start
_start:
    la    t0, __data_load
    la    t1, __data_start
    la    t2, __data_end
1:  bgeu  t1, t2, 2f
    lbu   t3, 0(t0)
    sb    t3, 0(t1)
    addi  t0, t0, 1
    addi  t1, t1, 1
    j     1b
2:  la    t1, __bss_start
    la    t2, __bss_end
3:  bgeu  t1, t2, 4f
    sb    zero, 0(t1)
    addi  t1, t1, 1
    j     3b
4:  call  check
    bnez  a0, fail
    li    t0, UART
    la    t1, message
5:  lbu   t2, 0(t1)
    beqz  t2, 6f
    sb    t2, 0(t0)
    addi  t1, t1, 1
    j     5b
6:  xcal.halt zero
fail:
    li    t0, 1
    xcal.halt t0

    .section .text.more, "ax"
    .globl check
# check() -> a0 = 0 if .data, .bss and .rodata hold what they should.
check:
    la    t0, counter
    lw    t1, 0(t0)
    li    t2, 0x12345678
    bne   t1, t2, bad
    lwu   t1, 4(t0)          # zero-extended, as li makes 0x9abcdef0
    li    t2, 0x9abcdef0
    bne   t1, t2, bad
    la    t0, scratch
    lw    t1, 0(t0)
    bnez  t1, bad
    lw    t1, 12(t0)
    bnez  t1, bad
    la    t0, message
    lbu   t1, 0(t0)
    li    t2, 'O'
    bne   t1, t2, bad
    li    a0, 0
    ret
bad:
    li    a0, 1
    ret

    .section .rodata
    .globl message
message:
    .asciz "OK\n"

    .data
    .globl counter
    .p2align 3
counter:
    .word 0x12345678, 0x9abcdef0

    .bss
    .globl scratch
    .p2align 4
scratch:
    .zero 16
