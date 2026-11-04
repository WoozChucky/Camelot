# M extension self-check for Grail: every mul/div/rem variant, including the
# corner cases the RISC-V spec defines instead of trapping (division by zero,
# INT_MIN / -1). Prints the number of cases with xcal.dbg and exits with 0
# when every result is right, or with the number of the first wrong case.
#
# muldiv-rv64.S checks the M extension of RV64 machines.
#
# Built with our own tools by `cargo xtask examples muldiv`, which does the same as
#
#   merlin-as examples/muldiv/muldiv.S -o muldiv.o
#   merlin-ld muldiv.o -o examples/muldiv/muldiv.elf
#   grail examples/muldiv/muldiv.elf
#
# (merlin-ld's default layout: no linker script.) Every label is local and in
# .text, so merlin-as resolves all of them and the object has no relocations.
# Reference build with LLVM, only for comparison:
#
#   clang --target=riscv32 -march=rv32im -mabi=ilp32 -mno-relax -nostdlib -static \
#         -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -T examples/hello/link.ld examples/muldiv/muldiv.S -o muldiv.elf

    .equ INT_MIN, 0x80000000

    .text
    .globl _start
_start:
    li    s0, 0                 # number of the case being checked

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
