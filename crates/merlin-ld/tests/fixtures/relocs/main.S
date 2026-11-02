# Every relocation type merlin-ld applies, linked with lib.o and relax.o
# under link.ld: calls and data references in both directions between
# objects, the exact edges of the BRANCH and JAL ranges, the %hi/%lo rounding
# edge, positive and negative addends, an undefined weak symbol.
#
# The program checks its own relocations: each `check` puts its number in a7
# and jumps to `fail` (which halts with that number) on a mismatch. When all
# pass it prints "OK\n" and halts with 0. merlin-ld's tests also compare the
# linked bytes with ld.lld's link of the same objects.
# Built by `cargo xtask ld-fixtures` (with -mno-relax).

    .equ UART, 0x10000000

# a != b: halt with code n. The bne goes to a global in another section, so
# the assembler leaves it to the linker; without `.option exact` it inverts
# the branch around a `j fail` (R_RISCV_JAL), with it, it is an R_RISCV_BRANCH.
.macro check a, b, n
    li    a7, \n
    bne   \a, \b, fail
.endm
.macro check_exact a, b, n
    li    a7, \n
    .option push
    .option exact
    bne   \a, \b, fail
    .option pop
.endm

    .section .text.start, "ax"
    .globl _start
_start:                             # link.ld puts this at 0x80100000
    li    s0, 0
    jal   high_target               # 0x80100004 -> 0x80200000: +0xffffc, the largest J offset a
                                    # 4-byte aligned target can have
    jal   low_target                # 0x80100008 -> 0x80000008: -0x100000, the smallest
    li    t0, 45
    check s0, t0, 1

    la    sp, __stack_top           # PCREL_HI20 + PCREL_LO12_I against a script symbol

    # R_RISCV_CALL_PLT (what clang emits for `call`), into another object.
    li    a0, 21
    call  lib_double
    li    t0, 42
    check_exact a0, t0, 2

    # R_RISCV_CALL, the older name: clang only emits it by request.
    li    a0, 7
    .reloc ., R_RISCV_CALL, lib_double
    auipc ra, 0
    jalr  ra, 0(ra)
    li    t0, 14
    check a0, t0, 3

    # lib.o calls back into this object (backward CALL_PLT and JAL) and
    # tail-calls main_tail: (3 + 100 + 100) + 1000.
    call  lib_calls_back
    li    t0, 1203
    check_exact a0, t0, 4

    # PCREL_HI20 + PCREL_LO12_I to data in another object: `la` and `lw sym`.
    la    a0, lib_value
    lw    a0, 0(a0)
    lw    a1, lib_value
    li    t0, 0x12345678
    check a0, t0, 5
    check a1, t0, 6
    call  lib_check                 # lib.o branches back here (to `fail`) on a mismatch

    # PCREL_LO12_S, via the `sw rs, sym, rt` pseudo and an explicit pair
    # whose store is 12 bytes after its auipc.
    li    t1, 5
    sw    t1, counter, t2
