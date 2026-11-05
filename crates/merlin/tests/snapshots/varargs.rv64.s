    .file "varargs.c"
    .text

    # int sum(int n, ...): frame 96 bytes
    #     -56(s0)  a1-a7, saved for va_arg
    #     -76(s0)  param int n
    #     -88(s0)  local void *ap
    #     -92(s0)  local int s
    .globl sum
    .type sum, @function
sum:
    addi  sp, sp, -80
    sd    ra, 16(sp)
    sd    s0, 8(sp)
    addi  s0, sp, 80
    addi  sp, sp, -16
    # the variadic arguments in registers, next to those on the stack
    sd    a1, -56(s0)
    sd    a2, -48(s0)
    sd    a3, -40(s0)
    sd    a4, -32(s0)
    sd    a5, -24(s0)
    sd    a6, -16(s0)
    sd    a7, -8(s0)
    sw    a0, -76(s0)
    li    a0, 0
    sw    a0, -92(s0)
    addi  a0, s0, -56
    sd    a0, -88(s0)
.L.continue.1:
    lw    a0, -76(s0)
    addiw a0, a0, -1
    sw    a0, -76(s0)
    addiw a0, a0, 1
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slt   a0, a0, a1
    beqz  a0, .L.break.2
    lw    a0, -92(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -88(s0)
    mv    t1, a0
    addi  a0, a0, 8
    sd    a0, -88(s0)
    lw    a0, 0(t1)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -92(s0)
    j     .L.continue.1
.L.break.2:
    ld    a0, -88(s0)
    lw    a0, -92(s0)
.L.return.sum:
    addi  sp, s0, -80
    ld    ra, 16(sp)
    ld    s0, 8(sp)
    addi  sp, sp, 80
    ret
    .size sum, . - sum

    # int mix(const char *fmt, ...): frame 144 bytes
    #     -56(s0)  a1-a7, saved for va_arg
    #     -80(s0)  param const char *fmt
    #     -88(s0)  local void *ap
    #     -96(s0)  local void *again
    #    -100(s0)  local int s
    #    -112(s0)  local const char *p
    #    -120(s0)  local struct pair q
    #    -136(s0)  local struct big b
    .globl mix
    .type mix, @function
mix:
    addi  sp, sp, -80
    sd    ra, 16(sp)
    sd    s0, 8(sp)
    addi  s0, sp, 80
    addi  sp, sp, -64
    # the variadic arguments in registers, next to those on the stack
    sd    a1, -56(s0)
    sd    a2, -48(s0)
    sd    a3, -40(s0)
    sd    a4, -32(s0)
    sd    a5, -24(s0)
    sd    a6, -16(s0)
    sd    a7, -8(s0)
    sd    a0, -80(s0)
    li    a0, 0
    sw    a0, -100(s0)
    addi  a0, s0, -56
    sd    a0, -88(s0)
    ld    a0, -88(s0)
    sd    a0, -96(s0)
    ld    a0, -80(s0)
    sd    a0, -112(s0)
.L.loop.3:
    ld    a0, -112(s0)
    lbu   a0, 0(a0)
    beqz  a0, .L.break.5
    ld    a0, -112(s0)
    lbu   a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 105
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.else.6
    lw    a0, -100(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -88(s0)
    mv    t1, a0
    addi  a0, a0, 8
    sd    a0, -88(s0)
    lw    a0, 0(t1)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -100(s0)
    j     .L.end.7
.L.else.6:
    ld    a0, -112(s0)
    lbu   a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 99
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.else.8
    lw    a0, -100(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -88(s0)
    mv    t1, a0
    addi  a0, a0, 8
    sd    a0, -88(s0)
    lw    a0, 0(t1)
    slli  a0, a0, 56
    srai  a0, a0, 56
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -100(s0)
    j     .L.end.9
.L.else.8:
    ld    a0, -112(s0)
    lbu   a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 117
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.else.10
    lw    a0, -100(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -88(s0)
    mv    t1, a0
    addi  a0, a0, 8
    sd    a0, -88(s0)
    lw    a0, 0(t1)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 100
    ld    a1, 0(sp)
    addi  sp, sp, 8
    remuw a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -100(s0)
    j     .L.end.11
.L.else.10:
    ld    a0, -112(s0)
    lbu   a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 115
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.else.12
    lw    a0, -100(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -88(s0)
    mv    t1, a0
    addi  a0, a0, 8
    sd    a0, -88(s0)
    ld    a0, 0(t1)
    lbu   a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -100(s0)
    j     .L.end.13
.L.else.12:
    ld    a0, -112(s0)
    lbu   a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 112
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.else.14
    ld    a0, -88(s0)
    mv    t1, a0
    addi  a0, a0, 8
    sd    a0, -88(s0)
    mv    a0, t1
    addi  a1, s0, -120
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    a0, -100(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -120
    lh    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -120
    addi  a0, a0, 2
    lh    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -120
    addi  a0, a0, 4
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -100(s0)
    j     .L.end.15
.L.else.14:
    ld    a0, -112(s0)
    lbu   a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 98
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.end.16
    ld    a0, -88(s0)
    mv    t1, a0
    addi  a0, a0, 16
    sd    a0, -88(s0)
    mv    a0, t1
    addi  a1, s0, -136
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    lw    t0, 12(a0)
    sw    t0, 12(a1)
    lw    a0, -100(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -136
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
    addi  a0, s0, -136
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
    addw  a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -100(s0)
.L.end.16:
.L.end.15:
.L.end.13:
.L.end.11:
.L.end.9:
.L.end.7:
.L.continue.4:
    ld    a0, -112(s0)
    addi  a0, a0, 1
    sd    a0, -112(s0)
    addi  a0, a0, -1
    j     .L.loop.3
.L.break.5:
    ld    a0, -80(s0)
    lbu   a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 105
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.end.17
    lw    a0, -100(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1000
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -96(s0)
    mv    t1, a0
    addi  a0, a0, 8
    sd    a0, -96(s0)
    lw    a0, 0(t1)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    mulw  a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -100(s0)
.L.end.17:
    ld    a0, -96(s0)
    ld    a0, -88(s0)
    lw    a0, -100(s0)
.L.return.mix:
    addi  sp, s0, -80
    ld    ra, 16(sp)
    ld    s0, 8(sp)
    addi  sp, sp, 80
    ret
    .size mix, . - mix

    # int late(int a, int b, int c, int d, int e, int f, int g, ...): frame 80 bytes
    #     -8(s0)  a7, saved for va_arg
    #     -28(s0)  param int a
    #     -32(s0)  param int b
    #     -36(s0)  param int c
    #     -40(s0)  param int d
    #     -44(s0)  param int e
    #     -48(s0)  param int f
    #     -52(s0)  param int g
    #     -64(s0)  local void *ap
    #     -72(s0)  local struct pair q
    #     -76(s0)  local int x
    .globl late
    .type late, @function
late:
    addi  sp, sp, -32
    sd    ra, 16(sp)
    sd    s0, 8(sp)
    addi  s0, sp, 32
    addi  sp, sp, -48
    # the variadic arguments in registers, next to those on the stack
    sd    a7, -8(s0)
    sw    a0, -28(s0)
    sw    a1, -32(s0)
    sw    a2, -36(s0)
    sw    a3, -40(s0)
    sw    a4, -44(s0)
    sw    a5, -48(s0)
    sw    a6, -52(s0)
    addi  a0, s0, -8
    sd    a0, -64(s0)
    addi  a0, s0, -72
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -64(s0)
    mv    t1, a0
    addi  a0, a0, 8
    sd    a0, -64(s0)
    mv    a0, t1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    mv    a0, a1
    ld    a0, -64(s0)
    mv    t1, a0
    addi  a0, a0, 8
    sd    a0, -64(s0)
    lw    a0, 0(t1)
    sw    a0, -76(s0)
    ld    a0, -64(s0)
    lw    a0, -28(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -32(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -36(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -40(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -44(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -48(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -52(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -72
    lh    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -72
    addi  a0, a0, 2
    lh    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -72
    addi  a0, a0, 4
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -76(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
.L.return.late:
    addi  sp, s0, -32
    ld    ra, 16(sp)
    ld    s0, 8(sp)
    addi  sp, sp, 32
    ret
    .size late, . - late

    # int beyond(int a, int b, int c, int d, int e, int f, int g, int h, int i, ...): frame 80 bytes
    #     -20(s0)  param int a
    #     -24(s0)  param int b
    #     -28(s0)  param int c
    #     -32(s0)  param int d
    #     -36(s0)  param int e
    #     -40(s0)  param int f
    #     -44(s0)  param int g
    #     -48(s0)  param int h
    #     -52(s0)  param int i
    #     -64(s0)  local void *ap
    #     -68(s0)  local int s
    .globl beyond
    .type beyond, @function
beyond:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -64
    sw    a0, -20(s0)
    sw    a1, -24(s0)
    sw    a2, -28(s0)
    sw    a3, -32(s0)
    sw    a4, -36(s0)
    sw    a5, -40(s0)
    sw    a6, -44(s0)
    sw    a7, -48(s0)
    ld    t1, 0(s0)
    sw    t1, -52(s0)
    addi  a0, s0, 8
    sd    a0, -64(s0)
    ld    a0, -64(s0)
    mv    t1, a0
    addi  a0, a0, 8
    sd    a0, -64(s0)
    lw    a0, 0(t1)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 10
    ld    a1, 0(sp)
    addi  sp, sp, 8
    mulw  a0, a1, a0
    sw    a0, -68(s0)
    lw    a0, -68(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -64(s0)
    mv    t1, a0
    addi  a0, a0, 8
    sd    a0, -64(s0)
    lw    a0, 0(t1)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -68(s0)
    ld    a0, -64(s0)
    lw    a0, -68(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -24(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
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
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -36(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -40(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -44(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -48(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -52(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    subw  a0, a1, a0
.L.return.beyond:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size beyond, . - beyond

    # int vsum(int n, void *ap): frame 48 bytes
    #     -20(s0)  param int n
    #     -32(s0)  param void *ap
    #     -36(s0)  local int s
    .globl vsum
    .type vsum, @function
vsum:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -32
    sw    a0, -20(s0)
    sd    a1, -32(s0)
    li    a0, 0
    sw    a0, -36(s0)
.L.continue.18:
    lw    a0, -20(s0)
    addiw a0, a0, -1
    sw    a0, -20(s0)
    addiw a0, a0, 1
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slt   a0, a0, a1
    beqz  a0, .L.break.19
    lw    a0, -36(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -32(s0)
    mv    t1, a0
    addi  a0, a0, 8
    sd    a0, -32(s0)
    lw    a0, 0(t1)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -36(s0)
    j     .L.continue.18
.L.break.19:
    lw    a0, -36(s0)
.L.return.vsum:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size vsum, . - vsum

    # int forward(int n, ...): frame 96 bytes
    #     -56(s0)  a1-a7, saved for va_arg
    #     -76(s0)  param int n
    #     -88(s0)  local void *ap
    #     -92(s0)  local int s
    .globl forward
    .type forward, @function
forward:
    addi  sp, sp, -80
    sd    ra, 16(sp)
    sd    s0, 8(sp)
    addi  s0, sp, 80
    addi  sp, sp, -16
    # the variadic arguments in registers, next to those on the stack
    sd    a1, -56(s0)
    sd    a2, -48(s0)
    sd    a3, -40(s0)
    sd    a4, -32(s0)
    sd    a5, -24(s0)
    sd    a6, -16(s0)
    sd    a7, -8(s0)
    sw    a0, -76(s0)
    addi  a0, s0, -56
    sd    a0, -88(s0)
    lw    a0, -76(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -88(s0)
    mv    a1, a0
    ld    a0, 0(sp)
    addi  sp, sp, 8
    call  vsum
    sw    a0, -92(s0)
    ld    a0, -88(s0)
    lw    a0, -92(s0)
.L.return.forward:
    addi  sp, s0, -80
    ld    ra, 16(sp)
    ld    s0, 8(sp)
    addi  sp, sp, 80
    ret
    .size forward, . - forward

    # int main(void): frame 64 bytes
    #     -24(s0)  local struct pair q
    #     -40(s0)  local struct big b
    #     -41(s0)  local signed char c
    #     -44(s0)  local unsigned short us
    #     -56(s0)  local int (*fp)(int, ...)
    #     -60(s0)  local int r
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -48
    # struct pair q = ...
    sd    zero, -24(s0)
    li    a0, 1
    sh    a0, -24(s0)
    li    a0, 2
    sh    a0, -22(s0)
    li    a0, 3
    sw    a0, -20(s0)
    # struct big b = ...
    sd    zero, -40(s0)
    sd    zero, -32(s0)
    li    a0, 10
    sw    a0, -40(s0)
    li    a0, 20
    sw    a0, -36(s0)
    li    a0, 30
    sw    a0, -32(s0)
    li    a0, 40
    sw    a0, -28(s0)
    li    a0, -2
    sb    a0, -41(s0)
    li    a0, 65535
    sh    a0, -44(s0)
    la    a0, sum
    sd    a0, -56(s0)
    li    a0, 0
    sw    a0, -60(s0)
    li    a0, 0
    call  sum
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    snez  a0, a0
    beqz  a0, .L.end.20
    li    a0, 1
    j     .L.return.main
.L.end.20:
    li    a0, 3
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    mv    a3, a0
    ld    a2, 0(sp)
    addi  sp, sp, 8
    ld    a1, 0(sp)
    addi  sp, sp, 8
    ld    a0, 0(sp)
    addi  sp, sp, 8
    call  sum
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 6
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    snez  a0, a0
    beqz  a0, .L.end.21
    li    a0, 2
    j     .L.return.main
.L.end.21:
    li    a0, 12
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 4
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 5
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 6
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 7
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 8
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 9
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 10
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 11
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 12
    addi  sp, sp, -8
    sd    a0, 0(sp)
    # 5 argument words go on the stack
    addi  sp, sp, -40
    ld    t1, 72(sp)
    sd    t1, 0(sp)
    ld    t1, 64(sp)
    sd    t1, 8(sp)
    ld    t1, 56(sp)
    sd    t1, 16(sp)
    ld    t1, 48(sp)
    sd    t1, 24(sp)
    ld    t1, 40(sp)
    sd    t1, 32(sp)
    ld    a0, 136(sp)
    ld    a1, 128(sp)
    ld    a2, 120(sp)
    ld    a3, 112(sp)
    ld    a4, 104(sp)
    ld    a5, 96(sp)
    ld    a6, 88(sp)
    ld    a7, 80(sp)
    call  sum
    addi  sp, sp, 144
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 78
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    snez  a0, a0
    beqz  a0, .L.end.22
    li    a0, 3
    j     .L.return.main
.L.end.22:
    ld    a0, -56(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 40
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    mv    a2, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    ld    a0, 0(sp)
    addi  sp, sp, 8
    ld    t1, 0(sp)
    addi  sp, sp, 8
    jalr  t1
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 42
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    snez  a0, a0
    beqz  a0, .L.end.23
    li    a0, 4
    j     .L.return.main
.L.end.23:
    la    a0, .L.str.0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lb    a0, -41(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 5
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, -294967296
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -24
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -40
    mv    t1, a0
    lwu   a5, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a5, a5, t0
    lwu   a6, 8(t1)
    lwu   t0, 12(t1)
    slli  t0, t0, 32
    or    a6, a6, t0
    ld    t1, 0(sp)
    addi  sp, sp, 8
    lwu   a4, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a4, a4, t0
    ld    a3, 0(sp)
    addi  sp, sp, 8
    ld    a2, 0(sp)
    addi  sp, sp, 8
    ld    a1, 0(sp)
    addi  sp, sp, 8
    ld    a0, 0(sp)
    addi  sp, sp, 8
    call  mix
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 59
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    snez  a0, a0
    beqz  a0, .L.end.24
    li    a0, 5
    j     .L.return.main
.L.end.24:
    la    a0, .L.str.1
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 7
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, .L.str.2
    mv    a2, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    ld    a0, 0(sp)
    addi  sp, sp, 8
    call  mix
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 7072
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    snez  a0, a0
    beqz  a0, .L.end.25
    li    a0, 6
    j     .L.return.main
.L.end.25:
    la    a0, .L.str.3
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -24
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -24
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -24
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -24
    mv    t1, a0
    lwu   a4, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a4, a4, t0
    ld    t1, 0(sp)
    addi  sp, sp, 8
    lwu   a3, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a3, a3, t0
    ld    t1, 0(sp)
    addi  sp, sp, 8
    lwu   a2, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a2, a2, t0
    ld    t1, 0(sp)
    addi  sp, sp, 8
    lwu   a1, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a1, a1, t0
    ld    a0, 0(sp)
    addi  sp, sp, 8
    call  mix
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 24
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    snez  a0, a0
    beqz  a0, .L.end.26
    li    a0, 7
    j     .L.return.main
.L.end.26:
    li    a0, 1
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 4
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 5
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 6
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 7
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -24
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 100
    addi  sp, sp, -8
    sd    a0, 0(sp)
    # 1 argument word goes on the stack
    addi  sp, sp, -8
    ld    t1, 8(sp)
    sd    t1, 0(sp)
    ld    a0, 72(sp)
    ld    a1, 64(sp)
    ld    a2, 56(sp)
    ld    a3, 48(sp)
    ld    a4, 40(sp)
    ld    a5, 32(sp)
    ld    a6, 24(sp)
    ld    t1, 16(sp)
    lwu   a7, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a7, a7, t0
    call  late
    addi  sp, sp, 80
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 134
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    snez  a0, a0
    beqz  a0, .L.end.27
    li    a0, 8
    j     .L.return.main
.L.end.27:
    li    a0, 1
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 4
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 5
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 6
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 7
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 8
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 9
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 7
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    addi  sp, sp, -8
    sd    a0, 0(sp)
    # 3 argument words go on the stack
    addi  sp, sp, -24
    ld    t1, 40(sp)
    sd    t1, 0(sp)
    ld    t1, 32(sp)
    sd    t1, 8(sp)
    ld    t1, 24(sp)
    sd    t1, 16(sp)
    ld    a0, 104(sp)
    ld    a1, 96(sp)
    ld    a2, 88(sp)
    ld    a3, 80(sp)
    ld    a4, 72(sp)
    ld    a5, 64(sp)
    ld    a6, 56(sp)
    ld    a7, 48(sp)
    call  beyond
    addi  sp, sp, 112
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 28
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    snez  a0, a0
    beqz  a0, .L.end.28
    li    a0, 9
    j     .L.return.main
.L.end.28:
    li    a0, 4
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 4
    mv    a4, a0
    ld    a3, 0(sp)
    addi  sp, sp, 8
    ld    a2, 0(sp)
    addi  sp, sp, 8
    ld    a1, 0(sp)
    addi  sp, sp, 8
    ld    a0, 0(sp)
    addi  sp, sp, 8
    call  forward
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 10
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    snez  a0, a0
    beqz  a0, .L.end.29
    li    a0, 10
    j     .L.return.main
.L.end.29:
    li    a0, 2
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lhu   a0, -44(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lb    a0, -41(s0)
    mv    a2, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    ld    a0, 0(sp)
    addi  sp, sp, 8
    call  sum
    sw    a0, -60(s0)
    lw    a0, -60(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 65533
    ld    a1, 0(sp)
    addi  sp, sp, 8
    subw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 4
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    mv    a1, a0
    ld    a0, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    call  sum
    addi  sp, sp, 8
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    mv    a2, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    ld    a0, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    call  sum
    addi  sp, sp, 8
    mv    a4, a0
    ld    a3, 0(sp)
    addi  sp, sp, 8
    ld    a2, 0(sp)
    addi  sp, sp, 8
    ld    a1, 0(sp)
    addi  sp, sp, 8
    ld    a0, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    call  sum
    addi  sp, sp, 8
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 70
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

    .section .rodata

.L.str.0:
    .asciz "ciupb"

.L.str.1:
    .asciz "is"

.L.str.2:
    .asciz "A"

.L.str.3:
    .asciz "pppp"
