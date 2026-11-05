    .data

    # const char *greeting: 8 bytes
    .globl greeting
    .balign 8
    .type greeting, @object
    .size greeting, 8
greeting:
    .quad .L.str

    # static int .compoundliteral[2]: 8 bytes
    .balign 4
    .type .compoundliteral, @object
    .size .compoundliteral, 8
.compoundliteral:
    .word 1
    .word 1

    # int *ones: 8 bytes
    .globl ones
    .balign 8
    .type ones, @object
    .size ones, 8
ones:
    .quad .compoundliteral

    # const char *farewell: 8 bytes
    .globl farewell
    .balign 8
    .type farewell, @object
    .size farewell, 8
farewell:
    .quad .L.str.1

    # static int .compoundliteral.2[1]: 4 bytes
    .balign 4
    .type .compoundliteral.2, @object
    .size .compoundliteral.2, 4
.compoundliteral.2:
    .word 2

    # int *twos: 8 bytes
    .globl twos
    .balign 8
    .type twos, @object
    .size twos, 8
twos:
    .quad .compoundliteral.2

    # const char *again: 8 bytes
    .globl again
    .balign 8
    .type again, @object
    .size again, 8
again:
    .quad .L.str

    .text

    # int count(void): frame 16 bytes
    .globl count
    .type count, @function
count:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    la    a0, count.n.4
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, 0(sp)
    lw    a0, 0(a0)
    addiw a0, a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sw    a0, 0(a1)
    addiw a0, a0, -1
    la    a0, count.n
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, count.tag
    ld    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 99
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
.L.return.count:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size count, . - count

    .data

    # static int n: 4 bytes
    .balign 4
    .type count.n, @object
    .size count.n, 4
count.n:
    .word 5

    # static const char *tag: 8 bytes
    .balign 8
    .type count.tag, @object
    .size count.tag, 8
count.tag:
    .quad .L.str.3

    # static int n: 4 bytes
    .balign 4
    .type count.n.4, @object
    .size count.n.4, 4
count.n.4:
    .word 7

    .text

    # int other(void): frame 16 bytes
    .globl other
    .type other, @function
other:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    la    a0, other.n.5
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, 0(sp)
    lw    a0, 0(a0)
    addiw a0, a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sw    a0, 0(a1)
    addiw a0, a0, -1
    la    a0, other.n
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, late
    ld    a0, 0(a0)
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
.L.return.other:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size other, . - other

    .data

    # static int n: 4 bytes
    .balign 4
    .type other.n, @object
    .size other.n, 4
other.n:
    .word 1

    .bss

    # static int n: 4 bytes
    .balign 4
    .type other.n.5, @object
    .size other.n.5, 4
other.n.5:
    .zero 4

    .data

    # static struct pair *late: 8 bytes
    .balign 8
    .type late, @object
    .size late, 8
late:
    .quad .compoundliteral.10

    .text

    # int copies(int k): frame 176 bytes
    #     -20(s0)  param int k
    #     -32(s0)  local int small[3]
    #     -48(s0)  local int small[3]
    #     -88(s0)  local int few[10]
    #    -104(s0)  local struct named nm
    #    -112(s0)  local int *five
    #    -136(s0)  temp int[5] (compound literal)
    #    -144(s0)  local int *more
    #    -168(s0)  temp int[5] (compound literal)
    .globl copies
    .type copies, @function
