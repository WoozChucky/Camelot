# The RV64 twin of ../../relocs/main.S: every relocation type merlin-ld
# applies, in RV64 code, linked with lib.o and relax.o under link.ld: calls,
# pc-relative loads and stores (ld, sd, lwu) and data words in both
# directions between objects, the exact edges of the BRANCH and JAL ranges
# and of the +-2 GiB window of lui, auipc and call, the %hi/%lo rounding
# edge, positive and negative addends, R_RISCV_64 (.dword) and R_RISCV_32
# (.word) data, an undefined weak symbol and a weak reference that lib.o
# defines.
#
# RV32 code reaches every address with lui or auipc (the sums wrap at
# 4 GiB). RV64 code reaches -2^31 - 0x800 <= v < 2^31 - 0x800: lui only
# absolute addresses near 0, auipc and call only what is within 2 GiB of
# them. So RAM at 0x80000000 is reached pc-relatively (as -mcmodel=medany
# code does), lui only names link.ld's absolute symbols, and the other
# 64-bit addresses come from .dword (R_RISCV_64).
#
# The program checks its own relocations: each `check` puts its number in a7
# and jumps to `fail` (which halts with that number) on a mismatch. When all
# pass it prints "OK\n" and halts with 0. merlin-ld's tests also compare the
# linked bytes with ld.lld's link of the same objects.
# Built by `cargo xtask ld-fixtures` (with -mno-relax).

# a != b: halt with code n. The bne goes to a global, so the assembler
# leaves it to the linker; without `.option exact` it inverts the branch
# around a `j fail` (R_RISCV_JAL), with it, it is an R_RISCV_BRANCH.
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

    # PCREL_HI20 + PCREL_LO12_I to data in another object: `la`, `ld sym`
    # and `lwu sym`.
    la    a0, lib_value
    ld    a0, 0(a0)
    ld    a1, lib_value
    li    t0, 0x123456789abcdef0
    check a0, t0, 5
    check a1, t0, 6
    lwu   a2, lib_word              # 0x89abcdef zero-extended (lw would sign-extend it)
    li    t0, 0x89abcdef
    check a2, t0, 7
    mv    a0, a1
    call  lib_check                 # lib.o branches back here (to `fail`) on a mismatch

    # PCREL_LO12_S, via the `sd rs, sym, rt` pseudo, into this object's
    # data and lib.o's, and an explicit pair whose store is 12 bytes after
    # its auipc.
    li    t1, 5
    sd    t1, counter, t2
1:  auipc t0, %pcrel_hi(counter)
    ld    t1, %pcrel_lo(1b)(t0)
    addi  t1, t1, 1
    sd    t1, %pcrel_lo(1b)(t0)
    ld    t1, counter
    li    t0, 6
    check t1, t0, 8
    li    t1, 0x0fedcba987654321
    sd    t1, lib_scratch, t2
    ld    t0, lib_scratch
    check t0, t1, 9

    # HI20 + LO12_I of absolute symbols (link.ld). `hi_round` sits at
    # 0x...800, so %hi rounds up and %lo is -0x800; 4 below it does not round.
    lui   a0, %hi(hi_round)
    addi  a0, a0, %lo(hi_round)
    li    t0, 0x12345800
    check a0, t0, 10
    lui   a0, %hi(hi_round - 4)     # the addend moves it back across the edge
    addi  a0, a0, %lo(hi_round - 4)
    li    t0, 0x123457fc
    check a0, t0, 11
    # The ends of the window: the largest value lui and addi make, and the
    # smallest (lui 0x80000 is -2^31 once sign-extended, then -0x800).
    lui   a0, %hi(hi_max)
    addi  a0, a0, %lo(hi_max)
    li    t0, 0x7ffff7ff
    check a0, t0, 12
    lui   a0, %hi(hi_min)
    addi  a0, a0, %lo(hi_min)
    li    t0, 0xffffffff7ffff800
    check a0, t0, 13
    lui   a0, %hi(kernel_base)      # lui sign-extends: a higher-half address
    addi  a0, a0, %lo(kernel_base)
    li    t0, 0xffffffff80000000
    check a0, t0, 14

    # PCREL_HI20 at the ends of the window: pc_max is 2^31 - 0x801 bytes
    # after its auipc, pc_min 2^31 + 0x800 before it (link.ld). Only the
    # addresses are compared; there is nothing there.
    .globl pc_max_site, pc_min_site
pc_max_site:
    la    a0, pc_max
    la    t0, pc_max_site
    li    t1, 0x7ffff7ff
    add   t0, t0, t1
    check a0, t0, 15
pc_min_site:
    la    a0, pc_min
    la    t0, pc_min_site
    li    t1, -0x80000800
    add   t0, t0, t1
    check a0, t0, 16

    # CALL_PLT at the ends of the window, never made: the program reads
    # the instructions instead (auipc ra, 0x7ffff; jalr ra, 0x7ff(ra) and
    # auipc ra, 0x80000; jalr ra, -0x800(ra)).
    j     3f
    .globl call_max_site, call_min_site
call_max_site:
    call  call_max
call_min_site:
    call  call_min
