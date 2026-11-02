    .text

    # int classify(int x): frame 16 bytes
    #     -12(s0)  param int x
    #     -16(s0)  local int y
    .globl classify
    .type classify, @function
classify:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    li    t0, 0
    beq   a0, t0, .L.case.classify.0
    li    t0, 1
    beq   a0, t0, .L.case.classify.1
    li    t0, 2
    beq   a0, t0, .L.case.classify.2
    li    t0, -5
    beq   a0, t0, .L.case.classify.3
    li    t0, 100
    beq   a0, t0, .L.case.classify.4
    j     .L.case.classify.5
.L.case.classify.0:
    li    a0, 10
    j     .L.return.classify
.L.case.classify.1:
.L.case.classify.2:
    li    a0, 20
    j     .L.return.classify
.L.case.classify.3:
    li    a0, 30
    j     .L.return.classify
.L.case.classify.4:
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 10
    lw    a1, 0(sp)
    addi  sp, sp, 4
    div   a0, a1, a0
    sw    a0, -16(s0)
    lw    a0, -16(s0)
    j     .L.return.classify
.L.case.classify.5:
    li    a0, 1
    neg   a0, a0
    j     .L.return.classify
.L.break.1:
    li    a0, 0
.L.return.classify:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size classify, . - classify

    # int fall(int x): frame 16 bytes
    #     -12(s0)  param int x
    #     -16(s0)  local int r
    .globl fall
    .type fall, @function
fall:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    sw    a0, -12(s0)
    li    a0, 0
    sw    a0, -16(s0)
    lw    a0, -12(s0)
    li    t0, 1
    beq   a0, t0, .L.case.fall.0
    li    t0, 2
    beq   a0, t0, .L.case.fall.1
    li    t0, 3
    beq   a0, t0, .L.case.fall.2
    li    t0, 4
    beq   a0, t0, .L.case.fall.3
    j     .L.break.2
.L.case.fall.0:
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -16(s0)
.L.case.fall.1:
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -16(s0)
.L.case.fall.2:
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -16(s0)
    j     .L.break.2
.L.case.fall.3:
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -16(s0)
.L.break.2:
    lw    a0, -16(s0)
.L.return.fall:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size fall, . - fall

    # int nested(int a, int b): frame 16 bytes
    #     -12(s0)  param int a
    #     -16(s0)  param int b
    .globl nested
    .type nested, @function
nested:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    sw    a0, -12(s0)
    sw    a1, -16(s0)
    lw    a0, -12(s0)
    li    t0, 1
    beq   a0, t0, .L.case.nested.0
    li    t0, 2
    beq   a0, t0, .L.case.nested.4
    j     .L.break.3
.L.case.nested.0:
    lw    a0, -16(s0)
    li    t0, 1
    beq   a0, t0, .L.case.nested.1
    li    t0, 2
    beq   a0, t0, .L.case.nested.2
    j     .L.case.nested.3
.L.case.nested.1:
    li    a0, 11
    j     .L.return.nested
.L.case.nested.2:
    j     .L.break.4
.L.case.nested.3:
    li    a0, 19
    j     .L.return.nested
.L.break.4:
    li    a0, 12
    j     .L.return.nested
.L.case.nested.4:
.L.continue.5:
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 10
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a1, a0
    beqz  a0, .L.break.6
    lw    a0, -16(s0)
    addi  a0, a0, 1
    sw    a0, -16(s0)
    addi  a0, a0, -1
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 5
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.end.7
    j     .L.break.6
.L.end.7:
    j     .L.continue.5
.L.break.6:
    lw    a0, -16(s0)
    j     .L.return.nested
.L.break.3:
    li    a0, 0
.L.return.nested:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size nested, . - nested

    # int loop_continue(void): frame 16 bytes
    #     -12(s0)  local int i
    #     -16(s0)  local int s
    .globl loop_continue
    .type loop_continue, @function
