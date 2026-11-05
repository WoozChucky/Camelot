    .text

    # int add(int a, int b): frame 16 bytes
    #     -12(s0)  param int a
    #     -16(s0)  param int b
    .globl add
    .type add, @function
add:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    sw    a0, -12(s0)
    sw    a1, -16(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -16(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
.L.return.add:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size add, . - add

    # int sub(int a, int b): frame 16 bytes
    #     -12(s0)  param int a
    #     -16(s0)  param int b
    .globl sub
    .type sub, @function
sub:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    sw    a0, -12(s0)
    sw    a1, -16(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -16(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
.L.return.sub:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size sub, . - sub

    # int mul(int a, int b): frame 16 bytes
    #     -12(s0)  param int a
    #     -16(s0)  param int b
    .globl mul
    .type mul, @function
mul:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    sw    a0, -12(s0)
    sw    a1, -16(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -16(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    mul   a0, a1, a0
.L.return.mul:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size mul, . - mul

    .data

    # int (*ops[3])(int, int): 12 bytes
    .globl ops
    .balign 4
    .type ops, @object
    .size ops, 12
ops:
    .word add
    .word sub
    .word mul

    .text

    # int (*pick(int which))(int, int): frame 16 bytes
    #     -12(s0)  param int which
    .globl pick
    .type pick, @function
pick:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    sw    a0, -12(s0)
    la    a0, ops
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -12(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
.L.return.pick:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size pick, . - pick

    # int apply(int (*f)(int, int), int a, int b): frame 32 bytes
    #     -12(s0)  param int (*f)(int, int)
    #     -16(s0)  param int a
    #     -20(s0)  param int b
    .globl apply
    .type apply, @function
apply:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sw    a0, -12(s0)
    sw    a1, -16(s0)
    sw    a2, -20(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -20(s0)
    mv    a1, a0
    lw    a0, 0(sp)
    addi  sp, sp, 4
    lw    t1, 0(sp)
    addi  sp, sp, 4
    jalr  t1
.L.return.apply:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size apply, . - apply

    # int fold(int (*f)(int, int), const int *a, int n, int acc): frame 32 bytes
    #     -12(s0)  param int (*f)(int, int)
    #     -16(s0)  param const int *a
    #     -20(s0)  param int n
    #     -24(s0)  param int acc
    #     -28(s0)  local int i
    .globl fold
    .type fold, @function
fold:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sw    a0, -12(s0)
    sw    a1, -16(s0)
    sw    a2, -20(s0)
    sw    a3, -24(s0)
    li    a0, 0
    sw    a0, -28(s0)
.L.loop.1:
    lw    a0, -28(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -20(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a1, a0
    beqz  a0, .L.break.3
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -24(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -28(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    mv    a1, a0
    lw    a0, 0(sp)
    addi  sp, sp, 4
    lw    t1, 0(sp)
    addi  sp, sp, 4
    jalr  t1
    sw    a0, -24(s0)
.L.continue.2:
    lw    a0, -28(s0)
    addi  a0, a0, 1
    sw    a0, -28(s0)
    addi  a0, a0, -1
    j     .L.loop.1
.L.break.3:
    lw    a0, -24(s0)
.L.return.fold:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size fold, . - fold

    # int main(void): frame 48 bytes
    #     -12(s0)  local int ok
    #     -16(s0)  local int (*f)(int, int)
    #     -20(s0)  local int (*g)(int)
    #     -28(s0)  local struct vtable vt
    #     -32(s0)  local struct vtable *pv
    #     -48(s0)  local int nums[4]
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -32
    li    a0, 0
    sw    a0, -12(s0)
    la    a0, add
    sw    a0, -16(s0)
    la    a0, twice
    sw    a0, -20(s0)
    # struct vtable vt = ...
    sw    zero, -28(s0)
    sw    zero, -24(s0)
    la    a0, mul
    sw    a0, -28(s0)
    li    a0, 1
    sw    a0, -24(s0)
    addi  a0, s0, -28
    sw    a0, -32(s0)
    # int nums[4] = ...
    sw    zero, -48(s0)
    sw    zero, -44(s0)
    sw    zero, -40(s0)
    sw    zero, -36(s0)
    li    a0, 1
    sw    a0, -48(s0)
    li    a0, 2
    sw    a0, -44(s0)
    li    a0, 3
    sw    a0, -40(s0)
    li    a0, 4
    sw    a0, -36(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    mv    a1, a0
    lw    a0, 0(sp)
    addi  sp, sp, 4
    lw    t1, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    jalr  t1
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
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    mv    a1, a0
    lw    a0, 0(sp)
    addi  sp, sp, 4
    lw    t1, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    jalr  t1
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
    la    a0, ops
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 10
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    mv    a1, a0
    lw    a0, 0(sp)
    addi  sp, sp, 4
    lw    t1, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    jalr  t1
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
    la    a0, mul
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 6
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 7
    mv    a2, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    a0, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    call  apply
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 42
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
    call  pick
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    mv    a1, a0
    lw    a0, 0(sp)
    addi  sp, sp, 4
    lw    t1, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    jalr  t1
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 9
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
    lw    a0, -20(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 21
    lw    t1, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    jalr  t1
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 42
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
    addi  a0, s0, -28
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 5
    mv    a1, a0
    lw    a0, 0(sp)
    addi  sp, sp, 4
    lw    t1, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    jalr  t1
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 20
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
    lw    a0, -32(s0)
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -32(s0)
    addi  a0, a0, 4
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 9
    mv    a1, a0
    lw    a0, 0(sp)
    addi  sp, sp, 4
    lw    t1, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    jalr  t1
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 9
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
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, add
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.4
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, sub
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    snez  a0, a0
.L.and.4:
    snez  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, nothing
    lw    a0, 0(a0)
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
    li    a0, 0
    sw    a0, -16(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -16(s0)
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    addi  sp, sp, -12
    call  pick
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -8
    call  pick
    addi  sp, sp, 8
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 5
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    mv    a1, a0
    lw    a0, 0(sp)
    addi  sp, sp, 4
    lw    t1, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -8
    jalr  t1
    addi  sp, sp, 8
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    mv    a2, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    a0, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    call  apply
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
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
    la    a0, mul
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -48
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    mv    a3, a0
    lw    a2, 0(sp)
    addi  sp, sp, 4
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    a0, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    call  fold
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 24
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.5
    la    a0, ops
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
    addi  a0, s0, -48
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    mv    a3, a0
    lw    a2, 0(sp)
    addi  sp, sp, 4
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    a0, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    call  fold
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 10
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.5:
    snez  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a0, a1
    beqz  a0, .L.else.6
    la    a0, add
    j     .L.end.7
.L.else.6:
    la    a0, sub
.L.end.7:
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    mv    a1, a0
    lw    a0, 0(sp)
    addi  sp, sp, 4
    lw    t1, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    jalr  t1
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
.L.return.main:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size main, . - main

    # int twice(int x): frame 16 bytes
    #     -12(s0)  param int x
    .type twice, @function
twice:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    sw    a0, -12(s0)
    li    a0, 2
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -12(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    mul   a0, a1, a0
.L.return.twice:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size twice, . - twice

    .bss

    # void (*nothing)(void): 4 bytes
    .globl nothing
    .balign 4
    .type nothing, @object
    .size nothing, 4
nothing:
    .zero 4
