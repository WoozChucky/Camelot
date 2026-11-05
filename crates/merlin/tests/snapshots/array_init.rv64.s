    .data

    # int g[5]: 20 bytes
    .globl g
    .balign 4
    .type g, @object
    .size g, 20
g:
    .word 10
    .word 20
    .zero 12

    # int inferred[4]: 16 bytes
    .globl inferred
    .balign 4
    .type inferred, @object
    .size inferred, 16
inferred:
    .word 1
    .word 2
    .word 3
    .word 4

    # int grid[2][3]: 24 bytes
    .globl grid
    .balign 4
    .type grid, @object
    .size grid, 24
grid:
    .word 1
    .word 2
    .word 3
    .word 4
    .zero 8

    # char word[8]: 8 bytes
    .globl word
    .type word, @object
    .size word, 8
word:
    .ascii "hey"
    .zero 5

    .text

    # int main(void): frame 80 bytes
    #     -40(s0)  local int a[6]
    #     -48(s0)  local char s[4]
    #     -56(s0)  local char exact[3]
    #     -64(s0)  local short h[4]
    #     -68(s0)  local int total
    #     -72(s0)  local int k
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -64
    # int a[6] = ...
    sd    zero, -40(s0)
    sd    zero, -32(s0)
    sd    zero, -24(s0)
    li    a0, 5
    sw    a0, -40(s0)
    li    a0, 6
    sw    a0, -36(s0)
    li    a0, 7
    sw    a0, -32(s0)
    # char s[4] = ...
    sb    zero, -48(s0)
    sb    zero, -47(s0)
    sb    zero, -46(s0)
    sb    zero, -45(s0)
    li    a0, 97
    sb    a0, -48(s0)
    li    a0, 98
    sb    a0, -47(s0)
    li    a0, 99
    sb    a0, -46(s0)
    # char exact[3] = ...
    sb    zero, -56(s0)
    sb    zero, -55(s0)
    sb    zero, -54(s0)
    li    a0, 120
    sb    a0, -56(s0)
    li    a0, 121
    sb    a0, -55(s0)
    li    a0, 122
    sb    a0, -54(s0)
    # short h[4] = ...
    sd    zero, -64(s0)
    li    a0, -1
    sh    a0, -64(s0)
    li    a0, 300
    sh    a0, -62(s0)
    li    a0, 0
    sw    a0, -68(s0)
    li    a0, 0
    sw    a0, -72(s0)
.L.loop.1:
    lw    a0, -72(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 6
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slt   a0, a1, a0
    beqz  a0, .L.break.3
    lw    a0, -68(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -40
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -72(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -68(s0)
.L.continue.2:
    lw    a0, -72(s0)
    addiw a0, a0, 1
    sw    a0, -72(s0)
    addiw a0, a0, -1
    j     .L.loop.1
.L.break.3:
    lw    a0, -68(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, g
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
    la    a0, g
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
    la    a0, g
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
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
    la    a0, g
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
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -68(s0)
    lw    a0, -68(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 4
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, inferred
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    sext.w a0, a0
    sw    a0, -68(s0)
    lw    a0, -68(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, grid
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    li    t0, 12
    mul   a0, a0, t0
    add   a0, a1, a0
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
    la    a0, grid
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    li    t0, 12
    mul   a0, a0, t0
    add   a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
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
    la    a0, grid
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    li    t0, 12
    mul   a0, a0, t0
    add   a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
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
    sw    a0, -68(s0)
    lw    a0, -68(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 4
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -48
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -48
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 97
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    sext.w a0, a0
    sw    a0, -68(s0)
    lw    a0, -68(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, word
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 121
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, word
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, word
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 7
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -68(s0)
    lw    a0, -68(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -56
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 122
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    sext.w a0, a0
    sw    a0, -68(s0)
    lw    a0, -68(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -64
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 1
    add   a0, a1, a0
    lh    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -64
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 1
    add   a0, a1, a0
    lh    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 10
    ld    a1, 0(sp)
    addi  sp, sp, 8
    divw  a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -64
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 1
    add   a0, a1, a0
    lh    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -68(s0)
    lw    a0, -68(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -64
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 1
    add   a0, a1, a0
    lh    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
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
