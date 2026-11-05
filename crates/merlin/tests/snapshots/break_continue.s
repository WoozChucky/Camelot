    .file "break_continue.c"
    .text

    # int main(void): frame 48 bytes
    #     -20(s0)  local int sum
    #     -24(s0)  local int i
    #     -28(s0)  local int n
    #     -32(s0)  local int k
    #     -36(s0)  local int m
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -32
    li    a0, 0
    sw    a0, -20(s0)
    li    a0, 0
    sw    a0, -24(s0)
.L.loop.1:
    lw    a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 100
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slt   a0, a1, a0
    beqz  a0, .L.break.3
    lw    a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    ld    a1, 0(sp)
    addi  sp, sp, 8
    remw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.end.4
    j     .L.continue.2
.L.end.4:
    lw    a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 9
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slt   a0, a0, a1
    beqz  a0, .L.end.5
    j     .L.break.3
.L.end.5:
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -24(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
.L.continue.2:
    lw    a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -24(s0)
    j     .L.loop.1
.L.break.3:
    li    a0, 0
    sw    a0, -28(s0)
.L.continue.6:
    lw    a0, -28(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -28(s0)
    lw    a0, -28(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 5
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slt   a0, a1, a0
    beqz  a0, .L.end.8
    j     .L.continue.6
.L.end.8:
    j     .L.break.7
.L.break.7:
    li    a0, 0
    sw    a0, -32(s0)
.L.continue.9:
    lw    a0, -32(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slt   a0, a1, a0
    beqz  a0, .L.break.10
    li    a0, 0
    sw    a0, -36(s0)
.L.loop.11:
    lw    a0, -36(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.end.14
    j     .L.break.13
.L.end.14:
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
.L.continue.12:
    lw    a0, -36(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -36(s0)
    j     .L.loop.11
.L.break.13:
    lw    a0, -32(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -32(s0)
    j     .L.continue.9
.L.break.10:
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -28(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -32(s0)
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
