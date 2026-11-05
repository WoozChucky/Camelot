    .data

    # int base: 4 bytes
    .globl base
    .balign 4
    .type base, @object
    .size base, 4
base:
    .word 40

    # int *pbase: 4 bytes
    .globl pbase
    .balign 4
    .type pbase, @object
    .size pbase, 4
pbase:
    .word base

    # int table[4]: 16 bytes
    .globl table
    .balign 4
    .type table, @object
    .size table, 16
table:
    .word 1
    .word 2
    .word 3
    .word 4

    # int *third: 4 bytes
    .globl third
    .balign 4
    .type third, @object
    .size third, 4
third:
    .word table+8

    # int *past: 4 bytes
    .globl past
    .balign 4
    .type past, @object
    .size past, 4
past:
    .word table+16

    .text

    # void bump(void): frame 16 bytes
    .globl bump
    .type bump, @function
bump:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    la    a0, counter
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, 0(sp)
    lw    a0, 0(a0)
    addi  a0, a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sw    a0, 0(a1)
    addi  a0, a0, -1
    la    a0, later
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, 0(sp)
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sw    a0, 0(a1)
    li    a0, 0
.L.return.bump:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size bump, . - bump

    .bss

    # int counter: 4 bytes
    .globl counter
    .balign 4
    .type counter, @object
    .size counter, 4
counter:
    .zero 4

    .data

    # int later: 4 bytes
    .globl later
    .balign 4
    .type later, @object
    .size later, 4
later:
    .word 10

    .text

    # int main(void): frame 16 bytes
    #     -12(s0)  local int r
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    call  bump
    call  bump
    la    a0, pbase
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, 0(sp)
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sw    a0, 0(a1)
    la    a0, counter
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, later
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, base
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, hidden
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, third
    lw    a0, 0(a0)
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, past
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, third
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
    srai  a0, a0, 2
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
    la    a0, past
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, -1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, pbase
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, base
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
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

    # static int hidden: 4 bytes
    .balign 4
    .type hidden, @object
    .size hidden, 4
hidden:
    .word 5
