    .file "symbol_names.c"
    .data

    # const char *greeting: 4 bytes
    .globl greeting
    .balign 4
    .type greeting, @object
    .size greeting, 4
greeting:
    .word .L.str

    # static int .compoundliteral[2]: 8 bytes
    .balign 4
    .type .compoundliteral, @object
    .size .compoundliteral, 8
.compoundliteral:
    .word 1
    .word 1

    # int *ones: 4 bytes
    .globl ones
    .balign 4
    .type ones, @object
    .size ones, 4
ones:
    .word .compoundliteral

    # const char *farewell: 4 bytes
    .globl farewell
    .balign 4
    .type farewell, @object
    .size farewell, 4
farewell:
    .word .L.str.1

    # static int .compoundliteral.2[1]: 4 bytes
    .balign 4
    .type .compoundliteral.2, @object
    .size .compoundliteral.2, 4
.compoundliteral.2:
    .word 2

    # int *twos: 4 bytes
    .globl twos
    .balign 4
    .type twos, @object
    .size twos, 4
twos:
    .word .compoundliteral.2

    # const char *again: 4 bytes
    .globl again
    .balign 4
    .type again, @object
    .size again, 4
again:
    .word .L.str

    .text

    # int count(void): frame 16 bytes
    .globl count
    .type count, @function
count:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    la    a0, count.n.4
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, 0(sp)
    lw    a0, 0(a0)
    addi  a0, a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sw    a0, 0(a1)
    addi  a0, a0, -1
    la    a0, count.n
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, count.tag
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 99
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
.L.return.count:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
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

    # static const char *tag: 4 bytes
    .balign 4
    .type count.tag, @object
    .size count.tag, 4
count.tag:
    .word .L.str.3

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
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    la    a0, other.n.5
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, 0(sp)
    lw    a0, 0(a0)
    addi  a0, a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sw    a0, 0(a1)
    addi  a0, a0, -1
    la    a0, other.n
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, late
    lw    a0, 0(a0)
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
.L.return.other:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
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

    # static struct pair *late: 4 bytes
    .balign 4
    .type late, @object
    .size late, 4
late:
    .word .compoundliteral.14

    .text

    # int copies(int k): frame 144 bytes
    #     -12(s0)  param int k
    #     -24(s0)  local int small[3]
    #     -36(s0)  local int small[3]
    #     -76(s0)  local int few[10]
    #     -88(s0)  local struct named nm
    #     -92(s0)  local int *five
    #    -112(s0)  temp int[5] (compound literal)
    #    -116(s0)  local int *more
    #    -136(s0)  temp int[5] (compound literal)
    .globl copies
    .type copies, @function
