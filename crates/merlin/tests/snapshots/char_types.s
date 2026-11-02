    .text

    # int main(void): frame 48 bytes
    #      -9(s0)  local char c
    #     -10(s0)  local signed char sc
    #     -11(s0)  local unsigned char uc
    #     -14(s0)  local short s
    #     -16(s0)  local unsigned short us
    #     -20(s0)  local int r
    #     -21(s0)  local char a
    #     -22(s0)  local char b
    #     -23(s0)  local char sum
    #     -24(s0)  local signed char x
    #     -25(s0)  local signed char y
    #     -26(s0)  local signed char wrapped
    #     -32(s0)  local unsigned char bytes[4]
    #     -36(s0)  local signed char *sp
    #     -38(s0)  local unsigned short one
    #     -39(s0)  local unsigned char uc2
    #     -40(s0)  local signed char sc2
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -32
    li    a0, 200
    sb    a0, -9(s0)
    li    a0, -56
    sb    a0, -10(s0)
    li    a0, 44
    sb    a0, -11(s0)
    li    a0, -25536
    sh    a0, -14(s0)
    li    a0, 1
    neg   a0, a0
    slli  a0, a0, 16
    srli  a0, a0, 16
    sh    a0, -16(s0)
    li    a0, 0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lbu   a0, -9(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a0, a1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lb    a0, -10(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lbu   a0, -11(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lh    a0, -14(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 25536
    neg   a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lhu   a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 65535
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -20(s0)
    li    a0, 100
    sb    a0, -21(s0)
    li    a0, 100
    sb    a0, -22(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lbu   a0, -21(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lbu   a0, -22(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 200
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -20(s0)
    lbu   a0, -21(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lbu   a0, -22(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    andi  a0, a0, 255
    sb    a0, -23(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lbu   a0, -23(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 200
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -20(s0)
    li    a0, 100
    sb    a0, -24(s0)
    li    a0, 100
    sb    a0, -25(s0)
    lb    a0, -24(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lb    a0, -25(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    slli  a0, a0, 24
    srai  a0, a0, 24
    sb    a0, -26(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lb    a0, -26(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 56
    neg   a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lbu   a0, -9(s0)
    neg   a0, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 200
    neg   a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -20(s0)
    # unsigned char bytes[4] = ...
    sw    zero, -32(s0)
    li    a0, 255
    sb    a0, -32(s0)
    li    a0, 128
    sb    a0, -31(s0)
    li    a0, 127
    sb    a0, -30(s0)
    li    a0, 0
    sb    a0, -29(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -32
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -32
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
    addi  a0, s0, -32
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 256
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -20(s0)
    addi  a0, s0, -32
    sw    a0, -36(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -36(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lb    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -36(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lb    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -20(s0)
    li    a0, 1
    sh    a0, -38(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    neg   a0, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lhu   a0, -38(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -20(s0)
    li    a0, 200
    sb    a0, -39(s0)
    li    a0, 2
    neg   a0, a0
    slli  a0, a0, 24
    srai  a0, a0, 24
    sb    a0, -40(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lbu   a0, -39(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lb    a0, -40(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    div   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 100
    neg   a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 129
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