3:  la    s1, call_max_site
    lwu   t0, 0(s1)
    li    t1, 0x7ffff097
    check t0, t1, 17
    lwu   t0, 4(s1)
    li    t1, 0x7ff080e7
    check t0, t1, 18
    lwu   t0, 8(s1)
    li    t1, 0x80000097
    check t0, t1, 19
    lwu   t0, 12(s1)
    li    t1, 0x800080e7
    check t0, t1, 20

    # R_RISCV_32: a `.word` of an address below 4 GiB (read back with lwu)
    # and of a higher-half one, negative as a 64-bit value, which lw
    # sign-extends back (a -mcmodel=medlow jump table in a higher-half
    # kernel is that).
    la    s1, table32
    lwu   t0, 0(s1)                 # .word lib_double
    la    t1, lib_double
    check t0, t1, 21
    lw    t0, 4(s1)                 # .word kernel_base + 0x10
    li    t1, 0xffffffff80000010
    check t0, t1, 22
    lw    t0, 8(s1)                 # .word missing
    check t0, zero, 23
    lw    t0, 12(s1)                # .word missing + 16
    li    t1, 16
    check t0, t1, 24
    lwu   t0, 16(s1)                # .word __stack_top
    check t0, sp, 25

    # R_RISCV_64: whole addresses, any of the 2^64.
    la    s1, table64
    ld    t0, 0(s1)                 # .dword lib_double
    li    a0, 50
    jalr  t0
    li    t0, 100
    check a0, t0, 26
    ld    t0, lib_ptr               # lib.o's .dword main_helper
    li    a0, 1
    jalr  t0
    li    t0, 101
    check a0, t0, 27
    ld    t0, 8(s1)                 # .dword far64 + 0x10
    li    t1, 0x123456789abcdf00
    check t0, t1, 28
    ld    t0, 16(s1)                # .dword kernel_base - 8
    li    t1, 0xffffffff7ffffff8
    check t0, t1, 29
    ld    t0, 24(s1)                # .dword top64 + 0x20: wraps past 2^64
    li    t1, 0x10
    check t0, t1, 30
    ld    t0, 32(s1)                # .dword missing
    check t0, zero, 31
    ld    t0, 40(s1)                # .dword missing - 1
    li    t1, -1
    check t0, t1, 32
    ld    t0, 48(s1)                # .dword __stack_top
    check t0, sp, 33

    # An undefined weak symbol is 0 as data and as an address. On RV64 an
    # auipc reaches 0 only from below 2^31 + 0x800, so the `la` is in .low
    # (low_weak, at 0x80000010). A call or jump to it would never return:
    # as in ld.lld, it targets itself.
    call  low_weak
    check a0, zero, 34
    lui   a0, %hi(missing)
    addi  a0, a0, %lo(missing)
    check a0, zero, 35
    beqz  zero, 2f
    call  missing
    jal   missing
    .option push
    .option exact
    beq   a0, a1, missing
    .option pop
2:

    # A weak reference that another object defines is that definition, not
    # an undefined weak: the call, the jump and the branch reach lib_inc.
    .weak lib_inc
    li    a0, 10
    call  lib_inc                   # CALL_PLT
    jal   lib_inc                   # JAL
    auipc ra, 0                     # ra = the instruction after the beq
    addi  ra, ra, 12
    .option push
    .option exact
    beq   zero, zero, lib_inc       # BRANCH
    .option pop
    li    t0, 13
    check_exact a0, t0, 36

    # The edges of the BRANCH range (in .text.branch), and code built with
    # relaxation enabled (relax.o).
    li    a0, 0
    call  branch_edges
    li    t0, 2
    check a0, t0, 37
    li    a0, 1
    call  branch_edges
    li    t0, 3
    check a0, t0, 38
    li    a0, 4
    call  relax_fn
    li    t0, 0x123456789abcdef8    # 4 * 2 + lib_value
    check a0, t0, 39
    ld    t1, relax_word
    check t1, t0, 40
    li    t0, 0x7ffff7ff            # relax_fn's lui/addi of hi_max
    check a1, t0, 41

    # "OK\n" and halt with 0: the UART through lui %hi and sb %lo (LO12_S).
    lui   t0, %hi(uart)
    li    t1, 'O'
    sb    t1, %lo(uart)(t0)
    li    t1, 'K'
    sb    t1, %lo(uart)(t0)
    li    t1, '\n'
    sb    t1, %lo(uart)(t0)
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

# The far ends of the J-type range, placed by link.ld, and the one `la` of
# the undefined weak symbol, within 0x800 bytes of 0x80000000.
    .section .low, "awx"
    .globl low_target, low_weak
    ret
    ret
low_target:                         # 0x80000008
    addi  s0, s0, 40
    ret
low_weak:                           # 0x80000010: a0 = &missing, 0
    la    a0, missing
    ret

    .section .high, "ax"
    .globl high_target
high_target:                        # 0x80200000
    addi  s0, s0, 5
    ret

    .section .rodata
    .weak missing
    .balign 8
table64:
    .dword lib_double
    .dword far64 + 0x10
    .dword kernel_base - 8
    .dword top64 + 0x20
    .dword missing
    .dword missing - 1
    .dword __stack_top
table32:
    .word lib_double
    .word kernel_base + 0x10
    .word missing
    .word missing + 16
    .word __stack_top

    .data
    .balign 8
counter:
    .dword 0
