# A higher-half program for the scripts in this directory: linked to run in
# the top 2 GiB of the address space (0xffffffff80000000, where a kernel
# runs once paging maps it there, Linux's KERNEL_LINK_ADDR) and loaded in
# RAM at 0x80000000 by AT, 0xffffffff00000000 lower. Grail loads it there and
# starts it there, with paging off, so the code runs at its load address:
# every address it uses is pc-relative (-mcmodel=medany). What it checks:
#
# - it runs 0xffffffff00000000 below the address it is linked at: the
#   difference between its pc (lla _start) and _start's address, which
#   `.dword _start` (R_RISCV_64) holds, and `lui %hi` / `addi %lo` computes
#   (in the window of R_RISCV_HI20 at the top: -2 GiB is a 32-bit value);
# - `.word _start` (R_RISCV_32) of that address fits, sign-extended by lw;
# - .data and .rodata hold their values, and .bss, which _start zeroes,
#   is zero.
#
# Prints "OK\n" and halts with 0, or halts with 1. Built by
# `cargo xtask ld-fixtures`.
    .equ UART, 0x10000000

    .section .text.start, "ax"
    .globl _start
_start:
    lla   t0, __bss_start
    lla   t1, __bss_end
1:  bgeu  t0, t1, 2f
    sb    zero, 0(t0)
    addi  t0, t0, 1
    j     1b
2:  call  check
    bnez  a0, fail
    li    t0, UART
    lla   t1, message
3:  lbu   t2, 0(t1)
    beqz  t2, 4f
    sb    t2, 0(t0)
    addi  t1, t1, 1
    j     3b
4:  xcal.halt zero
fail:
    li    t0, 1
    xcal.halt t0

    .text
    .globl check
# check() -> a0 = 0 if everything is where it should be.
check:
    lla   t0, _start            # where it runs
    lla   t1, start_address
    ld    t1, 0(t1)             # where it is linked to run
    sub   t2, t1, t0
    li    t3, 0xffffffff00000000
    bne   t2, t3, bad
    lui   t2, %hi(_start)
    addi  t2, t2, %lo(_start)
    bne   t2, t1, bad
    lla   t2, start_word
    lw    t2, 0(t2)
    bne   t2, t1, bad
    lla   t0, counter
    ld    t1, 0(t0)
    li    t2, 0x1122334455667788
    bne   t1, t2, bad
    lla   t0, scratch
    ld    t1, 0(t0)
    bnez  t1, bad
    ld    t1, 8(t0)
    bnez  t1, bad
    lla   t0, message
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
    .p2align 3
    .globl counter
counter:
    .dword 0x1122334455667788
start_address:
    .dword _start
start_word:
    .word _start

    .bss
    .globl scratch
    .p2align 4
scratch:
    .zero 16
