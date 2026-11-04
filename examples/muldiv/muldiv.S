# M extension self-check for Grail: every mul/div/rem variant, including the
# corner cases the RISC-V spec defines instead of trapping (division by zero,
# INT_MIN / -1). Prints the number of cases with xcal.dbg and exits with 0
# when every result is right, or with the number of the first wrong case.
#
# The RV64 build (muldiv-rv64.elf) has cases of its own: every variant at 64
# bits, mulh, mulhu and mulhsu returning the high 64 bits of the 128-bit
# product, then RV64's W instructions (mulw, divw, divuw, remw, remuw), which
# work on the low 32 bits and sign-extend their 32-bit result, including the
# corner cases (division by zero, INT64_MIN / -1 and INT32_MIN / -1).
#
# Built with our own tools by `cargo xtask examples muldiv`, which does the same as
#
#   merlin -E -I include examples/muldiv/muldiv.S -o muldiv.s
#   merlin-as muldiv.s -o muldiv.o
#   merlin-ld muldiv.o -o examples/muldiv/muldiv.elf
#   grail examples/muldiv/muldiv.elf
#
# and, for RV64, the same as
#
#   merlin -E -march=rv64ima_zicsr_zifencei -I include examples/muldiv/muldiv.S -o muldiv.s
#   merlin-as -march=rv64ima_zicsr_zifencei muldiv.s -o muldiv.o
#   merlin-ld muldiv.o -o examples/muldiv/muldiv-rv64.elf
#   grail examples/muldiv/muldiv-rv64.elf
#
# (merlin-ld's default layout: no linker script.) Every label is local and in
# .text, so merlin-as resolves all of them and the object has no relocations.
# Reference build with LLVM, only for comparison:
#
#   clang --target=riscv32 -march=rv32im -mabi=ilp32 -mno-relax -nostdlib -static \
#         -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -T examples/hello/link.ld examples/muldiv/muldiv.S -o muldiv.elf
#
# and for RV64:
#
#   clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mcmodel=medany -mno-relax \
#         -nostdlib -static -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -T examples/hello/link.ld examples/muldiv/muldiv.S -o muldiv-rv64.elf

#if __riscv_xlen == 64
    .equ INT64_MIN, 0x8000000000000000
    .equ INT32_MIN, -0x80000000         # sign-extended, as the W instructions leave it
#else
    .equ INT_MIN, 0x80000000
#endif

    .text
    .globl _start
_start:
    li    s0, 0                 # number of the case being checked

#if __riscv_xlen == 64
    # ----- multiplication: the low and the three kinds of high halves -----
    li    a1, 7
    li    a2, -3
    mul   a0, a1, a2
    li    t0, -21
    call  check                 # 1: 7 * -3 = -21

    li    a1, 0x123456789abcdef0
    li    a2, 0x9abcdef012345678
    mul   a0, a1, a2
    li    t0, 0x1b9ae38e242d2080
    call  check                 # 2: low 64 bits of the product

    mulh  a0, a1, a2
    li    t0, 0xf8cc93d5e8436ffd
    call  check                 # 3: high half, both signed (a2 is negative)

    mulhu a0, a1, a2
    li    t0, 0x0b00ea4e83004eed
    call  check                 # 4: high half, both unsigned

    li    a1, -2
    mulhsu a0, a1, a2
    li    t0, -2
    call  check                 # 5: high half, signed -2 times unsigned 0x9abcdef012345678

    mulhu a0, a1, a2
    li    t0, 0x9abcdef012345676
    call  check                 # 6: the same bits, both unsigned

    mulh  a0, a1, a2
    li    t0, 0
    call  check                 # 7: the same bits, both signed: small and positive

    # ----- mulw: the low 32 bits of the product, sign-extended -----
    li    a1, 0x0000000112345678 # the upper halves do not count
    li    a2, 0xffffffff9abcdef0
    mulw  a0, a1, a2
    li    t0, 0x242d2080
    call  check                 # 8

    li    a1, 0x10000
    li    a2, 0x8000
    mulw  a0, a1, a2
    li    t0, INT32_MIN
    call  check                 # 9: 0x8000_0000 has bit 31 set: negative

    # ----- division rounds toward zero; the remainder takes the dividend's sign -----
    li    a1, -21
    li    a2, 4
    div   a0, a1, a2
    li    t0, -5
    call  check                 # 10: -21 / 4 = -5

    rem   a0, a1, a2
    li    t0, -1
    call  check                 # 11: -21 % 4 = -1

    li    a1, -1
    li    a2, 2
    divu  a0, a1, a2
    li    t0, 0x7fffffffffffffff
    call  check                 # 12: 0xffff_ffff_ffff_ffff / 2

    li    a2, 10
    remu  a0, a1, a2
    li    t0, 5
    call  check                 # 13: 18446744073709551615 % 10

    # ----- the W divisions: the low 32 bits, sign-extended results -----
    li    a1, 0x7fffffffffffffeb # low 32 bits: -21
    li    a2, 0x100000004       # low 32 bits: 4
    divw  a0, a1, a2
    li    t0, -5
    call  check                 # 14

    remw  a0, a1, a2
    li    t0, -1
    call  check                 # 15

    li    a1, -1
    li    a2, 2
    divuw a0, a1, a2
    li    t0, 0x7fffffff
    call  check                 # 16: 0xffff_ffff / 2

    li    a2, 10
    remuw a0, a1, a2
    li    t0, 5
    call  check                 # 17: 4294967295 % 10

    # ----- division by zero: quotient all ones, remainder the dividend -----
    li    a1, 1234
    div   a0, a1, zero
    li    t0, -1
    call  check                 # 18

    divu  a0, a1, zero
    li    t0, 0xffffffffffffffff
    call  check                 # 19

    rem   a0, a1, zero
    mv    t0, a1
    call  check                 # 20

    remu  a0, a1, zero
    mv    t0, a1
    call  check                 # 21

    li    a1, 0x1234567887654321 # the W ones take the low 32 bits
    divw  a0, a1, zero
    li    t0, -1
    call  check                 # 22

    divuw a0, a1, zero
    li    t0, -1
    call  check                 # 23: 0xffff_ffff, sign-extended

    remw  a0, a1, zero
    li    t0, 0xffffffff87654321
    call  check                 # 24: the low 32 bits, sign-extended

    remuw a0, a1, zero
    li    t0, 0xffffffff87654321
    call  check                 # 25: the same: remuw sign-extends too

    # ----- signed overflow: MIN / -1 = MIN, remainder 0 -----
    li    a1, INT64_MIN
    li    a2, -1
    div   a0, a1, a2
    li    t0, INT64_MIN
    call  check                 # 26

    rem   a0, a1, a2
    li    t0, 0
    call  check                 # 27

    li    a1, INT32_MIN
    divw  a0, a1, a2
    li    t0, INT32_MIN
    call  check                 # 28

    remw  a0, a1, a2
    li    t0, 0
    call  check                 # 29