1:  auipc t0, %pcrel_hi(counter)
    lw    t1, %pcrel_lo(1b)(t0)
    addi  t1, t1, 1
    sw    t1, %pcrel_lo(1b)(t0)
    lw    t1, counter
    li    t0, 6
    check t1, t0, 7

    # HI20 / LO12 at the rounding edge: `edge` sits at 0x...800, so %hi
    # rounds up and %lo is -0x800; `below_edge` at 0x...7fc does not round.
    lui   a0, %hi(edge)
    lw    a1, %lo(edge)(a0)
    li    t0, 0x22222222
    check a1, t0, 8
    lui   a0, %hi(below_edge)
    lw    a1, %lo(below_edge)(a0)
    li    t0, 0x11111111
    check a1, t0, 9
    lui   a2, %hi(edge_copy)
    sw    a1, %lo(edge_copy)(a2)    # LO12_S
    lw    a3, edge_copy
    check a3, t0, 10
    lui   a0, %hi(below_edge + 4)   # the addend moves it across the edge
    addi  a0, a0, %lo(below_edge + 4)
    la    a1, edge
    check_exact a0, a1, 11
    lui   a0, %hi(edge_copy - 8)    # negative addend, back across it
    addi  a0, a0, %lo(edge_copy - 8)
    la    a1, below_edge
    check a0, a1, 12

    # R_RISCV_32: a table of addresses.
    la    s1, table
    lw    t0, 4(s1)                 # .word edge
    la    t1, edge
    check t0, t1, 13
    lw    t0, 8(s1)                 # .word below_edge + 4
    check t0, t1, 14
    lw    t0, 12(s1)                # .word edge_copy - 4
    check t0, t1, 15
    lw    t0, 16(s1)                # .word missing
    check t0, zero, 16
    lw    t0, 20(s1)                # .word missing + 16
    li    t1, 16
    check t0, t1, 17
    lw    t0, 24(s1)                # .word __stack_top
    check t0, sp, 18
    lw    t0, 0(s1)                 # .word lib_double
    li    a0, 50
    jalr  t0
    li    t0, 100
    check a0, t0, 19
    lw    t0, lib_ptr               # lib.o's .word main_helper
    li    a0, 1
    jalr  t0
    li    t0, 101
    check a0, t0, 20

    # An undefined weak symbol is 0 as data and as an address. A call or
    # jump to it would never return: as in ld.lld, it targets itself.
    la    a0, missing
    check a0, zero, 21
    lui   a0, %hi(missing)
    addi  a0, a0, %lo(missing)
    check a0, zero, 22
    beqz  zero, 2f
    call  missing
    jal   missing
    .option push
    .option exact
    beq   a0, a1, missing
    .option pop
2:

    # The edges of the BRANCH range (in .text.branch), and code built with
    # relaxation enabled (relax.o).
    li    a0, 0
    call  branch_edges
    li    t0, 2
    check a0, t0, 23
    li    a0, 1
    call  branch_edges
    li    t0, 3
    check a0, t0, 24
    li    a0, 4
    call  relax_fn
    li    t0, 0x12345680            # 4 * 2 + 0x12345678
    check a0, t0, 25
    lw    t1, relax_word
    check t1, t0, 26

    # "OK\n" and halt with 0.
    li    t0, UART
    li    t1, 'O'
    sb    t1, 0(t0)
    li    t1, 'K'
    sb    t1, 0(t0)
    li    t1, '\n'
    sb    t1, 0(t0)
    .insn i 0x0b, 1, x0, zero, 0    # xcal.halt zero

    .globl fail
fail:
    .insn i 0x0b, 1, x0, a7, 0      # xcal.halt a7

    .text
    .globl main_helper, main_tail
main_helper:                        # a0 + 100
    addi  a0, a0, 100
    ret
main_tail:                          # a0 + 1000
    addi  a0, a0, 1000
    ret

# The B-type range is -4096..=4094. branch_edges(0) takes a branch to exactly
# -4096 bytes and returns 2; branch_edges(1) one to +4092 (the largest offset
# to a 4-byte aligned target) and returns 3. The targets are global, so the
# assembler leaves the branches to the linker.
    .section .text.branch, "ax"
    .globl branch_back, branch_edges, branch_fwd
branch_back:
    li    a0, 2
    ret
    .org  4096
branch_edges:
    .option push
    .option exact
    beq   a0, zero, branch_back     # .text.branch+4096 -> +0: -4096
    bne   a0, zero, branch_fwd      # .text.branch+4100 -> +8192: +4092
    .option pop
    li    a0, 99
    ret
    .org  4100 + 4092
branch_fwd:
    li    a0, 3
    ret

# The far ends of the J-type range, placed by link.ld.
    .section .low, "awx"
    .globl low_target
    ret
    ret
low_target:                         # 0x80000008
    addi  s0, s0, 40
    ret

    .section .high, "ax"
    .globl high_target
high_target:                        # 0x80200000
    addi  s0, s0, 5
    ret

    .section .rodata
    .weak missing
    .balign 4
table:
    .word lib_double
    .word edge
    .word below_edge + 4
    .word edge_copy - 4
    .word missing
    .word missing + 16
    .word __stack_top

# 4 KiB aligned, so the low 12 bits of `edge` are exactly 0x800.
    .data
    .balign 4096
counter:
    .word 0
    .org  0x7fc
below_edge:
    .word 0x11111111
edge:
    .word 0x22222222
edge_copy:
    .word 0
