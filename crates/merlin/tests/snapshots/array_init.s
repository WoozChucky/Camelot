    .data

    # int g[5]: 20 bytes
    .globl g
    .balign 4
g:
    .word 10
    .word 20
    .zero 12

    # int inferred[4]: 16 bytes
    .globl inferred
    .balign 4
inferred:
    .word 1
    .word 2
    .word 3
    .word 4

    # int grid[2][3]: 24 bytes
    .globl grid
    .balign 4
grid:
    .word 1
    .word 2
    .word 3
    .word 4
    .zero 8

    # char word[8]: 8 bytes
    .globl word
word:
    .ascii "hey"
    .zero 5

    .text

    # int main(void): frame 64 bytes
    #     -32(s0)  local int a[6]
    #     -36(s0)  local char s[4]
    #     -40(s0)  local char exact[3]
    #     -48(s0)  local short h[4]
    #     -52(s0)  local int total
    #     -56(s0)  local int k
    .globl main
main:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -48
    # int a[6] = ...
    sw    zero, -32(s0)
    sw    zero, -28(s0)
    sw    zero, -24(s0)
    sw    zero, -20(s0)
    sw    zero, -16(s0)
    sw    zero, -12(s0)
    li    a0, 5
    sw    a0, -32(s0)
    li    a0, 6
    sw    a0, -28(s0)
    li    a0, 7
    sw    a0, -24(s0)
    # char s[4] = ...
    sw    zero, -36(s0)
    li    a0, 97
    sb    a0, -36(s0)
    li    a0, 98
    sb    a0, -35(s0)
    li    a0, 99
    sb    a0, -34(s0)
    # char exact[3] = ...
    sb    zero, -40(s0)
    sb    zero, -39(s0)
    sb    zero, -38(s0)
    li    a0, 120
    sb    a0, -40(s0)
    li    a0, 121
    sb    a0, -39(s0)
    li    a0, 122
    sb    a0, -38(s0)
    # short h[4] = ...
    sw    zero, -48(s0)
    sw    zero, -44(s0)
    li    a0, 1
    neg   a0, a0
    slli  a0, a0, 16
    srai  a0, a0, 16
    sh    a0, -48(s0)
    li    a0, 300
    sh    a0, -46(s0)
    li    a0, 0
    sw    a0, -52(s0)
    li    a0, 0
    sw    a0, -56(s0)
.L.loop.1:
    lw    a0, -56(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 6
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a1, a0
    beqz  a0, .L.break.3
    lw    a0, -52(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -32
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -56(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -52(s0)
.L.continue.2:
    lw    a0, -56(s0)
    addi  a0, a0, 1
    sw    a0, -56(s0)
    addi  a0, a0, -1
    j     .L.loop.1
.L.break.3:
    lw    a0, -52(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, g
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
    la    a0, g
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
    la    a0, g
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
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
    la    a0, g
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
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -52(s0)
    lw    a0, -52(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 16
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    lw    a1, 0(sp)
    addi  sp, sp, 4
    divu  a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, inferred
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
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
    sw    a0, -52(s0)
    lw    a0, -52(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, grid
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    li    t0, 12
    mul   a0, a0, t0
    add   a0, a1, a0
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
    la    a0, grid
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    li    t0, 12
    mul   a0, a0, t0
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
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
    la    a0, grid
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    li    t0, 12
    mul   a0, a0, t0
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
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
    sw    a0, -52(s0)
    lw    a0, -52(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -36
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -36
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 97
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -52(s0)
    lw    a0, -52(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, word
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 121
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, word
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, word
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 7
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -52(s0)
    lw    a0, -52(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -40
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 122
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -52(s0)
    lw    a0, -52(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -48
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 1
    add   a0, a1, a0
    lh    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -48
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 1
    add   a0, a1, a0
    lh    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 10
    lw    a1, 0(sp)
    addi  sp, sp, 4
    div   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -48
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 1
    add   a0, a1, a0
    lh    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -52(s0)
    lw    a0, -52(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -48
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 1
    add   a0, a1, a0
    lh    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
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
