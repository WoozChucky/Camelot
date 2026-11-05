    .text

    # int bump(void): frame 16 bytes
    .globl bump
    .type bump, @function
bump:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    la    a0, calls
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, 0(sp)
    lw    a0, 0(a0)
    addiw a0, a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sw    a0, 0(a1)
    addiw a0, a0, -1
    li    a0, 1
.L.return.bump:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size bump, . - bump

    .bss

    # int calls: 4 bytes
    .globl calls
    .balign 4
    .type calls, @object
    .size calls, 4
calls:
    .zero 4

    .text

    # int main(void): frame 32 bytes
    #     -20(s0)  local int r
    #     -24(s0)  local int x
    #     -28(s0)  local int dead
    #     -32(s0)  local int y
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
    li    a0, 3
    sw    a0, -24(s0)
    lw    a0, -20(s0)
    addiw a0, a0, 1
    sw    a0, -20(s0)
    addiw a0, a0, -1
    lw    a0, -20(s0)
    addiw a0, a0, 1
    sw    a0, -20(s0)
    addiw a0, a0, -1
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 4
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -24(s0)
    snez  a0, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.1
    lw    a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    ld    a1, 0(sp)
    addi  sp, sp, 8
    subw  a0, a1, a0
    snez  a0, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.1:
    snez  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
.L.continue.2:
    lw    a0, -24(s0)
    addiw a0, a0, 1
    sw    a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 6
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.end.4
    j     .L.break.3
.L.end.4:
    j     .L.continue.2
.L.break.3:
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 6
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
.L.do.5:
    lw    a0, -24(s0)
    addiw a0, a0, 1
    sw    a0, -24(s0)
    addiw a0, a0, -1
.L.continue.6:
.L.break.7:
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 7
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    li    a0, 10
    sw    a0, -24(s0)
    lw    a0, -24(s0)
    addiw a0, a0, 1
    sw    a0, -24(s0)
    addiw a0, a0, -1
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 11
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -24(s0)
    li    t0, 1
    beq   a0, t0, .L.case.main.5
    li    t0, 11
    beq   a0, t0, .L.case.main.6
    j     .L.break.8
.L.case.main.5:
    li    a0, 0
    beqz  a0, .L.end.9
.L.case.main.6:
    lw    a0, -20(s0)
    addiw a0, a0, 1
    sw    a0, -20(s0)
    addiw a0, a0, -1
.L.end.9:
.L.break.8:
    li    a0, 2
    sw    a0, -32(s0)
    lw    a0, -32(s0)
    sw    a0, -24(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    li    a0, 0
    beqz  a0, .L.end.10
.L.label.main.skipped:
    lw    a0, -20(s0)
    addiw a0, a0, 1
    sw    a0, -20(s0)
    addiw a0, a0, -1
    j     .L.label.main.done
.L.end.10:
    j     .L.label.main.skipped
.L.label.main.done:
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, calls
    lw    a0, 0(a0)
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
    sw    a0, -20(s0)
    lw    a0, -20(s0)
.L.return.main:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size main, . - main
