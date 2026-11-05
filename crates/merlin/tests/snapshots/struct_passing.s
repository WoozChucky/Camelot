    .text

    # struct one mk_one(char c): frame 16 bytes
    #      -9(s0)  param char c
    #     -16(s0)  local struct one o
    .globl mk_one
    .type mk_one, @function
mk_one:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    sb    a0, -9(s0)
    addi  a0, s0, -16
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lbu   a0, -9(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sb    a0, 0(a1)
    addi  a0, s0, -16
    mv    t1, a0
    lbu   a0, 0(t1)
.L.return.mk_one:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size mk_one, . - mk_one

    # struct six mk_six(short a, short b, short c): frame 32 bytes
    #     -10(s0)  param short a
    #     -12(s0)  param short b
    #     -14(s0)  param short c
    #     -24(s0)  local struct six s
    .globl mk_six
    .type mk_six, @function
mk_six:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sh    a0, -10(s0)
    sh    a1, -12(s0)
    sh    a2, -14(s0)
    addi  a0, s0, -24
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lh    a0, -10(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sh    a0, 0(a1)
    addi  a0, s0, -24
    addi  a0, a0, 2
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lh    a0, -12(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sh    a0, 0(a1)
    addi  a0, s0, -24
    addi  a0, a0, 4
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lh    a0, -14(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sh    a0, 0(a1)
    addi  a0, s0, -24
    mv    t1, a0
    lhu   a0, 0(t1)
    lhu   t0, 2(t1)
    slli  t0, t0, 16
    or    a0, a0, t0
    lhu   a1, 4(t1)
.L.return.mk_six:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size mk_six, . - mk_six

    # struct pair swap(struct pair p): frame 32 bytes
    #     -16(s0)  param struct pair p
    #     -20(s0)  local int t
    .globl swap
    .type swap, @function
swap:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sw    a0, -16(s0)
    sw    a1, -12(s0)
    addi  a0, s0, -16
    lw    a0, 0(a0)
    sw    a0, -20(s0)
    addi  a0, s0, -16
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -16
    addi  a0, a0, 4
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sw    a0, 0(a1)
    addi  a0, s0, -16
    addi  a0, a0, 4
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -20(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sw    a0, 0(a1)
    addi  a0, s0, -16
    mv    t1, a0
    lw    a0, 0(t1)
    lw    a1, 4(t1)
.L.return.swap:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size swap, . - swap

    # struct big add_big(struct big l, struct big r): frame 48 bytes
    #     -20(s0)  param struct big l
    #     -32(s0)  param struct big r
    #     -36(s0)  temp struct big * (result address)
    #     -48(s0)  local struct big s
    .globl add_big
    .type add_big, @function
add_big:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -32
    sw    a0, -36(s0)
    addi  t4, s0, -20
    lw    t0, 0(a1)
    sw    t0, 0(t4)
    lw    t0, 4(a1)
    sw    t0, 4(t4)
    lw    t0, 8(a1)
    sw    t0, 8(t4)
    addi  t4, s0, -32
    lw    t0, 0(a2)
    sw    t0, 0(t4)
    lw    t0, 4(a2)
    sw    t0, 4(t4)
    lw    t0, 8(a2)
    sw    t0, 8(t4)
    addi  a0, s0, -48
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -20
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -32
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sw    a0, 0(a1)
    addi  a0, s0, -48
    addi  a0, a0, 4
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -20
    addi  a0, a0, 4
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -32
    addi  a0, a0, 4
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sw    a0, 0(a1)
    addi  a0, s0, -48
    addi  a0, a0, 8
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -20
    addi  a0, a0, 8
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -32
    addi  a0, a0, 8
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sw    a0, 0(a1)
    addi  a0, s0, -48
    lw    a1, -36(s0)
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
.L.return.add_big:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size add_big, . - add_big

    # int sum_six(struct six s): frame 16 bytes
    #     -16(s0)  param struct six s
    .globl sum_six
    .type sum_six, @function
sum_six:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    sw    a0, -16(s0)
    sw    a1, -12(s0)
    addi  a0, s0, -16
    lh    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -16
    addi  a0, a0, 2
    lh    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -16
    addi  a0, a0, 4
    lh    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
.L.return.sum_six:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size sum_six, . - sum_six

    # void clobber(struct big b): frame 32 bytes
    #     -20(s0)  param struct big b
    .globl clobber
    .type clobber, @function
clobber:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    addi  t4, s0, -20
    lw    t0, 0(a0)
    sw    t0, 0(t4)
    lw    t0, 4(a0)
    sw    t0, 4(t4)
    lw    t0, 8(a0)
    sw    t0, 8(t4)
    addi  a0, s0, -20
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 99
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sw    a0, 0(a1)
    addi  a0, s0, -20
    addi  a0, a0, 4
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 99
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sw    a0, 0(a1)
    li    a0, 0
.L.return.clobber:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size clobber, . - clobber

    # int sum_huge(struct huge h): frame 96 bytes
    #     -88(s0)  param struct huge h
    #     -92(s0)  local int i
    #     -96(s0)  local int s
    .globl sum_huge
    .type sum_huge, @function
sum_huge:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -80
    addi  t4, s0, -88
    mv    t1, a0
    mv    t2, t4
    li    t3, 80
    add   t3, t1, t3
2:
    lw    t0, 0(t1)
    sw    t0, 0(t2)
    addi  t1, t1, 4
    addi  t2, t2, 4
    bltu  t1, t3, 2b
    li    a0, 0
    sw    a0, -96(s0)
    li    a0, 0
    sw    a0, -92(s0)
.L.loop.1:
    lw    a0, -92(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 20
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a1, a0
    beqz  a0, .L.break.3
    lw    a0, -96(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -88
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -92(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -96(s0)
.L.continue.2:
    lw    a0, -92(s0)
    addi  a0, a0, 1
    sw    a0, -92(s0)
    addi  a0, a0, -1
    j     .L.loop.1
.L.break.3:
    addi  a0, s0, -88
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1000
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sw    a0, 0(a1)
    lw    a0, -96(s0)
.L.return.sum_huge:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size sum_huge, . - sum_huge

    # struct huge make_huge(int base): frame 112 bytes
    #     -12(s0)  param int base
    #     -16(s0)  temp struct huge * (result address)
    #     -96(s0)  local struct huge h
    #    -100(s0)  local int i
    .globl make_huge
    .type make_huge, @function
make_huge:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -96
    sw    a0, -16(s0)
    sw    a1, -12(s0)
    li    a0, 0
    sw    a0, -100(s0)
.L.loop.4:
    lw    a0, -100(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 20
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a1, a0
    beqz  a0, .L.break.6
    addi  a0, s0, -96
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -100(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -100(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sw    a0, 0(a1)
.L.continue.5:
    lw    a0, -100(s0)
    addi  a0, a0, 1
    sw    a0, -100(s0)
    addi  a0, a0, -1
    j     .L.loop.4
.L.break.6:
    addi  a0, s0, -96
    lw    a1, -16(s0)
    mv    t1, a0
    mv    t2, a1
    li    t3, 80
    add   t3, t1, t3
2:
    lw    t0, 0(t1)
    sw    t0, 0(t2)
    addi  t1, t1, 4
    addi  t2, t2, 4
    bltu  t1, t3, 2b
.L.return.make_huge:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size make_huge, . - make_huge

    # int mixed(int a, struct six s, int b, struct big c, char d): frame 48 bytes
    #     -12(s0)  param int a
    #     -20(s0)  param struct six s
    #     -24(s0)  param int b
    #     -36(s0)  param struct big c
    #     -37(s0)  param char d
    .globl mixed
    .type mixed, @function
mixed:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -32
    sw    a0, -12(s0)
    sw    a1, -20(s0)
    sw    a2, -16(s0)
    sw    a3, -24(s0)
    addi  t4, s0, -36
    lw    t0, 0(a4)
    sw    t0, 0(t4)
    lw    t0, 4(a4)
    sw    t0, 4(t4)
    lw    t0, 8(a4)
    sw    t0, 8(t4)
    sb    a5, -37(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -20
    mv    t1, a0
    lhu   a0, 0(t1)
    lhu   t0, 2(t1)
    slli  t0, t0, 16
    or    a0, a0, t0
    lhu   a1, 4(t1)
    addi  sp, sp, -12
    call  sum_six
    addi  sp, sp, 12
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
    addi  a0, s0, -36
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -36
    addi  a0, a0, 4
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -36
    addi  a0, a0, 8
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lbu   a0, -37(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
.L.return.mixed:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size mixed, . - mixed

    # struct word id_word(struct word w): frame 16 bytes
    #     -12(s0)  param struct word w
    .globl id_word
    .type id_word, @function
id_word:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    sw    a0, -12(s0)
    addi  a0, s0, -12
    mv    t1, a0
    lhu   a0, 0(t1)
    lhu   t0, 2(t1)
    slli  t0, t0, 16
    or    a0, a0, t0
.L.return.id_word:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size id_word, . - id_word

    # int full(struct six s, struct pair p, struct big b, int x, int y, int z): frame 48 bytes
    #     -16(s0)  param struct six s
    #     -24(s0)  param struct pair p
    #     -36(s0)  param struct big b
    #     -40(s0)  param int x
    #     -44(s0)  param int y
    #     -48(s0)  param int z
    .globl full
    .type full, @function
full:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -32
    sw    a0, -16(s0)
    sw    a1, -12(s0)
    sw    a2, -24(s0)
    sw    a3, -20(s0)
    addi  t4, s0, -36
    lw    t0, 0(a4)
    sw    t0, 0(t4)
    lw    t0, 4(a4)
    sw    t0, 4(t4)
    lw    t0, 8(a4)
    sw    t0, 8(t4)
    sw    a5, -40(s0)
    sw    a6, -44(s0)
    sw    a7, -48(s0)
    addi  a0, s0, -16
    addi  a0, a0, 4
    lh    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -24
    addi  a0, a0, 4
    lw    a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -36
    addi  a0, a0, 8
    lw    a0, 0(a0)
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
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -48(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
.L.return.full:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size full, . - full

    # struct big scaled(int k, struct big b): frame 32 bytes
    #     -12(s0)  param int k
    #     -24(s0)  param struct big b
    #     -28(s0)  temp struct big * (result address)
    .globl scaled
    .type scaled, @function
scaled:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sw    a0, -28(s0)
    sw    a1, -12(s0)
    addi  t4, s0, -24
    lw    t0, 0(a2)
    sw    t0, 0(t4)
    lw    t0, 4(a2)
    sw    t0, 4(t4)
    lw    t0, 8(a2)
    sw    t0, 8(t4)
    addi  a0, s0, -24
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, 0(sp)
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -12(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    mul   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sw    a0, 0(a1)
    addi  a0, s0, -24
    addi  a0, a0, 4
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, 0(sp)
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -12(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    mul   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sw    a0, 0(a1)
    addi  a0, s0, -24
    addi  a0, a0, 8
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, 0(sp)
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -12(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    mul   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sw    a0, 0(a1)
    addi  a0, s0, -24
    lw    a1, -28(s0)
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
.L.return.scaled:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size scaled, . - scaled

    # int main(void): frame 752 bytes
    #     -12(s0)  local int ok
    #     -32(s0)  local struct six arr[3]
    #     -44(s0)  local struct big b
    #     -56(s0)  local struct big c
    #     -64(s0)  local struct pair p
    #    -144(s0)  local struct huge h
    #    -224(s0)  temp struct huge (call result)
    #    -228(s0)  local struct word w
    #    -232(s0)  local struct pair (*fp)(struct pair)
    #    -236(s0)  temp struct one (call result)
    #    -244(s0)  temp struct six (call result)
    #    -252(s0)  temp struct six (call result)
    #    -260(s0)  temp struct six (call result)
    #    -268(s0)  temp struct pair (call result)
    #    -280(s0)  temp struct big (argument copy)
    #    -292(s0)  temp struct big (argument copy)
    #    -304(s0)  temp struct big (call result)
    #    -316(s0)  temp struct big (argument copy)
    #    -396(s0)  temp struct huge (argument copy)
    #    -476(s0)  temp struct huge (call result)
    #    -488(s0)  temp struct big (argument copy)
    #    -492(s0)  temp struct word (call result)
    #    -496(s0)  temp struct word (call result)
    #    -508(s0)  temp struct big (argument copy)
    #    -520(s0)  temp struct big (argument copy)
    #    -532(s0)  temp struct big (argument copy)
    #    -544(s0)  temp struct big (call result)
    #    -556(s0)  temp struct big (argument copy)
    #    -568(s0)  temp struct big (argument copy)
    #    -580(s0)  temp struct big (call result)
    #    -592(s0)  temp struct big (argument copy)
    #    -604(s0)  temp struct big (call result)
    #    -612(s0)  temp struct pair (call result)
    #    -620(s0)  temp struct pair (call result)
    #    -632(s0)  temp struct big (argument copy)
    #    -644(s0)  temp struct big (argument copy)
    #    -656(s0)  temp struct big (call result)
    #    -668(s0)  temp struct big (argument copy)
    #    -680(s0)  temp struct big (call result)
    #    -692(s0)  temp struct big (argument copy)
    #    -704(s0)  temp struct big (call result)
    #    -716(s0)  temp struct big (argument copy)
    #    -728(s0)  temp struct big (argument copy)
    #    -740(s0)  temp struct big (call result)
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -736
    li    a0, 0
    sw    a0, -12(s0)
    # struct big b = ...
    sw    zero, -44(s0)
    sw    zero, -40(s0)
    sw    zero, -36(s0)
    li    a0, 1
    sw    a0, -44(s0)
    li    a0, 2
    sw    a0, -40(s0)
    li    a0, 3
    sw    a0, -36(s0)
    # struct pair p = ...
    sw    zero, -64(s0)
    sw    zero, -60(s0)
    li    a0, 1
    sw    a0, -64(s0)
    li    a0, 2
    sw    a0, -60(s0)
    li    a0, 10
    mv    a1, a0
    addi  a0, s0, -224
    call  make_huge
    addi  a0, s0, -224
    addi  a1, s0, -144
    mv    t1, a0
    mv    t2, a1
    li    t3, 80
    add   t3, t1, t3
2:
    lw    t0, 0(t1)
    sw    t0, 0(t2)
    addi  t1, t1, 4
    addi  t2, t2, 4
    bltu  t1, t3, 2b
    # struct word w = ...
    sw    zero, -228(s0)
    li    a0, -3
    sh    a0, -228(s0)
    li    a0, 4
    sh    a0, -226(s0)
    la    a0, swap
    sw    a0, -232(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 122
    addi  sp, sp, -12
    call  mk_one
    addi  sp, sp, 12
    sw    a0, -236(s0)
    addi  a0, s0, -236
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 122
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
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    mv    a2, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    a0, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    call  mk_six
    addi  sp, sp, 12
    sw    a0, -244(s0)
    sw    a1, -240(s0)
    addi  a0, s0, -244
    addi  a0, a0, 4
    lh    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    addi  a0, s0, -32
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    li    t0, 6
    mul   a0, a0, t0
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 10
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 20
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 30
    mv    a2, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    a0, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    call  mk_six
    addi  sp, sp, 12
    sw    a0, -252(s0)
    sw    a1, -248(s0)
    addi  a0, s0, -252
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lhu   t0, 0(a0)
    sh    t0, 0(a1)
    lhu   t0, 2(a0)
    sh    t0, 2(a1)
    lhu   t0, 4(a0)
    sh    t0, 4(a1)
    mv    a0, a1
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -32
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    li    t0, 6
    mul   a0, a0, t0
    add   a0, a1, a0
    mv    t1, a0
    lhu   a0, 0(t1)
    lhu   t0, 2(t1)
    slli  t0, t0, 16
    or    a0, a0, t0
    lhu   a1, 4(t1)
    addi  sp, sp, -12
    call  sum_six
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 60
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
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    mv    a2, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    a0, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    call  mk_six
    addi  sp, sp, 12
    sw    a0, -260(s0)
    sw    a1, -256(s0)
    addi  a0, s0, -260
    mv    t1, a0
    lhu   a0, 0(t1)
    lhu   t0, 2(t1)
    slli  t0, t0, 16
    or    a0, a0, t0
    lhu   a1, 4(t1)
    addi  sp, sp, -12
    call  sum_six
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
    addi  a0, s0, -64
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -64
    mv    t1, a0
    lw    a0, 0(t1)
    lw    a1, 4(t1)
    addi  sp, sp, -12
    call  swap
    addi  sp, sp, 12
    sw    a0, -268(s0)
    sw    a1, -264(s0)
    addi  a0, s0, -268
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    mv    a0, a1
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -64
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.7
    addi  a0, s0, -64
    addi  a0, a0, 4
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.7:
    snez  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    addi  a0, s0, -56
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -44
    addi  a1, s0, -280
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -44
    addi  a1, s0, -292
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    mv    a2, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    addi  a0, s0, -304
    addi  sp, sp, -12
    call  add_big
    addi  sp, sp, 12
    addi  a0, s0, -304
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -56
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.9
    addi  a0, s0, -56
    addi  a0, a0, 4
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.9:
    snez  a0, a0
    beqz  a0, .L.and.8
    addi  a0, s0, -56
    addi  a0, a0, 8
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 6
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.8:
    snez  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    addi  a0, s0, -44
    addi  a1, s0, -316
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    call  clobber
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -44
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.10
    addi  a0, s0, -44
    addi  a0, a0, 4
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.10:
    snez  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -144
    addi  a1, s0, -396
    mv    t1, a0
    mv    t2, a1
    li    t3, 80
    add   t3, t1, t3
2:
    lw    t0, 0(t1)
    sw    t0, 0(t2)
    addi  t1, t1, 4
    addi  t2, t2, 4
    bltu  t1, t3, 2b
    mv    a0, a1
    addi  sp, sp, -12
    call  sum_huge
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 390
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
    addi  a0, s0, -144
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
    li    a0, 10
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
    li    a0, 5
    mv    a1, a0
    addi  a0, s0, -476
    addi  sp, sp, -12
    call  make_huge
    addi  sp, sp, 12
    addi  a0, s0, -476
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 19
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 24
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
    addi  a0, s0, -32
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    li    t0, 6
    mul   a0, a0, t0
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -44
    addi  a1, s0, -488
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    mv    a5, a0
    lw    a4, 0(sp)
    addi  sp, sp, 4
    lw    a3, 0(sp)
    addi  sp, sp, 4
    lw    t1, 0(sp)
    addi  sp, sp, 4
    lhu   a1, 0(t1)
    lhu   t0, 2(t1)
    slli  t0, t0, 16
    or    a1, a1, t0
    lhu   a2, 4(t1)
    lw    a0, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    call  mixed
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 72
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
    addi  a0, s0, -228
    mv    t1, a0
    lhu   a0, 0(t1)
    lhu   t0, 2(t1)
    slli  t0, t0, 16
    or    a0, a0, t0
    addi  sp, sp, -12
    call  id_word
    addi  sp, sp, 12
    sw    a0, -492(s0)
    addi  a0, s0, -492
    lh    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, -3
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.11
    addi  a0, s0, -228
    mv    t1, a0
    lhu   a0, 0(t1)
    lhu   t0, 2(t1)
    slli  t0, t0, 16
    or    a0, a0, t0
    addi  sp, sp, -12
    call  id_word
    addi  sp, sp, 12
    sw    a0, -496(s0)
    addi  a0, s0, -496
    addi  a0, a0, 2
    lh    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.11:
    snez  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -32
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    li    t0, 6
    mul   a0, a0, t0
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -64
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -44
    addi  a1, s0, -508
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 4
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 5
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 6
    mv    a7, a0
    lw    a6, 0(sp)
    addi  sp, sp, 4
    lw    a5, 0(sp)
    addi  sp, sp, 4
    lw    a4, 0(sp)
    addi  sp, sp, 4
    lw    t1, 0(sp)
    addi  sp, sp, 4
    lw    a2, 0(t1)
    lw    a3, 4(t1)
    lw    t1, 0(sp)
    addi  sp, sp, 4
    lhu   a0, 0(t1)
    lhu   t0, 2(t1)
    slli  t0, t0, 16
    or    a0, a0, t0
    lhu   a1, 4(t1)
    addi  sp, sp, -12
    call  full
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 49
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
    addi  a0, s0, -44
    addi  a1, s0, -520
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -44
    addi  a1, s0, -532
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    mv    a2, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    addi  a0, s0, -544
    addi  sp, sp, -12
    call  add_big
    addi  sp, sp, 12
    addi  a0, s0, -544
    addi  a1, s0, -556
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -44
    addi  a1, s0, -568
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    mv    a2, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    addi  a0, s0, -580
    addi  sp, sp, -12
    call  add_big
    addi  sp, sp, 12
    addi  a0, s0, -580
    addi  a0, a0, 8
    lw    a0, 0(a0)
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
    li    a0, 3
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -44
    addi  a1, s0, -592
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    mv    a2, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    addi  a0, s0, -604
    addi  sp, sp, -12
    call  scaled
    addi  sp, sp, 12
    addi  a0, s0, -604
    addi  a0, a0, 4
    lw    a0, 0(a0)
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
    addi  a0, s0, -64
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -232(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -64
    mv    t1, a0
    lw    a0, 0(t1)
    lw    a1, 4(t1)
    lw    t1, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    jalr  t1
    addi  sp, sp, 12
    sw    a0, -612(s0)
    sw    a1, -608(s0)
    addi  a0, s0, -612
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    mv    a0, a1
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -64
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
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
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    lw    a1, 0(sp)
    addi  sp, sp, 4
    slt   a0, a0, a1
    beqz  a0, .L.else.12
    addi  a0, s0, -64
    j     .L.end.13
.L.else.12:
    addi  a0, s0, -64
    mv    t1, a0
    lw    a0, 0(t1)
    lw    a1, 4(t1)
    addi  sp, sp, -12
    call  swap
    addi  sp, sp, 12
    sw    a0, -620(s0)
    sw    a1, -616(s0)
    addi  a0, s0, -620
.L.end.13:
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    addi  a0, s0, -56
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -44
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -56
    addi  a1, s0, -632
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -44
    addi  a1, s0, -644
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    mv    a2, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    addi  a0, s0, -656
    addi  sp, sp, -8
    call  add_big
    addi  sp, sp, 8
    addi  a0, s0, -656
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -44
    addi  a0, a0, 8
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 9
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.14
    addi  a0, s0, -56
    addi  a0, a0, 8
    lw    a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 9
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.14:
    snez  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
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
    addi  a0, s0, -44
    addi  a1, s0, -668
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    mv    a2, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    addi  a0, s0, -680
    addi  sp, sp, -12
    call  scaled
    addi  sp, sp, 12
    addi  a0, s0, -680
    addi  a1, s0, -716
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    addi  a0, s0, -56
    addi  a1, s0, -692
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    mv    a2, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    addi  a0, s0, -704
    addi  sp, sp, -8
    call  scaled
    addi  sp, sp, 8
    addi  a0, s0, -704
    addi  a1, s0, -728
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    mv    a2, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    addi  a0, s0, -740
    addi  sp, sp, -12
    call  add_big
    addi  sp, sp, 12
    addi  a0, s0, -740
    lw    a0, 0(a0)
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
.L.return.main:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size main, . - main
