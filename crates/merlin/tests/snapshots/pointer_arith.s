    .text

    # int main(void): frame 224 bytes
    #     -24(s0)  local char c[8]
    #     -40(s0)  local short s[8]
    #     -72(s0)  local int i[8]
    #    -136(s0)  local long l[8]
    #    -144(s0)  local char *pc
    #    -152(s0)  local short *ps
    #    -160(s0)  local int *pi
    #    -168(s0)  local long *pl
    #    -172(s0)  local int bytes
    #    -176(s0)  local int k
    #    -184(s0)  local int *q
    #    -188(s0)  local int sum
    #    -200(s0)  local int *end
    #    -204(s0)  local int n
    #    -208(s0)  local int before
    #    -212(s0)  local int after
    #    -216(s0)  local int here
    #    -220(s0)  local int order
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -208
    addi  a0, s0, -24
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    sd    a0, -144(s0)
    addi  a0, s0, -40
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 1
    add   a0, a1, a0
    sd    a0, -152(s0)
    addi  a0, s0, -72
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 2
    add   a0, a1, a0
    sd    a0, -160(s0)
    addi  a0, s0, -136
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 3
    add   a0, a1, a0
    sd    a0, -168(s0)
    ld    a0, -144(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -24
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sub   a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -152(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -40
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sub   a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -160(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -72
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sub   a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -168(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -136
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sub   a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    add   a0, a1, a0
    sext.w a0, a0
    sw    a0, -172(s0)
    li    a0, 0
    sw    a0, -176(s0)
.L.loop.1:
    lw    a0, -176(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 8
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slt   a0, a1, a0
    beqz  a0, .L.break.3
    addi  a0, s0, -72
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -176(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 2
    add   a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -176(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -176(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    mulw  a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sw    a0, 0(a1)
.L.continue.2:
    lw    a0, -176(s0)
    addiw a0, a0, 1
    sw    a0, -176(s0)
    addiw a0, a0, -1
    j     .L.loop.1
.L.break.3:
    addi  a0, s0, -72
    sd    a0, -184(s0)
    ld    a0, -184(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 2
    add   a0, a1, a0
    sd    a0, -184(s0)
    ld    a0, -184(s0)
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -184(s0)
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
    ld    a0, -184(s0)
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
    li    a0, 3
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -184(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a1, a1, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -184(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a1, a1, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -188(s0)
    addi  a0, s0, -72
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 8
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 2
    add   a0, a1, a0
    sd    a0, -200(s0)
    ld    a0, -200(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -184(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sub   a0, a1, a0
    srai  a0, a0, 2
    sext.w a0, a0
    sw    a0, -204(s0)
    ld    a0, -184(s0)
    addi  a0, a0, -4
    sd    a0, -184(s0)
    addi  a0, a0, 4
    ld    a0, -184(s0)
    addi  a0, a0, -4
    sd    a0, -184(s0)
    lw    a0, 0(a0)
    sw    a0, -208(s0)
    ld    a0, -184(s0)
    addi  a0, a0, 4
    sd    a0, -184(s0)
    addi  a0, a0, -4
    lw    a0, 0(a0)
    sw    a0, -212(s0)
    ld    a0, -184(s0)
    lw    a0, 0(a0)
    sw    a0, -216(s0)
    ld    a0, -184(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -200(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sltu  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -200(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -184(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sltu  a0, a0, a1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -184(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -184(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sltu  a0, a0, a1
    xori  a0, a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -200(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -200(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sltu  a0, a1, a0
    xori  a0, a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -184(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -200(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sltu  a0, a0, a1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -220(s0)
    lw    a0, -172(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -188(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -204(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -208(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -212(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -216(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -220(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -144(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -24
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sub   a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 4
    ld    a1, 0(sp)
    addi  sp, sp, 8
    mul   a0, a1, a0
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
