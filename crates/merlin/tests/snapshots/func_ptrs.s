    .text

    # int add(int a, int b): frame 32 bytes
    #     -20(s0)  param int a
    #     -24(s0)  param int b
    .globl add
    .type add, @function
add:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sw    a0, -20(s0)
    sw    a1, -24(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -24(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
.L.return.add:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size add, . - add

    # int sub(int a, int b): frame 32 bytes
    #     -20(s0)  param int a
    #     -24(s0)  param int b
    .globl sub
    .type sub, @function
sub:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sw    a0, -20(s0)
    sw    a1, -24(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -24(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    subw  a0, a1, a0
.L.return.sub:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size sub, . - sub

    # int mul(int a, int b): frame 32 bytes
    #     -20(s0)  param int a
    #     -24(s0)  param int b
    .globl mul
    .type mul, @function
mul:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sw    a0, -20(s0)
    sw    a1, -24(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -24(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    mulw  a0, a1, a0
.L.return.mul:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size mul, . - mul

    .data

    # int (*ops[3])(int, int): 24 bytes
    .globl ops
    .balign 8
    .type ops, @object
    .size ops, 24
ops:
    .quad add
    .quad sub
    .quad mul

    .text

    # int (*pick(int which))(int, int): frame 32 bytes
    #     -20(s0)  param int which
    .globl pick
    .type pick, @function
pick:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sw    a0, -20(s0)
    la    a0, ops
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -20(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 3
    add   a0, a1, a0
    ld    a0, 0(a0)
.L.return.pick:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size pick, . - pick

    # int apply(int (*f)(int, int), int a, int b): frame 32 bytes
    #     -24(s0)  param int (*f)(int, int)
    #     -28(s0)  param int a
    #     -32(s0)  param int b
    .globl apply
    .type apply, @function
apply:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sd    a0, -24(s0)
    sw    a1, -28(s0)
    sw    a2, -32(s0)
    ld    a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -28(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -32(s0)
    mv    a1, a0
    ld    a0, 0(sp)
    addi  sp, sp, 8
    ld    t1, 0(sp)
    addi  sp, sp, 8
    jalr  t1
.L.return.apply:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size apply, . - apply

    # int fold(int (*f)(int, int), const int *a, int n, int acc): frame 48 bytes
    #     -24(s0)  param int (*f)(int, int)
    #     -32(s0)  param const int *a
    #     -36(s0)  param int n
    #     -40(s0)  param int acc
    #     -44(s0)  local int i
    .globl fold
    .type fold, @function
fold:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -32
    sd    a0, -24(s0)
    sd    a1, -32(s0)
    sw    a2, -36(s0)
    sw    a3, -40(s0)
    li    a0, 0
    sw    a0, -44(s0)
.L.loop.1:
    lw    a0, -44(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -36(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slt   a0, a1, a0
    beqz  a0, .L.break.3
    ld    a0, -24(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -40(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -32(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -44(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    mv    a1, a0
    ld    a0, 0(sp)
    addi  sp, sp, 8
    ld    t1, 0(sp)
    addi  sp, sp, 8
    jalr  t1
    sw    a0, -40(s0)
.L.continue.2:
    lw    a0, -44(s0)
    addiw a0, a0, 1
    sw    a0, -44(s0)
    addiw a0, a0, -1
    j     .L.loop.1
.L.break.3:
    lw    a0, -40(s0)
.L.return.fold:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size fold, . - fold

    # int main(void): frame 80 bytes
    #     -20(s0)  local int ok
    #     -32(s0)  local int (*f)(int, int)
    #     -40(s0)  local int (*g)(int)
    #     -56(s0)  local struct vtable vt
    #     -64(s0)  local struct vtable *pv
    #     -80(s0)  local int nums[4]
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -64
    li    a0, 0
    sw    a0, -20(s0)
    la    a0, add
    sd    a0, -32(s0)
    la    a0, twice
    sd    a0, -40(s0)
    # struct vtable vt = ...
    sd    zero, -56(s0)
    sd    zero, -48(s0)
    la    a0, mul
    sd    a0, -56(s0)
    li    a0, 1
    sw    a0, -48(s0)
    addi  a0, s0, -56
    sd    a0, -64(s0)
    # int nums[4] = ...
    sd    zero, -80(s0)
    sd    zero, -72(s0)
    li    a0, 1
    sw    a0, -80(s0)
    li    a0, 2
    sw    a0, -76(s0)
    li    a0, 3
    sw    a0, -72(s0)
    li    a0, 4
    sw    a0, -68(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -32(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    mv    a1, a0
    ld    a0, 0(sp)
    addi  sp, sp, 8
    ld    t1, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    jalr  t1
    addi  sp, sp, 8
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 5
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
    ld    a0, -32(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    mv    a1, a0
    ld    a0, 0(sp)
    addi  sp, sp, 8
    ld    t1, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    jalr  t1
    addi  sp, sp, 8
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 5
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
    la    a0, ops
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 3
    add   a0, a1, a0
    ld    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 10
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 4
    mv    a1, a0
    ld    a0, 0(sp)
    addi  sp, sp, 8
    ld    t1, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    jalr  t1
    addi  sp, sp, 8
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
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, mul
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 6
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 7
    mv    a2, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    ld    a0, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    call  apply
    addi  sp, sp, 8
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 42
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
    li    a0, 2
    addi  sp, sp, -8
    call  pick
    addi  sp, sp, 8
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    mv    a1, a0
    ld    a0, 0(sp)
    addi  sp, sp, 8
    ld    t1, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    jalr  t1
    addi  sp, sp, 8
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 9
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
    ld    a0, -40(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 21
    ld    t1, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    jalr  t1
    addi  sp, sp, 8
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 42
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
    addi  a0, s0, -56
    ld    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 4
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 5
    mv    a1, a0
    ld    a0, 0(sp)
    addi  sp, sp, 8
    ld    t1, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    jalr  t1
    addi  sp, sp, 8
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 20
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
    ld    a0, -64(s0)
    ld    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -64(s0)
    addi  a0, a0, 8
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 9
    mv    a1, a0
    ld    a0, 0(sp)
    addi  sp, sp, 8
    ld    t1, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    jalr  t1
    addi  sp, sp, 8
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 9
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
    ld    a0, -32(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, add
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.4
    ld    a0, -32(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, sub
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    snez  a0, a0
.L.and.4:
    snez  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    la    a0, nothing
    ld    a0, 0(a0)
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
    li    a0, 0
    sd    a0, -32(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -32(s0)
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    addi  sp, sp, -8
    call  pick
    addi  sp, sp, 8
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    call  pick
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 5
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    mv    a1, a0
    ld    a0, 0(sp)
    addi  sp, sp, 8
    ld    t1, 0(sp)
    addi  sp, sp, 8
    jalr  t1
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    mv    a2, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    ld    a0, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    call  apply
    addi  sp, sp, 8
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
    la    a0, mul
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -80
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 4
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    mv    a3, a0
    ld    a2, 0(sp)
    addi  sp, sp, 8
    ld    a1, 0(sp)
    addi  sp, sp, 8
    ld    a0, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    call  fold
    addi  sp, sp, 8
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 24
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.5
    la    a0, ops
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 3
    add   a0, a1, a0
    ld    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -80
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 4
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    mv    a3, a0
    ld    a2, 0(sp)
    addi  sp, sp, 8
    ld    a1, 0(sp)
    addi  sp, sp, 8
    ld    a0, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    call  fold
    addi  sp, sp, 8
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 10
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.5:
    snez  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slt   a0, a0, a1
    beqz  a0, .L.else.6
    la    a0, add
    j     .L.end.7
.L.else.6:
    la    a0, sub
.L.end.7:
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    mv    a1, a0
    ld    a0, 0(sp)
    addi  sp, sp, 8
    ld    t1, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    jalr  t1
    addi  sp, sp, 8
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
    lw    a0, -20(s0)
.L.return.main:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size main, . - main

    # int twice(int x): frame 32 bytes
    #     -20(s0)  param int x
    .type twice, @function
twice:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sw    a0, -20(s0)
    li    a0, 2
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -20(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    mulw  a0, a1, a0
.L.return.twice:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size twice, . - twice

    .bss

    # void (*nothing)(void): 8 bytes
    .globl nothing
    .balign 8
    .type nothing, @object
    .size nothing, 8
nothing:
    .zero 8
