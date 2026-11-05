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
    .word .compoundliteral.7

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
    la    a0, late
    lw    a0, 0(a0)
    addi  a0, a0, 4
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, .L.str.6
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

    # static struct pair .compoundliteral.7: 8 bytes
    .balign 4
    .type .compoundliteral.7, @object
    .size .compoundliteral.7, 8
.compoundliteral.7:
    .word 3
    .word 4

    .section .rodata

.L.str:
    .asciz "hi"

.L.str.1:
    .asciz "bye"

.L.str.3:
    .asciz "count"

.L.str.6:
    .asciz "x"
