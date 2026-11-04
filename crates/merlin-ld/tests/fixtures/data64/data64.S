# R_RISCV_64 on RV32: `.dword sym + A` is S + A in 64 bits, in a loaded
# section (.data) and in a debug section, which ld.lld fills differently.
#
# * Loaded, the value is S + A computed in 64 bits: `target + 0x7fffffff`
#   carries into the high word.
# * In a section that is not loaded, ld.lld's relocateNonAlloc sign-extends
#   the 32-bit value S + A (`SignExtend64<32>`), so `target` (0x800000xx)
#   gets 0xffffffff above it and the carry is lost.
#
# The program checks the loaded words: on a mismatch it halts with the
# number of the word, otherwise it prints "OK\n" and halts with 0. merlin-ld's
# tests compare the loaded bytes and .debug_info's with ld.lld's link.
# Built by `cargo xtask ld-fixtures` (with -mno-relax).

    .equ UART, 0x10000000

    .text
    .globl _start
_start:                         # link.ld puts this at 0x80000000
    la    s0, words
    la    s1, target
    li    s2, 0x7fffffff

    # .dword target: target, 0
    lw    a0, 0(s0)
    li    a7, 1
    bne   a0, s1, fail
    lw    a0, 4(s0)
    li    a7, 2
    bnez  a0, fail

    # .dword target - 1: target - 1, 0
    lw    a0, 8(s0)
    addi  t0, s1, -1
    li    a7, 3
    bne   a0, t0, fail
    lw    a0, 12(s0)
    li    a7, 4
    bnez  a0, fail

    # .dword target + 0x7fffffff: the low word wraps, the high word is the carry
    lw    a0, 16(s0)
    add   t0, s1, s2
    li    a7, 5
    bne   a0, t0, fail
    lw    a0, 20(s0)
    li    t0, 1
    li    a7, 6
    bne   a0, t0, fail

    # .dword _start + 0x7fffffff: 0xffffffff, no carry
    lw    a0, 24(s0)
    li    t0, -1
    li    a7, 7
    bne   a0, t0, fail
    lw    a0, 28(s0)
    li    a7, 8
    bnez  a0, fail

    li    t0, UART
    li    t1, 79                # 'O'
    sb    t1, 0(t0)
    li    t1, 75                # 'K'
    sb    t1, 0(t0)
    li    t1, 10                # '\n'
    sb    t1, 0(t0)
    xcal.halt zero

fail:
    xcal.halt a7

    .data
    .p2align 3
words:
    .dword target
    .dword target - 1
    .dword target + 0x7fffffff
    .dword _start + 0x7fffffff
    .globl target
target:
    .word 0

# The same words where nothing loads them.
    .section .debug_info, "", @progbits
    .dword target
    .dword target - 1
    .dword target + 0x7fffffff
    .dword _start + 0x7fffffff
