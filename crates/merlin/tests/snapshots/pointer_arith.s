    .text

    # int main(void): frame 160 bytes
    #     -16(s0)  local char c[8]
    #     -32(s0)  local short s[8]
    #     -64(s0)  local int i[8]
    #     -96(s0)  local long l[8]
    #    -100(s0)  local char *pc
    #    -104(s0)  local short *ps
    #    -108(s0)  local int *pi
    #    -112(s0)  local long *pl
    #    -116(s0)  local int bytes
    #    -120(s0)  local int k
    #    -124(s0)  local int *q
    #    -128(s0)  local int sum
    #    -132(s0)  local int *end
    #    -136(s0)  local int n
    #    -140(s0)  local int before
    #    -144(s0)  local int after
    #    -148(s0)  local int here
    #    -152(s0)  local int order
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -144
    addi  a0, s0, -16
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -100(s0)
    addi  a0, s0, -32
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 1
    add   a0, a1, a0
    sw    a0, -104(s0)
    addi  a0, s0, -64
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    sw    a0, -108(s0)
    addi  a0, s0, -96
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    sw    a0, -112(s0)
    lw    a0, -100(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -16
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -104(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -32
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -108(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -64
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -112(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -96
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -116(s0)
    li    a0, 0
    sw    a0, -120(s0)
.L.loop.1:
    lw    a0, -120(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 8
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a1, a0
    beqz  a0, .L.break.3
    addi  a0, s0, -64
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -120(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -120(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -120(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    mul   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sw    a0, 0(a1)
.L.continue.2:
    lw    a0, -120(s0)
    addi  a0, a0, 1
    sw    a0, -120(s0)
    addi  a0, a0, -1
    j     .L.loop.1
.L.break.3:
    addi  a0, s0, -64
    sw    a0, -124(s0)
    lw    a0, -124(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    sw    a0, -124(s0)
    lw    a0, -124(s0)
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -124(s0)
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
    lw    a0, -124(s0)
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
    li    a0, 3
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -124(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a1, a1, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -124(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a1, a1, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -128(s0)
    addi  a0, s0, -64
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 8
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    sw    a0, -132(s0)
    lw    a0, -132(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -124(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
    srai  a0, a0, 2
    sw    a0, -136(s0)
    lw    a0, -124(s0)
    addi  a0, a0, -4
    sw    a0, -124(s0)
    addi  a0, a0, 4
    lw    a0, -124(s0)
    addi  a0, a0, -4
    sw    a0, -124(s0)
    lw    a0, 0(a0)
    sw    a0, -140(s0)
    lw    a0, -124(s0)
    addi  a0, a0, 4
    sw    a0, -124(s0)
    addi  a0, a0, -4
    lw    a0, 0(a0)
    sw    a0, -144(s0)
    lw    a0, -124(s0)
    lw    a0, 0(a0)
    sw    a0, -148(s0)
    lw    a0, -124(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -132(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sltu  a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -132(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -124(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sltu  a0, a0, a1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -124(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -124(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sltu  a0, a0, a1
    xori  a0, a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -132(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -132(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sltu  a0, a1, a0
    xori  a0, a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -124(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -132(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sltu  a0, a0, a1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -152(s0)
    lw    a0, -116(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -128(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -136(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -140(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -144(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -148(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -152(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -100(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -16
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    lw    a1, 0(sp)
    addi  sp, sp, 4
    mul   a0, a1, a0
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
