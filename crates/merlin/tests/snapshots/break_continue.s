    .file "break_continue.c"
    .text

    # int main(void): frame 32 bytes
    #     -12(s0)  local int sum
    #     -16(s0)  local int i
    #     -20(s0)  local int n
    #     -24(s0)  local int k
    #     -28(s0)  local int m
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    li    a0, 0
    sw    a0, -12(s0)
    li    a0, 0
    sw    a0, -16(s0)
.L.loop.1:
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 100
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a1, a0
    beqz  a0, .L.break.3
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    rem   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.end.4
    j     .L.continue.2
.L.end.4:
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 9
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a0, a1
    beqz  a0, .L.end.5
    j     .L.break.3
.L.end.5:
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -16(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
.L.continue.2:
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -16(s0)
    j     .L.loop.1
.L.break.3:
    li    a0, 0
    sw    a0, -20(s0)
.L.continue.6:
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 5
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a1, a0
    beqz  a0, .L.end.8
    j     .L.continue.6
.L.end.8:
    j     .L.break.7
    j     .L.continue.6
.L.break.7:
    li    a0, 0
    sw    a0, -24(s0)
.L.continue.9:
    lw    a0, -24(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a1, a0
    beqz  a0, .L.break.10
    li    a0, 0
    sw    a0, -28(s0)
.L.loop.11:
    lw    a0, -28(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.end.14
    j     .L.break.13
.L.end.14:
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
.L.continue.12:
    lw    a0, -28(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -28(s0)
    j     .L.loop.11
.L.break.13:
    lw    a0, -24(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -24(s0)
    j     .L.continue.9
.L.break.10:
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -20(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -24(s0)
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
