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
    .quad .compoundliteral.7

    .text

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
    la    a0, late
    ld    a0, 0(a0)
    addi  a0, a0, 4
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, .L.str.6
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
