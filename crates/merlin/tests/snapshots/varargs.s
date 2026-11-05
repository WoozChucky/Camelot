    .file "varargs.c"
    .text

    # int sum(int n, ...): frame 48 bytes
    #     -28(s0)  a1-a7, saved for va_arg
    #     -40(s0)  param int n
    #     -44(s0)  local void *ap
    #     -48(s0)  local int s
    .globl sum
    .type sum, @function
sum:
    addi  sp, sp, -48
    sw    ra, 16(sp)
    sw    s0, 12(sp)
    addi  s0, sp, 48
    # the variadic arguments in registers, next to those on the stack
    sw    a1, -28(s0)
    sw    a2, -24(s0)
    sw    a3, -20(s0)
    sw    a4, -16(s0)
    sw    a5, -12(s0)
    sw    a6, -8(s0)
    sw    a7, -4(s0)
    sw    a0, -40(s0)
    li    a0, 0
    sw    a0, -48(s0)
    addi  a0, s0, -28
    sw    a0, -44(s0)
.L.continue.1:
    lw    a0, -40(s0)
    addi  a0, a0, -1
    sw    a0, -40(s0)
    addi  a0, a0, 1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a0, a1
    beqz  a0, .L.break.2
    lw    a0, -48(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -44(s0)
    mv    t1, a0
    addi  a0, a0, 4
    sw    a0, -44(s0)
    lw    a0, 0(t1)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -48(s0)
    j     .L.continue.1
.L.break.2:
    lw    a0, -44(s0)
    lw    a0, -48(s0)
.L.return.sum:
    addi  sp, s0, -48
    lw    ra, 16(sp)
    lw    s0, 12(sp)
    addi  sp, sp, 48
    ret
    .size sum, . - sum

    # int mix(const char *fmt, ...): frame 80 bytes
    #     -28(s0)  a1-a7, saved for va_arg
    #     -40(s0)  param const char *fmt
    #     -44(s0)  local void *ap
    #     -48(s0)  local void *again
    #     -52(s0)  local int s
    #     -56(s0)  local const char *p
    #     -64(s0)  local struct pair q
    #     -80(s0)  local struct big b
    .globl mix
    .type mix, @function
mix:
    addi  sp, sp, -48
    sw    ra, 16(sp)
    sw    s0, 12(sp)
    addi  s0, sp, 48
    addi  sp, sp, -32
    # the variadic arguments in registers, next to those on the stack
    sw    a1, -28(s0)
    sw    a2, -24(s0)
    sw    a3, -20(s0)
    sw    a4, -16(s0)
    sw    a5, -12(s0)
    sw    a6, -8(s0)
    sw    a7, -4(s0)
    sw    a0, -40(s0)
    li    a0, 0
    sw    a0, -52(s0)
    addi  a0, s0, -28
    sw    a0, -44(s0)
    lw    a0, -44(s0)
    sw    a0, -48(s0)
    lw    a0, -40(s0)
    sw    a0, -56(s0)
.L.loop.3:
    lw    a0, -56(s0)
    lbu   a0, 0(a0)
    beqz  a0, .L.break.5
    lw    a0, -56(s0)
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 105
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.else.6
    lw    a0, -52(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -44(s0)
    mv    t1, a0
    addi  a0, a0, 4
    sw    a0, -44(s0)
    lw    a0, 0(t1)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -52(s0)
    j     .L.end.7
.L.else.6:
    lw    a0, -56(s0)
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 99
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.else.8
    lw    a0, -52(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -44(s0)
    mv    t1, a0
    addi  a0, a0, 4
    sw    a0, -44(s0)
    lw    a0, 0(t1)
    slli  a0, a0, 24
    srai  a0, a0, 24
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -52(s0)
    j     .L.end.9
.L.else.8:
    lw    a0, -56(s0)
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 117
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.else.10
    lw    a0, -52(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -44(s0)
    mv    t1, a0
    addi  a0, a0, 4
    sw    a0, -44(s0)
    lw    a0, 0(t1)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 100
    lw    a1, 0(sp)
    addi  sp, sp, 4
    remu  a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -52(s0)
    j     .L.end.11
.L.else.10:
    lw    a0, -56(s0)
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 115
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.else.12
    lw    a0, -52(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -44(s0)
    mv    t1, a0
    addi  a0, a0, 4
    sw    a0, -44(s0)
    lw    a0, 0(t1)
    lbu   a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -52(s0)
    j     .L.end.13
.L.else.12:
    lw    a0, -56(s0)
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 112
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.else.14
    lw    a0, -44(s0)
    mv    t1, a0
    addi  a0, a0, 8
    sw    a0, -44(s0)
    mv    a0, t1
    addi  a1, s0, -64
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    a0, -52(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -64
    lh    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -64
    addi  a0, a0, 2
    lh    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -64
    addi  a0, a0, 4
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -52(s0)
    j     .L.end.15
.L.else.14:
    lw    a0, -56(s0)
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 98
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.end.16
    lw    a0, -44(s0)
    mv    t1, a0
    addi  a0, a0, 4
    sw    a0, -44(s0)
    lw    a0, 0(t1)
    addi  a1, s0, -80
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    lw    t0, 12(a0)
    sw    t0, 12(a1)
    lw    a0, -52(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -80
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
    addi  a0, s0, -80
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
.L.end.16:
.L.end.15:
.L.end.13:
.L.end.11:
.L.end.9:
.L.end.7:
.L.continue.4:
    lw    a0, -56(s0)
    addi  a0, a0, 1
    sw    a0, -56(s0)
    addi  a0, a0, -1
    j     .L.loop.3
.L.break.5:
    lw    a0, -40(s0)
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 105
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.end.17
    lw    a0, -52(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1000
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -48(s0)
    mv    t1, a0
    addi  a0, a0, 4
    sw    a0, -48(s0)
    lw    a0, 0(t1)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    mul   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -52(s0)
.L.end.17:
    lw    a0, -48(s0)
    lw    a0, -44(s0)
    lw    a0, -52(s0)
.L.return.mix:
    addi  sp, s0, -48
    lw    ra, 16(sp)
    lw    s0, 12(sp)
    addi  sp, sp, 48
    ret
    .size mix, . - mix

    # int late(int a, int b, int c, int d, int e, int f, int g, ...): frame 64 bytes
    #     -4(s0)  a7, saved for va_arg
    #     -16(s0)  param int a
    #     -20(s0)  param int b
    #     -24(s0)  param int c
    #     -28(s0)  param int d
    #     -32(s0)  param int e
    #     -36(s0)  param int f
    #     -40(s0)  param int g
    #     -44(s0)  local void *ap
    #     -52(s0)  local struct pair q
    #     -56(s0)  local int x
    .globl late
    .type late, @function
late:
    addi  sp, sp, -16
    sw    ra, 8(sp)
    sw    s0, 4(sp)
    addi  s0, sp, 16
    addi  sp, sp, -48
    # the variadic arguments in registers, next to those on the stack
    sw    a7, -4(s0)
    sw    a0, -16(s0)
    sw    a1, -20(s0)
    sw    a2, -24(s0)
    sw    a3, -28(s0)
    sw    a4, -32(s0)
    sw    a5, -36(s0)
    sw    a6, -40(s0)
    addi  a0, s0, -4
    sw    a0, -44(s0)
    addi  a0, s0, -52
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -44(s0)
    mv    t1, a0
    addi  a0, a0, 8
    sw    a0, -44(s0)
    mv    a0, t1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    mv    a0, a1
    lw    a0, -44(s0)
    mv    t1, a0
    addi  a0, a0, 4
    sw    a0, -44(s0)
    lw    a0, 0(t1)
    sw    a0, -56(s0)
    lw    a0, -44(s0)
    lw    a0, -16(s0)
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
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -28(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -32(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -36(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -40(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -52
    lh    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -52
    addi  a0, a0, 2
    lh    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -52
    addi  a0, a0, 4
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -56(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
.L.return.late:
    addi  sp, s0, -16
    lw    ra, 8(sp)
    lw    s0, 4(sp)
    addi  sp, sp, 16
    ret
    .size late, . - late

    # int beyond(int a, int b, int c, int d, int e, int f, int g, int h, int i, ...): frame 64 bytes
    #     -12(s0)  param int a
    #     -16(s0)  param int b
    #     -20(s0)  param int c
    #     -24(s0)  param int d
    #     -28(s0)  param int e
    #     -32(s0)  param int f
    #     -36(s0)  param int g
    #     -40(s0)  param int h
    #     -44(s0)  param int i
    #     -48(s0)  local void *ap
    #     -52(s0)  local int s
    .globl beyond
    .type beyond, @function
beyond:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -48
    sw    a0, -12(s0)
    sw    a1, -16(s0)
    sw    a2, -20(s0)
    sw    a3, -24(s0)
    sw    a4, -28(s0)
    sw    a5, -32(s0)
    sw    a6, -36(s0)
    sw    a7, -40(s0)
    lw    t1, 0(s0)
    sw    t1, -44(s0)
    addi  a0, s0, 4
    sw    a0, -48(s0)
    lw    a0, -48(s0)
    mv    t1, a0
    addi  a0, a0, 4
    sw    a0, -48(s0)
    lw    a0, 0(t1)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 10
    lw    a1, 0(sp)
    addi  sp, sp, 4
    mul   a0, a1, a0
    sw    a0, -52(s0)
    lw    a0, -52(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -48(s0)
    mv    t1, a0
    addi  a0, a0, 4
    sw    a0, -48(s0)
    lw    a0, 0(t1)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -52(s0)
    lw    a0, -48(s0)
    lw    a0, -52(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -16(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
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
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -28(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -32(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -36(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -40(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -44(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
.L.return.beyond:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size beyond, . - beyond

    # int vsum(int n, void *ap): frame 32 bytes
    #     -12(s0)  param int n
    #     -16(s0)  param void *ap
    #     -20(s0)  local int s
    .globl vsum
    .type vsum, @function
vsum:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sw    a0, -12(s0)
    sw    a1, -16(s0)
    li    a0, 0
    sw    a0, -20(s0)
.L.continue.18:
    lw    a0, -12(s0)
    addi  a0, a0, -1
    sw    a0, -12(s0)
    addi  a0, a0, 1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a0, a1
    beqz  a0, .L.break.19
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -16(s0)
    mv    t1, a0
    addi  a0, a0, 4
    sw    a0, -16(s0)
    lw    a0, 0(t1)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -20(s0)
    j     .L.continue.18
.L.break.19:
    lw    a0, -20(s0)
.L.return.vsum:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size vsum, . - vsum

    # int forward(int n, ...): frame 48 bytes
    #     -28(s0)  a1-a7, saved for va_arg
    #     -40(s0)  param int n
    #     -44(s0)  local void *ap
    #     -48(s0)  local int s
    .globl forward
    .type forward, @function
forward:
    addi  sp, sp, -48
    sw    ra, 16(sp)
    sw    s0, 12(sp)
    addi  s0, sp, 48
    # the variadic arguments in registers, next to those on the stack
    sw    a1, -28(s0)
    sw    a2, -24(s0)
    sw    a3, -20(s0)
    sw    a4, -16(s0)
    sw    a5, -12(s0)
    sw    a6, -8(s0)
    sw    a7, -4(s0)
    sw    a0, -40(s0)
    addi  a0, s0, -28
    sw    a0, -44(s0)
    lw    a0, -40(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -44(s0)
    mv    a1, a0
    lw    a0, 0(sp)
    addi  sp, sp, 4
    call  vsum
    sw    a0, -48(s0)
    lw    a0, -44(s0)
    lw    a0, -48(s0)
.L.return.forward:
    addi  sp, s0, -48
    lw    ra, 16(sp)
    lw    s0, 12(sp)
    addi  sp, sp, 48
    ret
    .size forward, . - forward

    # int main(void): frame 64 bytes
    #     -16(s0)  local struct pair q
    #     -32(s0)  local struct big b
    #     -33(s0)  local signed char c
    #     -36(s0)  local unsigned short us
    #     -40(s0)  local int (*fp)(int, ...)
    #     -44(s0)  local int r
    #     -60(s0)  temp struct big (argument copy)
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -48
    # struct pair q = ...
    sw    zero, -16(s0)
    sw    zero, -12(s0)
    li    a0, 1
    sh    a0, -16(s0)
    li    a0, 2
    sh    a0, -14(s0)
    li    a0, 3
    sw    a0, -12(s0)
    # struct big b = ...
    sw    zero, -32(s0)
    sw    zero, -28(s0)
    sw    zero, -24(s0)
    sw    zero, -20(s0)
    li    a0, 10
    sw    a0, -32(s0)
    li    a0, 20
    sw    a0, -28(s0)
    li    a0, 30
    sw    a0, -24(s0)
    li    a0, 40
    sw    a0, -20(s0)
    li    a0, -2
    sb    a0, -33(s0)
    li    a0, 65535
    sh    a0, -36(s0)
    la    a0, sum
    sw    a0, -40(s0)
    li    a0, 0
    sw    a0, -44(s0)
    li    a0, 0
    call  sum
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    snez  a0, a0
    beqz  a0, .L.end.20
    li    a0, 1
    j     .L.return.main
.L.end.20:
    li    a0, 3
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    mv    a3, a0
    lw    a2, 0(sp)
    addi  sp, sp, 4
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    a0, 0(sp)
    addi  sp, sp, 4
    call  sum
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 6
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    snez  a0, a0
    beqz  a0, .L.end.21
    li    a0, 2
    j     .L.return.main
.L.end.21:
    li    a0, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 5
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 6
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 7
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 8
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 9
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 10
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 11
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    # 5 argument words go on the stack
    addi  sp, sp, -28
    lw    t1, 44(sp)
    sw    t1, 0(sp)
    lw    t1, 40(sp)
    sw    t1, 4(sp)
    lw    t1, 36(sp)
    sw    t1, 8(sp)
    lw    t1, 32(sp)
    sw    t1, 12(sp)
    lw    t1, 28(sp)
    sw    t1, 16(sp)
    lw    a0, 76(sp)
    lw    a1, 72(sp)
    lw    a2, 68(sp)
    lw    a3, 64(sp)
    lw    a4, 60(sp)
    lw    a5, 56(sp)
    lw    a6, 52(sp)
    lw    a7, 48(sp)
    call  sum
    addi  sp, sp, 80
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 78
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    snez  a0, a0
    beqz  a0, .L.end.22
    li    a0, 3
    j     .L.return.main
.L.end.22:
    lw    a0, -40(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 40
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    mv    a2, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    a0, 0(sp)
    addi  sp, sp, 4
    lw    t1, 0(sp)
    addi  sp, sp, 4
    jalr  t1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 42
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    snez  a0, a0
    beqz  a0, .L.end.23
    li    a0, 4
    j     .L.return.main
.L.end.23:
    la    a0, .L.str
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lb    a0, -33(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 5
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, -294967296
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -16
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -32
    addi  a1, s0, -60
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    lw    t0, 12(a0)
    sw    t0, 12(a1)
    mv    a0, a1
    mv    a6, a0
    lw    t1, 0(sp)
    addi  sp, sp, 4
    lw    a4, 0(t1)
    lw    a5, 4(t1)
    lw    a3, 0(sp)
    addi  sp, sp, 4
    lw    a2, 0(sp)
    addi  sp, sp, 4
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    a0, 0(sp)
    addi  sp, sp, 4
    call  mix
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 59
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    snez  a0, a0
    beqz  a0, .L.end.24
    li    a0, 5
    j     .L.return.main
.L.end.24:
    la    a0, .L.str.1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 7
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, .L.str.2
    mv    a2, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    a0, 0(sp)
    addi  sp, sp, 4
    call  mix
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 7072
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    snez  a0, a0
    beqz  a0, .L.end.25
    li    a0, 6
    j     .L.return.main
.L.end.25:
    la    a0, .L.str.3
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -16
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -16
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -16
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -16
    addi  sp, sp, -4
    sw    a0, 0(sp)
    # 1 argument word goes on the stack
    addi  sp, sp, -12
    lw    t1, 12(sp)
    lw    t2, 4(t1)
    sw    t2, 0(sp)
    lw    a0, 28(sp)
    lw    t1, 24(sp)
    lw    a1, 0(t1)
    lw    a2, 4(t1)
    lw    t1, 20(sp)
    lw    a3, 0(t1)
    lw    a4, 4(t1)
    lw    t1, 16(sp)
    lw    a5, 0(t1)
    lw    a6, 4(t1)
    lw    t1, 12(sp)
    lw    a7, 0(t1)
    call  mix
    addi  sp, sp, 32
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 24
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    snez  a0, a0
    beqz  a0, .L.end.26
    li    a0, 7
    j     .L.return.main
.L.end.26:
    li    a0, 1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 5
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 6
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 7
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -16
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 100
    addi  sp, sp, -4
    sw    a0, 0(sp)
    # 2 argument words go on the stack
    addi  sp, sp, -12
    lw    t1, 16(sp)
    lw    t2, 4(t1)
    sw    t2, 0(sp)
    lw    t1, 12(sp)
    sw    t1, 4(sp)
    lw    a0, 44(sp)
    lw    a1, 40(sp)
    lw    a2, 36(sp)
    lw    a3, 32(sp)
    lw    a4, 28(sp)
    lw    a5, 24(sp)
    lw    a6, 20(sp)
    lw    t1, 16(sp)
    lw    a7, 0(t1)
    call  late
    addi  sp, sp, 48
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 134
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    snez  a0, a0
    beqz  a0, .L.end.27
    li    a0, 8
    j     .L.return.main
.L.end.27:
    li    a0, 1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 5
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 6
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 7
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 8
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 9
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 7
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    addi  sp, sp, -4
    sw    a0, 0(sp)
    # 3 argument words go on the stack
    addi  sp, sp, -20
    lw    t1, 28(sp)
    sw    t1, 0(sp)
    lw    t1, 24(sp)
    sw    t1, 4(sp)
    lw    t1, 20(sp)
    sw    t1, 8(sp)
    lw    a0, 60(sp)
    lw    a1, 56(sp)
    lw    a2, 52(sp)
    lw    a3, 48(sp)
    lw    a4, 44(sp)
    lw    a5, 40(sp)
    lw    a6, 36(sp)
    lw    a7, 32(sp)
    call  beyond
    addi  sp, sp, 64
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 28
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    snez  a0, a0
    beqz  a0, .L.end.28
    li    a0, 9
    j     .L.return.main
.L.end.28:
    li    a0, 4
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    mv    a4, a0
    lw    a3, 0(sp)
    addi  sp, sp, 4
    lw    a2, 0(sp)
    addi  sp, sp, 4
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    a0, 0(sp)
    addi  sp, sp, 4
    call  forward
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 10
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    snez  a0, a0
    beqz  a0, .L.end.29
    li    a0, 10
    j     .L.return.main
.L.end.29:
    li    a0, 2
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lhu   a0, -36(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lb    a0, -33(s0)
    mv    a2, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    a0, 0(sp)
    addi  sp, sp, 4
    call  sum
    sw    a0, -44(s0)
    lw    a0, -44(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 65533
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    mv    a1, a0
    lw    a0, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -4
    call  sum
    addi  sp, sp, 4
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    mv    a2, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    a0, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    call  sum
    addi  sp, sp, 12
    mv    a4, a0
    lw    a3, 0(sp)
    addi  sp, sp, 4
    lw    a2, 0(sp)
    addi  sp, sp, 4
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    a0, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    call  sum
    addi  sp, sp, 12
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 70
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

    .section .rodata

.L.str:
    .asciz "ciupb"

.L.str.1:
    .asciz "is"

.L.str.2:
    .asciz "A"

.L.str.3:
    .asciz "pppp"
