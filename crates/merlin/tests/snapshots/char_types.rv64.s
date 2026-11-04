    .text

    # int main(void): frame 64 bytes
    #     -17(s0)  local char c
    #     -18(s0)  local signed char sc
    #     -19(s0)  local unsigned char uc
    #     -22(s0)  local short s
    #     -24(s0)  local unsigned short us
    #     -28(s0)  local int r
    #     -29(s0)  local char a
    #     -30(s0)  local char b
    #     -31(s0)  local char sum
    #     -32(s0)  local signed char x
    #     -33(s0)  local signed char y
    #     -34(s0)  local signed char wrapped
    #     -40(s0)  local unsigned char bytes[4]
    #     -48(s0)  local signed char *sp
    #     -50(s0)  local unsigned short one
    #     -51(s0)  local unsigned char uc2
    #     -52(s0)  local signed char sc2
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -48
    li    a0, 200
    sb    a0, -17(s0)
    li    a0, -56
    sb    a0, -18(s0)
    li    a0, 44
    sb    a0, -19(s0)
    li    a0, -25536
    sh    a0, -22(s0)
    li    a0, 1
    negw  a0, a0
    slli  a0, a0, 48
    srli  a0, a0, 48
    sh    a0, -24(s0)
    li    a0, 0
    sw    a0, -28(s0)
    lw    a0, -28(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lbu   a0, -17(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slt   a0, a0, a1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -28(s0)
    lw    a0, -28(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lb    a0, -18(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slt   a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -28(s0)
    lw    a0, -28(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lbu   a0, -19(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -28(s0)
    lw    a0, -28(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lh    a0, -22(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 25536
    negw  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -28(s0)
    lw    a0, -28(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lhu   a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 65535
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -28(s0)
    li    a0, 100
    sb    a0, -29(s0)
    li    a0, 100
    sb    a0, -30(s0)
    lw    a0, -28(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lbu   a0, -29(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lbu   a0, -30(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 200
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -28(s0)
    lbu   a0, -29(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lbu   a0, -30(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    andi  a0, a0, 255
    sb    a0, -31(s0)
    lw    a0, -28(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lbu   a0, -31(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 200
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -28(s0)
    li    a0, 100
    sb    a0, -32(s0)
    li    a0, 100
    sb    a0, -33(s0)
    lb    a0, -32(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lb    a0, -33(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    slli  a0, a0, 56
    srai  a0, a0, 56
    sb    a0, -34(s0)
    lw    a0, -28(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lb    a0, -34(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 56
    negw  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -28(s0)
    lw    a0, -28(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lbu   a0, -17(s0)
    negw  a0, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 200
    negw  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -28(s0)
    # unsigned char bytes[4] = ...
    sb    zero, -40(s0)
    sb    zero, -39(s0)
    sb    zero, -38(s0)
    sb    zero, -37(s0)
    li    a0, 255
    sb    a0, -40(s0)
    li    a0, 128
    sb    a0, -39(s0)
    li    a0, 127
    sb    a0, -38(s0)
    li    a0, 0
    sb    a0, -37(s0)
    lw    a0, -28(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -40
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -40
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
    addi  a0, s0, -40
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    lbu   a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    subw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 256
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -28(s0)
    addi  a0, s0, -40
    sd    a0, -48(s0)
    lw    a0, -28(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -48(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    lb    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -48(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    lb    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -28(s0)
    li    a0, 1
    sh    a0, -50(s0)
    lw    a0, -28(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    negw  a0, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lhu   a0, -50(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slt   a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -28(s0)
    li    a0, 200
    sb    a0, -51(s0)
    li    a0, 2
    negw  a0, a0
    slli  a0, a0, 56
    srai  a0, a0, 56
    sb    a0, -52(s0)
    lw    a0, -28(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lbu   a0, -51(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lb    a0, -52(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    divw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 100
    negw  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -28(s0)
    lw    a0, -28(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 129
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    sext.w a0, a0
.L.return.main:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size main, . - main
