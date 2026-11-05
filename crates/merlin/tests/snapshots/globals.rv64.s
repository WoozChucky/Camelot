    .bss

    # int counter: 4 bytes
    .globl counter
    .balign 4
    .type counter, @object
    .size counter, 4
counter:
    .zero 4

    .data

    # int base: 4 bytes
    .globl base
    .balign 4
    .type base, @object
    .size base, 4
base:
    .word 40

    # int *pbase: 8 bytes
    .globl pbase
    .balign 8
    .type pbase, @object
    .size pbase, 8
pbase:
    .quad base

    # static int hidden: 4 bytes
    .balign 4
    .type hidden, @object
    .size hidden, 4
hidden:
    .word 5

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

    # int *third: 8 bytes
    .globl third
    .balign 8
    .type third, @object
    .size third, 8
third:
    .quad table+8

    # int *past: 8 bytes
    .globl past
    .balign 8
    .type past, @object
    .size past, 8
past:
    .quad table+16

    .text

    # void bump(void): frame 16 bytes
    .globl bump
    .type bump, @function
bump:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    la    a0, counter
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, 0(sp)
    lw    a0, 0(a0)
    addiw a0, a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sw    a0, 0(a1)
    addiw a0, a0, -1
    la    a0, later
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, 0(sp)
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sw    a0, 0(a1)
    li    a0, 0
.L.return.bump:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size bump, . - bump

    .data

    # int later: 4 bytes
    .globl later
    .balign 4
    .type later, @object
    .size later, 4
later:
    .word 10

    .text

    # int main(void): frame 32 bytes
    #     -20(s0)  local int r
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    call  bump
    call  bump
    la    a0, pbase
    ld    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, 0(sp)
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sw    a0, 0(a1)
    la    a0, counter
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, later
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, base
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, hidden
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, third
    ld    a0, 0(a0)
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, past
    ld    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, third
    ld    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sub   a0, a1, a0
    srai  a0, a0, 2
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    sext.w a0, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, past
    ld    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, -1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, pbase
    ld    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, base
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
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