copies:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -160
    sw    a0, -20(s0)
    # int small[3] = ...
    sd    zero, -32(s0)
    sb    zero, -24(s0)
    sb    zero, -23(s0)
    sb    zero, -22(s0)
    sb    zero, -21(s0)
    li    a0, 1
    sw    a0, -32(s0)
    li    a0, 2
    sw    a0, -28(s0)
    li    a0, 3
    sw    a0, -24(s0)
    # int small[3] = ...
    sd    zero, -48(s0)
    sb    zero, -40(s0)
    sb    zero, -39(s0)
    sb    zero, -38(s0)
    sb    zero, -37(s0)
    li    a0, 4
    sw    a0, -48(s0)
    li    a0, 5
    sw    a0, -44(s0)
    li    a0, 6
    sw    a0, -40(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -48
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 4
    ld    a1, 0(sp)
    addi  sp, sp, 8
    subw  a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    # int few[10] = ...
    sd    zero, -88(s0)
    sd    zero, -80(s0)
    sd    zero, -72(s0)
    sd    zero, -64(s0)
    sd    zero, -56(s0)
    li    a0, 1
    sw    a0, -88(s0)
    li    a0, 2
    sw    a0, -84(s0)
    # struct named nm = ...
    sd    zero, -104(s0)
    sb    zero, -96(s0)
    sb    zero, -95(s0)
    sb    zero, -94(s0)
    sb    zero, -93(s0)
    la    a0, .L.str.7
    addi  a1, s0, -104
    lbu   t0, 0(a0)
    sb    t0, 0(a1)
    lbu   t0, 1(a0)
    sb    t0, 1(a1)
    lbu   t0, 2(a0)
    sb    t0, 2(a1)
    lbu   t0, 3(a0)
    sb    t0, 3(a1)
    lbu   t0, 4(a0)
    sb    t0, 4(a1)
    lbu   t0, 5(a0)
    sb    t0, 5(a1)
    lbu   t0, 6(a0)
    sb    t0, 6(a1)
    lbu   t0, 7(a0)
    sb    t0, 7(a1)
    lw    a0, -20(s0)
    sw    a0, -96(s0)
    # compound literal (int[5])
    sd    zero, -136(s0)
    sd    zero, -128(s0)
    sb    zero, -120(s0)
    sb    zero, -119(s0)
    sb    zero, -118(s0)
    sb    zero, -117(s0)
    li    a0, 1
    sw    a0, -136(s0)
    li    a0, 2
    sw    a0, -132(s0)
    li    a0, 3
    sw    a0, -128(s0)
    li    a0, 4
    sw    a0, -124(s0)
    li    a0, 5
    sw    a0, -120(s0)
    addi  a0, s0, -136
    sd    a0, -112(s0)
    # compound literal (int[5])
    sd    zero, -168(s0)
    sd    zero, -160(s0)
    sb    zero, -152(s0)
    sb    zero, -151(s0)
    sb    zero, -150(s0)
    sb    zero, -149(s0)
    li    a0, 5
    sw    a0, -168(s0)
    li    a0, 4
    sw    a0, -164(s0)
    li    a0, 3
    sw    a0, -160(s0)
    li    a0, 2
    sw    a0, -156(s0)
    li    a0, 1
    sw    a0, -152(s0)
    addi  a0, s0, -168
    sd    a0, -144(s0)
    addi  a0, s0, -32
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -88
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -104
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    lbu   a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 98
    ld    a1, 0(sp)
    addi  sp, sp, 8
    subw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -104
    addi  a0, a0, 8
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -112(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 4
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -144(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 5
    ld    a1, 0(sp)
    addi  sp, sp, 8
    subw  a0, a1, a0
.L.return.copies:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size copies, . - copies

    # int main(void): frame 32 bytes
    #     -20(s0)  local int sum
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    la    a0, greeting
    ld    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 104
    ld    a1, 0(sp)
    addi  sp, sp, 8
    subw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, farewell
    ld    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    lbu   a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 121
    ld    a1, 0(sp)
    addi  sp, sp, 8
    subw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, again
    ld    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    lbu   a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 105
    ld    a1, 0(sp)
    addi  sp, sp, 8
    subw  a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, ones
    ld    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, ones
    ld    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, twos
    ld    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  sp, sp, -8
    call  count
    addi  sp, sp, 8
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  sp, sp, -8
    call  other
    addi  sp, sp, 8
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -8
    call  copies
    addi  sp, sp, 8
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, late
    ld    a0, 0(a0)
    addi  a0, a0, 4
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, .L.str.9
    lbu   a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 120
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
.L.return.main:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size main, . - main

    .data

    # static struct pair .compoundliteral.10: 8 bytes
    .balign 4
    .type .compoundliteral.10, @object
    .size .compoundliteral.10, 8
.compoundliteral.10:
    .word 3
    .word 4

    .section .rodata

.L.str:
    .asciz "hi"

.L.str.1:
    .asciz "bye"

.L.str.3:
    .asciz "count"

.L.str.7:
    .ascii "ab"
    .byte 0, 0, 0, 0, 0, 0

.L.str.9:
    .asciz "x"
