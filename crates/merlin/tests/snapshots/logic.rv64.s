    .text

    # int bump(int v): frame 32 bytes
    #     -20(s0)  param int v
    .globl bump
    .type bump, @function
bump:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
.L.return.bump:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size bump, . - bump

    # int main(void): frame 32 bytes
    #     -20(s0)  local int hits
    #     -24(s0)  local int r
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    li    a0, 0
    sw    a0, -20(s0)
    li    a0, 5
    beqz  a0, .L.and.1
    li    a0, 7
.L.and.1:
    snez  a0, a0
    sw    a0, -24(s0)
    lw    a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    bnez  a0, .L.or.2
    li    a0, 3
    negw  a0, a0
.L.or.2:
    snez  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -24(s0)
    lw    a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    beqz  a0, .L.and.3
    lw    a0, -20(s0)
    addi  sp, sp, -8
    call  bump
    addi  sp, sp, 8
    sw    a0, -20(s0)
.L.and.3:
    snez  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -24(s0)
    lw    a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    bnez  a0, .L.or.4
    lw    a0, -20(s0)
    addi  sp, sp, -8
    call  bump
    addi  sp, sp, 8
    sw    a0, -20(s0)
.L.or.4:
    snez  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -24(s0)
    lw    a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    beqz  a0, .L.and.5
    lw    a0, -20(s0)
    addi  sp, sp, -8
    call  bump
    addi  sp, sp, 8
    sw    a0, -20(s0)
.L.and.5:
    snez  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -24(s0)
    lw    a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    bnez  a0, .L.or.6
    lw    a0, -20(s0)
    addi  sp, sp, -8
    call  bump
    addi  sp, sp, 8
    sw    a0, -20(s0)
.L.or.6:
    snez  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -24(s0)
    lw    a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -20(s0)
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