copies:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -128
    sw    a0, -12(s0)
    # int small[3] = ...
    sw    zero, -24(s0)
    sw    zero, -20(s0)
    sw    zero, -16(s0)
    li    a0, 1
    sw    a0, -24(s0)
    li    a0, 2
    sw    a0, -20(s0)
    li    a0, 3
    sw    a0, -16(s0)
    # int small[3] = ...
    sw    zero, -36(s0)
    sw    zero, -32(s0)
    sw    zero, -28(s0)
    li    a0, 4
    sw    a0, -36(s0)
    li    a0, 5
    sw    a0, -32(s0)
    li    a0, 6
    sw    a0, -28(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -36
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    # int few[10] = ...
    sw    zero, -76(s0)
    sw    zero, -72(s0)
    sw    zero, -68(s0)
    sw    zero, -64(s0)
    sw    zero, -60(s0)
    sw    zero, -56(s0)
    sw    zero, -52(s0)
    sw    zero, -48(s0)
    sw    zero, -44(s0)
    sw    zero, -40(s0)
    li    a0, 1
    sw    a0, -76(s0)
    li    a0, 2
    sw    a0, -72(s0)
    # struct named nm = ...
    sw    zero, -88(s0)
    sw    zero, -84(s0)
    sw    zero, -80(s0)
    la    a0, .L.str.7
    addi  a1, s0, -88
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
    lw    a0, -12(s0)
    sw    a0, -80(s0)
    # compound literal (int[5])
    sw    zero, -112(s0)
    sw    zero, -108(s0)
    sw    zero, -104(s0)
    sw    zero, -100(s0)
    sw    zero, -96(s0)
    li    a0, 1
    sw    a0, -112(s0)
    li    a0, 2
    sw    a0, -108(s0)
    li    a0, 3
    sw    a0, -104(s0)
    li    a0, 4
    sw    a0, -100(s0)
    li    a0, 5
    sw    a0, -96(s0)
    addi  a0, s0, -112
    sw    a0, -92(s0)
    # compound literal (int[5])
    sw    zero, -136(s0)
    sw    zero, -132(s0)
    sw    zero, -128(s0)
    sw    zero, -124(s0)
    sw    zero, -120(s0)
    li    a0, 5
    sw    a0, -136(s0)
    li    a0, 4
    sw    a0, -132(s0)
    li    a0, 3
    sw    a0, -128(s0)
    li    a0, 2
    sw    a0, -124(s0)
    li    a0, 1
    sw    a0, -120(s0)
    addi  a0, s0, -136
    sw    a0, -116(s0)
    addi  a0, s0, -24
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -76
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -88
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 98
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -88
    addi  a0, a0, 8
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -92(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -116(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 5
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
.L.return.copies:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size copies, . - copies

    # int dead(int k): frame 16 bytes
    #     -12(s0)  param int k
    #     -16(s0)  local int m
    .globl dead
    .type dead, @function
dead:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    li    t0, 1
    beq   a0, t0, .L.case.dead.0
    j     .L.case.dead.1
.L.case.dead.0:
    la    a0, .L.str.9
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 111
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
    j     .L.return.dead
.L.case.dead.1:
    j     .L.break.1
.L.break.1:
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.else.2
    li    a0, 2
    j     .L.return.dead
.L.else.2:
    j     .L.label.dead.out
.L.label.dead.out:
.L.loop.4:
    lw    a0, -12(s0)
    addi  a0, a0, 1
    sw    a0, -12(s0)
    addi  a0, a0, -1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a0, a1
    beqz  a0, .L.end.7
    j     .L.break.6
.L.end.7:
    j     .L.continue.5
.L.continue.5:
    j     .L.loop.4
.L.break.6:
.L.continue.8:
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, .L.str.11
    lbu   a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 108
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, dead.n.10
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
.L.return.dead:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size dead, . - dead

    .data

    # static int n: 4 bytes
    .balign 4
    .type dead.n, @object
    .size dead.n, 4
dead.n:
    .word 3

    # static int n: 4 bytes
    .balign 4
    .type dead.n.10, @object
    .size dead.n.10, 4
dead.n.10:
    .word 4

    .text

    # int main(void): frame 16 bytes
    #     -12(s0)  local int sum
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    la    a0, greeting
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 104
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, farewell
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 121
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, again
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 105
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, ones
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, ones
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, twos
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  sp, sp, -12
    call  count
    addi  sp, sp, 12
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  sp, sp, -12
    call  other
    addi  sp, sp, 12
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -12
    call  copies
    addi  sp, sp, 12
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -12
    call  dead
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 5
    addi  sp, sp, -8
    call  dead
    addi  sp, sp, 8
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, table
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, late
    lw    a0, 0(a0)
    addi  a0, a0, 4
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, .L.str.12
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 120
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
.L.return.main:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size main, . - main

    .data

    # int table[3]: 12 bytes
    .globl table
    .balign 4
    .type table, @object
    .size table, 12
table:
    .word 1
    .word 2
    .word 3

    # static struct pair .compoundliteral.14: 8 bytes
    .balign 4
    .type .compoundliteral.14, @object
    .size .compoundliteral.14, 8
.compoundliteral.14:
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
    .asciz "one"

.L.str.11:
    .asciz "loop"

.L.str.12:
    .asciz "x"