loop_continue:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    li    a0, 0
    sw    a0, -16(s0)
    li    a0, 0
    sw    a0, -12(s0)
.L.loop.8:
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 6
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a1, a0
    beqz  a0, .L.break.10
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    lw    a1, 0(sp)
    addi  sp, sp, 4
    rem   a0, a1, a0
    li    t0, 0
    beq   a0, t0, .L.case.loop_continue.0
    li    t0, 1
    beq   a0, t0, .L.case.loop_continue.1
    j     .L.case.loop_continue.2
.L.case.loop_continue.0:
    j     .L.continue.9
.L.case.loop_continue.1:
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -16(s0)
    j     .L.break.11
.L.case.loop_continue.2:
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 10
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -16(s0)
.L.break.11:
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 100
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -16(s0)
.L.continue.9:
    lw    a0, -12(s0)
    addi  a0, a0, 1
    sw    a0, -12(s0)
    addi  a0, a0, -1
    j     .L.loop.8
.L.break.10:
    lw    a0, -16(s0)
.L.return.loop_continue:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size loop_continue, . - loop_continue

    # void copy(char *to, const char *from, int count): frame 32 bytes
    #     -12(s0)  param char *to
    #     -16(s0)  param const char *from
    #     -20(s0)  param int count
    #     -24(s0)  local int n
    .globl copy
    .type copy, @function