#else
    # ----- multiplication: the low and the three kinds of high halves -----
    li    a1, 7
    li    a2, -3
    mul   a0, a1, a2
    li    t0, -21
    call  check                 # 1: 7 * -3 = -21

    li    a1, 0x12345678
    li    a2, 0x9abcdef0
    mul   a0, a1, a2
    li    t0, 0x242d2080
    call  check                 # 2: low 32 bits of the product

    mulh  a0, a1, a2
    li    t0, 0xf8cc93d6
    call  check                 # 3: high half, both signed (a2 is negative)

    mulhu a0, a1, a2
    li    t0, 0x0b00ea4e
    call  check                 # 4: high half, both unsigned

    li    a1, -2
    mulhsu a0, a1, a2
    li    t0, -2
    call  check                 # 5: high half, signed -2 times unsigned 0x9abcdef0

    mulhu a0, a1, a2
    li    t0, 0x9abcdeee
    call  check                 # 6: the same bits, both unsigned

    mulh  a0, a1, a2
    li    t0, 0
    call  check                 # 7: the same bits, both signed: small and positive

    # ----- division rounds toward zero; the remainder takes the dividend's sign -----
    li    a1, -21
    li    a2, 4
    div   a0, a1, a2
    li    t0, -5
    call  check                 # 8: -21 / 4 = -5

    rem   a0, a1, a2
    li    t0, -1
    call  check                 # 9: -21 % 4 = -1

    li    a1, -1
    li    a2, 2
    divu  a0, a1, a2
    li    t0, 0x7fffffff
    call  check                 # 10: 0xffffffff / 2

    li    a2, 10
    remu  a0, a1, a2
    li    t0, 5
    call  check                 # 11: 4294967295 % 10

    # ----- division by zero: quotient all ones, remainder the dividend -----
    li    a1, 1234
    div   a0, a1, zero
    li    t0, -1
    call  check                 # 12

    divu  a0, a1, zero
    li    t0, 0xffffffff
    call  check                 # 13

    rem   a0, a1, zero
    mv    t0, a1
    call  check                 # 14

    remu  a0, a1, zero
    mv    t0, a1
    call  check                 # 15

    # ----- signed overflow: INT_MIN / -1 = INT_MIN, remainder 0 -----
    li    a1, INT_MIN
    li    a2, -1
    div   a0, a1, a2
    li    t0, INT_MIN
    call  check                 # 16

    rem   a0, a1, a2
    li    t0, 0
    call  check                 # 17
#endif

    # All passed: show how many, exit 0.
    mv    a0, s0
    xcal.dbg a0
    li    a0, 0
    xcal.halt a0

# check(a0 = result, t0 = expected): counts the case; on a mismatch, stops
# the machine with the case number as the exit code.
check:
    addi  s0, s0, 1
    bne   a0, t0, 1f
    ret
1:  mv    a0, s0
    xcal.halt a0