copy:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sw    a0, -12(s0)
    sw    a1, -16(s0)
    sw    a2, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    lw    a1, 0(sp)
    addi  sp, sp, 4
    div   a0, a1, a0
    sw    a0, -24(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    lw    a1, 0(sp)
    addi  sp, sp, 4
    rem   a0, a1, a0
    li    t0, 0
    beq   a0, t0, .L.case.copy.0
    li    t0, 3
    beq   a0, t0, .L.case.copy.1
    li    t0, 2
    beq   a0, t0, .L.case.copy.2
    li    t0, 1
    beq   a0, t0, .L.case.copy.3
    j     .L.break.12
.L.case.copy.0:
.L.do.13:
    lw    a0, -12(s0)
    addi  a0, a0, 1
    sw    a0, -12(s0)
    addi  a0, a0, -1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -16(s0)
    addi  a0, a0, 1
    sw    a0, -16(s0)
    addi  a0, a0, -1
    lbu   a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sb    a0, 0(a1)
.L.case.copy.1:
    lw    a0, -12(s0)
    addi  a0, a0, 1
    sw    a0, -12(s0)
    addi  a0, a0, -1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -16(s0)
    addi  a0, a0, 1
    sw    a0, -16(s0)
    addi  a0, a0, -1
    lbu   a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sb    a0, 0(a1)
.L.case.copy.2:
    lw    a0, -12(s0)
    addi  a0, a0, 1
    sw    a0, -12(s0)
    addi  a0, a0, -1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -16(s0)
    addi  a0, a0, 1
    sw    a0, -16(s0)
    addi  a0, a0, -1
    lbu   a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sb    a0, 0(a1)
.L.case.copy.3:
    lw    a0, -12(s0)
    addi  a0, a0, 1
    sw    a0, -12(s0)
    addi  a0, a0, -1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -16(s0)
    addi  a0, a0, 1
    sw    a0, -16(s0)
    addi  a0, a0, -1
    lbu   a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sb    a0, 0(a1)
.L.continue.14:
    lw    a0, -24(s0)
    addi  a0, a0, -1
    sw    a0, -24(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a0, a1
    bnez  a0, .L.do.13
.L.break.15:
.L.break.12:
    li    a0, 0
.L.return.copy:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size copy, . - copy

    # int kind(unsigned int u): frame 16 bytes
    #     -12(s0)  param unsigned int u
    .globl kind
    .type kind, @function
kind:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    li    t0, -1
    beq   a0, t0, .L.case.kind.0
    li    t0, 7
    beq   a0, t0, .L.case.kind.1
    j     .L.break.16
.L.case.kind.0:
    li    a0, 1
    j     .L.return.kind
.L.case.kind.1:
    li    a0, 2
    j     .L.return.kind
.L.break.16:
    li    a0, 3
.L.return.kind:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size kind, . - kind

    # int letter(char c): frame 16 bytes
    #      -9(s0)  param char c
    .globl letter
    .type letter, @function
letter:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    sb    a0, -9(s0)
    lbu   a0, -9(s0)
    li    t0, 97
    beq   a0, t0, .L.case.letter.0
    li    t0, 101
    beq   a0, t0, .L.case.letter.1
    li    t0, 200
    beq   a0, t0, .L.case.letter.2
    j     .L.break.17
.L.case.letter.0:
.L.case.letter.1:
    li    a0, 1
    j     .L.return.letter
.L.case.letter.2:
    li    a0, 2
    j     .L.return.letter
.L.break.17:
    li    a0, 0
.L.return.letter:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size letter, . - letter

    # int main(void): frame 32 bytes
    #     -12(s0)  local int ok
    #     -20(s0)  local char dst[8]
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
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    addi  sp, sp, -12
    call  classify
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 10
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.19
    li    a0, 2
    addi  sp, sp, -12
    call  classify
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 20
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.19:
    snez  a0, a0
    beqz  a0, .L.and.18
    li    a0, 5
    neg   a0, a0
    addi  sp, sp, -12
    call  classify
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 30
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.18:
    snez  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 100
    addi  sp, sp, -12
    call  classify
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 10
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.20
    li    a0, 7
    addi  sp, sp, -12
    call  classify
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    neg   a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.20:
    snez  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -12
    call  fall
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 6
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -12
    call  fall
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 5
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    addi  sp, sp, -12
    call  fall
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.21
    li    a0, 9
    addi  sp, sp, -12
    call  fall
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.21:
    snez  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    mv    a1, a0
    lw    a0, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    call  nested
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 11
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    mv    a1, a0
    lw    a0, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    call  nested
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 12
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    mv    a1, a0
    lw    a0, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    call  nested
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 19
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    mv    a1, a0
    lw    a0, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    call  nested
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 5
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    mv    a1, a0
    lw    a0, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    call  nested
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  sp, sp, -12
    call  loop_continue
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 422
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    addi  a0, s0, -20
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, .L.str.0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 7
    mv    a2, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    a0, 0(sp)
    addi  sp, sp, 4
    call  copy
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -20
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 97
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.22
    addi  a0, s0, -20
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 6
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 103
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.22:
    snez  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    addi  a0, s0, -20
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, .L.str.1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    mv    a2, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    a0, 0(sp)
    addi  sp, sp, 4
    call  copy
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -20
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 121
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.23
    addi  a0, s0, -20
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 99
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.23:
    snez  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    neg   a0, a0
    addi  sp, sp, -12
    call  kind
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.25
    li    a0, 7
    addi  sp, sp, -12
    call  kind
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.25:
    snez  a0, a0
    beqz  a0, .L.and.24
    li    a0, 8
    addi  sp, sp, -12
    call  kind
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.24:
    snez  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 101
    addi  sp, sp, -12
    call  letter
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.27
    li    a0, 200
    addi  sp, sp, -12
    call  letter
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.27:
    snez  a0, a0
    beqz  a0, .L.and.26
    li    a0, 122
    addi  sp, sp, -12
    call  letter
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.26:
    snez  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    j     .L.case.main.0
.L.case.main.0:
    lw    a0, -12(s0)
    addi  a0, a0, 1
    sw    a0, -12(s0)
    addi  a0, a0, -1
.L.break.28:
    lw    a0, -12(s0)
.L.return.main:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size main, . - main

    .section .rodata

.L.str.0:
    .asciz "abcdefg"

.L.str.1:
    .asciz "xyz"
