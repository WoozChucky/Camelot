    .text

    # struct one mk_one(char c): frame 32 bytes
    #     -17(s0)  param char c
    #     -32(s0)  local struct one o
    .globl mk_one
    .type mk_one, @function
mk_one:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sb    a0, -17(s0)
    addi  a0, s0, -32
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lbu   a0, -17(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sb    a0, 0(a1)
    addi  a0, s0, -32
    mv    t1, a0
    lbu   a0, 0(t1)
.L.return.mk_one:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size mk_one, . - mk_one

    # struct six mk_six(short a, short b, short c): frame 32 bytes
    #     -18(s0)  param short a
    #     -20(s0)  param short b
    #     -22(s0)  param short c
    #     -32(s0)  local struct six s
    .globl mk_six
    .type mk_six, @function
mk_six:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sh    a0, -18(s0)
    sh    a1, -20(s0)
    sh    a2, -22(s0)
    addi  a0, s0, -32
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lh    a0, -18(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sh    a0, 0(a1)
    addi  a0, s0, -32
    addi  a0, a0, 2
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lh    a0, -20(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sh    a0, 0(a1)
    addi  a0, s0, -32
    addi  a0, a0, 4
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lh    a0, -22(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sh    a0, 0(a1)
    addi  a0, s0, -32
    mv    t1, a0
    lhu   a0, 0(t1)
    lhu   t0, 2(t1)
    slli  t0, t0, 16
    or    a0, a0, t0
    lhu   t0, 4(t1)
    slli  t0, t0, 32
    or    a0, a0, t0
.L.return.mk_six:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size mk_six, . - mk_six

    # struct pair swap(struct pair p): frame 32 bytes
    #     -24(s0)  param struct pair p
    #     -28(s0)  local int t
    .globl swap
    .type swap, @function
swap:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sd    a0, -24(s0)
    addi  a0, s0, -24
    lw    a0, 0(a0)
    sw    a0, -28(s0)
    addi  a0, s0, -24
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -24
    addi  a0, a0, 4
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sw    a0, 0(a1)
    addi  a0, s0, -24
    addi  a0, a0, 4
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -28(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sw    a0, 0(a1)
    addi  a0, s0, -24
    mv    t1, a0
    lwu   a0, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a0, a0, t0
.L.return.swap:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size swap, . - swap

    # struct big add_big(struct big l, struct big r): frame 64 bytes
    #     -32(s0)  param struct big l
    #     -48(s0)  param struct big r
    #     -64(s0)  local struct big s
    .globl add_big
    .type add_big, @function
add_big:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -48
    sd    a0, -32(s0)
    sd    a1, -24(s0)
    sd    a2, -48(s0)
    sd    a3, -40(s0)
    addi  a0, s0, -64
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -32
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -48
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sw    a0, 0(a1)
    addi  a0, s0, -64
    addi  a0, a0, 4
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -32
    addi  a0, a0, 4
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -48
    addi  a0, a0, 4
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sw    a0, 0(a1)
    addi  a0, s0, -64
    addi  a0, a0, 8
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -32
    addi  a0, a0, 8
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -48
    addi  a0, a0, 8
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sw    a0, 0(a1)
    addi  a0, s0, -64
    mv    t1, a0
    lwu   a0, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a0, a0, t0
    lwu   a1, 8(t1)
.L.return.add_big:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size add_big, . - add_big

    # int sum_six(struct six s): frame 32 bytes
    #     -24(s0)  param struct six s
    .globl sum_six
    .type sum_six, @function
sum_six:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sd    a0, -24(s0)
    addi  a0, s0, -24
    lh    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -24
    addi  a0, a0, 2
    lh    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -24
    addi  a0, a0, 4
    lh    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
.L.return.sum_six:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size sum_six, . - sum_six

    # void clobber(struct big b): frame 32 bytes
    #     -32(s0)  param struct big b
    .globl clobber
    .type clobber, @function
clobber:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sd    a0, -32(s0)
    sd    a1, -24(s0)
    addi  a0, s0, -32
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 99
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sw    a0, 0(a1)
    addi  a0, s0, -32
    addi  a0, a0, 4
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 99
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sw    a0, 0(a1)
    li    a0, 0
.L.return.clobber:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size clobber, . - clobber

    # int sum_huge(struct huge h): frame 112 bytes
    #     -96(s0)  param struct huge h
    #    -100(s0)  local int i
    #    -104(s0)  local int s
    .globl sum_huge
    .type sum_huge, @function
sum_huge:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -96
    addi  t4, s0, -96
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
    sw    a0, -104(s0)
    li    a0, 0
    sw    a0, -100(s0)
.L.loop.1:
    lw    a0, -100(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 20
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slt   a0, a1, a0
    beqz  a0, .L.break.3
    lw    a0, -104(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -96
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -100(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -104(s0)
.L.continue.2:
    lw    a0, -100(s0)
    addiw a0, a0, 1
    sw    a0, -100(s0)
    addiw a0, a0, -1
    j     .L.loop.1
.L.break.3:
    addi  a0, s0, -96
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 2
    add   a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1000
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sw    a0, 0(a1)
    lw    a0, -104(s0)
.L.return.sum_huge:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size sum_huge, . - sum_huge

    # struct huge make_huge(int base): frame 128 bytes
    #     -20(s0)  param int base
    #     -32(s0)  temp struct huge * (result address)
    #    -112(s0)  local struct huge h
    #    -116(s0)  local int i
    .globl make_huge
    .type make_huge, @function
make_huge:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -112
    sd    a0, -32(s0)
    sw    a1, -20(s0)
    li    a0, 0
    sw    a0, -116(s0)
.L.loop.4:
    lw    a0, -116(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 20
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slt   a0, a1, a0
    beqz  a0, .L.break.6
    addi  a0, s0, -112
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -116(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 2
    add   a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -116(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sw    a0, 0(a1)
.L.continue.5:
    lw    a0, -116(s0)
    addiw a0, a0, 1
    sw    a0, -116(s0)
    addiw a0, a0, -1
    j     .L.loop.4
.L.break.6:
    addi  a0, s0, -112
    ld    a1, -32(s0)
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
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size make_huge, . - make_huge

    # int mixed(int a, struct six s, int b, struct big c, char d): frame 64 bytes
    #     -20(s0)  param int a
    #     -32(s0)  param struct six s
    #     -36(s0)  param int b
    #     -56(s0)  param struct big c
    #     -57(s0)  param char d
    .globl mixed
    .type mixed, @function
mixed:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -48
    sw    a0, -20(s0)
    sd    a1, -32(s0)
    sw    a2, -36(s0)
    sd    a3, -56(s0)
    sd    a4, -48(s0)
    sb    a5, -57(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -32
    mv    t1, a0
    lhu   a0, 0(t1)
    lhu   t0, 2(t1)
    slli  t0, t0, 16
    or    a0, a0, t0
    lhu   t0, 4(t1)
    slli  t0, t0, 32
    or    a0, a0, t0
    addi  sp, sp, -8
    call  sum_six
    addi  sp, sp, 8
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
    addi  a0, s0, -56
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -56
    addi  a0, a0, 4
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -56
    addi  a0, a0, 8
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lbu   a0, -57(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
.L.return.mixed:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size mixed, . - mixed

    # struct word id_word(struct word w): frame 32 bytes
    #     -24(s0)  param struct word w
    .globl id_word
    .type id_word, @function
id_word:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    sd    a0, -24(s0)
    addi  a0, s0, -24
    mv    t1, a0
    lhu   a0, 0(t1)
    lhu   t0, 2(t1)
    slli  t0, t0, 16
    or    a0, a0, t0
.L.return.id_word:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size id_word, . - id_word

    # int full(struct six s, struct pair p, struct big b, int x, int y, int z): frame 64 bytes
    #     -24(s0)  param struct six s
    #     -32(s0)  param struct pair p
    #     -48(s0)  param struct big b
    #     -52(s0)  param int x
    #     -56(s0)  param int y
    #     -60(s0)  param int z
    .globl full
    .type full, @function
full:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -48
    sd    a0, -24(s0)
    sd    a1, -32(s0)
    sd    a2, -48(s0)
    sd    a3, -40(s0)
    sw    a4, -52(s0)
    sw    a5, -56(s0)
    sw    a6, -60(s0)
    addi  a0, s0, -24
    addi  a0, a0, 4
    lh    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -32
    addi  a0, a0, 4
    lw    a0, 0(a0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -48
    addi  a0, a0, 8
    lw    a0, 0(a0)
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
    lw    a0, -56(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -60(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
.L.return.full:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size full, . - full

    # struct big scaled(int k, struct big b): frame 48 bytes
    #     -20(s0)  param int k
    #     -40(s0)  param struct big b
    .globl scaled
    .type scaled, @function
scaled:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -32
    sw    a0, -20(s0)
    sd    a1, -40(s0)
    sd    a2, -32(s0)
    addi  a0, s0, -40
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, 0(sp)
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -20(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    mulw  a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sw    a0, 0(a1)
    addi  a0, s0, -40
    addi  a0, a0, 4
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, 0(sp)
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -20(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    mulw  a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sw    a0, 0(a1)
    addi  a0, s0, -40
    addi  a0, a0, 8
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, 0(sp)
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    lw    a0, -20(s0)
    ld    a1, 0(sp)
    addi  sp, sp, 8
    mulw  a0, a1, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    sw    a0, 0(a1)
    addi  a0, s0, -40
    mv    t1, a0
    lwu   a0, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a0, a0, t0
    lwu   a1, 8(t1)
.L.return.scaled:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size scaled, . - scaled

    # int main(void): frame 624 bytes
    #     -20(s0)  local int ok
    #     -40(s0)  local struct six arr[3]
    #     -56(s0)  local struct big b
    #     -72(s0)  local struct big c
    #     -80(s0)  local struct pair p
    #    -160(s0)  local struct huge h
    #    -240(s0)  temp struct huge (call result)
    #    -248(s0)  local struct word w
    #    -256(s0)  local struct pair (*fp)(struct pair)
    #    -264(s0)  temp struct one (call result)
    #    -272(s0)  temp struct six (call result)
    #    -280(s0)  temp struct six (call result)
    #    -288(s0)  temp struct six (call result)
    #    -296(s0)  temp struct pair (call result)
    #    -312(s0)  temp struct big (call result)
    #    -392(s0)  temp struct huge (argument copy)
    #    -472(s0)  temp struct huge (call result)
    #    -480(s0)  temp struct word (call result)
    #    -488(s0)  temp struct word (call result)
    #    -504(s0)  temp struct big (call result)
    #    -520(s0)  temp struct big (call result)
    #    -536(s0)  temp struct big (call result)
    #    -544(s0)  temp struct pair (call result)
    #    -552(s0)  temp struct pair (call result)
    #    -568(s0)  temp struct big (call result)
    #    -584(s0)  temp struct big (call result)
    #    -600(s0)  temp struct big (call result)
    #    -616(s0)  temp struct big (call result)
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    sd    s0, 0(sp)
    addi  s0, sp, 16
    addi  sp, sp, -608
    li    a0, 0
    sw    a0, -20(s0)
    # struct big b = ...
    sd    zero, -56(s0)
    sb    zero, -48(s0)
    sb    zero, -47(s0)
    sb    zero, -46(s0)
    sb    zero, -45(s0)
    li    a0, 1
    sw    a0, -56(s0)
    li    a0, 2
    sw    a0, -52(s0)
    li    a0, 3
    sw    a0, -48(s0)
    # struct pair p = ...
    sd    zero, -80(s0)
    li    a0, 1
    sw    a0, -80(s0)
    li    a0, 2
    sw    a0, -76(s0)
    li    a0, 10
    mv    a1, a0
    addi  a0, s0, -240
    call  make_huge
    addi  a0, s0, -240
    addi  a1, s0, -160
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
    sb    zero, -248(s0)
    sb    zero, -247(s0)
    sb    zero, -246(s0)
    sb    zero, -245(s0)
    li    a0, -3
    sh    a0, -248(s0)
    li    a0, 4
    sh    a0, -246(s0)
    la    a0, swap
    sd    a0, -256(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 122
    addi  sp, sp, -8
    call  mk_one
    addi  sp, sp, 8
    sd    a0, -264(s0)
    addi  a0, s0, -264
    lbu   a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 122
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
    li    a0, 1
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    mv    a2, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    ld    a0, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    call  mk_six
    addi  sp, sp, 8
    sd    a0, -272(s0)
    addi  a0, s0, -272
    addi  a0, a0, 4
    lh    a0, 0(a0)
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
    addi  a0, s0, -40
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    li    t0, 6
    mul   a0, a0, t0
    add   a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 10
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 20
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 30
    mv    a2, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    ld    a0, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    call  mk_six
    addi  sp, sp, 8
    sd    a0, -280(s0)
    addi  a0, s0, -280
    ld    a1, 0(sp)
    addi  sp, sp, 8
    lhu   t0, 0(a0)
    sh    t0, 0(a1)
    lhu   t0, 2(a0)
    sh    t0, 2(a1)
    lhu   t0, 4(a0)
    sh    t0, 4(a1)
    mv    a0, a1
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -40
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    li    t0, 6
    mul   a0, a0, t0
    add   a0, a1, a0
    mv    t1, a0
    lhu   a0, 0(t1)
    lhu   t0, 2(t1)
    slli  t0, t0, 16
    or    a0, a0, t0
    lhu   t0, 4(t1)
    slli  t0, t0, 32
    or    a0, a0, t0
    addi  sp, sp, -8
    call  sum_six
    addi  sp, sp, 8
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 60
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
    li    a0, 1
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    mv    a2, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    ld    a0, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    call  mk_six
    addi  sp, sp, 8
    sd    a0, -288(s0)
    addi  a0, s0, -288
    mv    t1, a0
    lhu   a0, 0(t1)
    lhu   t0, 2(t1)
    slli  t0, t0, 16
    or    a0, a0, t0
    lhu   t0, 4(t1)
    slli  t0, t0, 32
    or    a0, a0, t0
    addi  sp, sp, -8
    call  sum_six
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
    addi  a0, s0, -80
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -80
    mv    t1, a0
    lwu   a0, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a0, a0, t0
    addi  sp, sp, -8
    call  swap
    addi  sp, sp, 8
    sd    a0, -296(s0)
    addi  a0, s0, -296
    ld    a1, 0(sp)
    addi  sp, sp, 8
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    mv    a0, a1
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -80
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.7
    addi  a0, s0, -80
    addi  a0, a0, 4
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.7:
    snez  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    addi  a0, s0, -72
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -56
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -56
    mv    t1, a0
    lwu   a2, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a2, a2, t0
    lwu   a3, 8(t1)
    ld    t1, 0(sp)
    addi  sp, sp, 8
    lwu   a0, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a0, a0, t0
    lwu   a1, 8(t1)
    addi  sp, sp, -8
    call  add_big
    addi  sp, sp, 8
    sd    a0, -312(s0)
    sd    a1, -304(s0)
    addi  a0, s0, -312
    ld    a1, 0(sp)
    addi  sp, sp, 8
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -72
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.9
    addi  a0, s0, -72
    addi  a0, a0, 4
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 4
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.9:
    snez  a0, a0
    beqz  a0, .L.and.8
    addi  a0, s0, -72
    addi  a0, a0, 8
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 6
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.8:
    snez  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    addi  a0, s0, -56
    mv    t1, a0
    lwu   a0, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a0, a0, t0
    lwu   a1, 8(t1)
    call  clobber
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -56
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.10
    addi  a0, s0, -56
    addi  a0, a0, 4
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.10:
    snez  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -160
    addi  a1, s0, -392
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
    addi  sp, sp, -8
    call  sum_huge
    addi  sp, sp, 8
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 390
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
    addi  a0, s0, -160
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
    li    a0, 10
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
    li    a0, 5
    mv    a1, a0
    addi  a0, s0, -472
    addi  sp, sp, -8
    call  make_huge
    addi  sp, sp, 8
    addi  a0, s0, -472
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 19
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slli  a0, a0, 2
    add   a0, a1, a0
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 24
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
    li    a0, 1
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -40
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    li    t0, 6
    mul   a0, a0, t0
    add   a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -56
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    mv    a5, a0
    ld    t1, 0(sp)
    addi  sp, sp, 8
    lwu   a3, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a3, a3, t0
    lwu   a4, 8(t1)
    ld    a2, 0(sp)
    addi  sp, sp, 8
    ld    t1, 0(sp)
    addi  sp, sp, 8
    lhu   a1, 0(t1)
    lhu   t0, 2(t1)
    slli  t0, t0, 16
    or    a1, a1, t0
    lhu   t0, 4(t1)
    slli  t0, t0, 32
    or    a1, a1, t0
    ld    a0, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    call  mixed
    addi  sp, sp, 8
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 72
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
    addi  a0, s0, -248
    mv    t1, a0
    lhu   a0, 0(t1)
    lhu   t0, 2(t1)
    slli  t0, t0, 16
    or    a0, a0, t0
    addi  sp, sp, -8
    call  id_word
    addi  sp, sp, 8
    sd    a0, -480(s0)
    addi  a0, s0, -480
    lh    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, -3
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.11
    addi  a0, s0, -248
    mv    t1, a0
    lhu   a0, 0(t1)
    lhu   t0, 2(t1)
    slli  t0, t0, 16
    or    a0, a0, t0
    addi  sp, sp, -8
    call  id_word
    addi  sp, sp, 8
    sd    a0, -488(s0)
    addi  a0, s0, -488
    addi  a0, a0, 2
    lh    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 4
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.11:
    snez  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -40
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    li    t0, 6
    mul   a0, a0, t0
    add   a0, a1, a0
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -80
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -56
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 4
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 5
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 6
    mv    a6, a0
    ld    a5, 0(sp)
    addi  sp, sp, 8
    ld    a4, 0(sp)
    addi  sp, sp, 8
    ld    t1, 0(sp)
    addi  sp, sp, 8
    lwu   a2, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a2, a2, t0
    lwu   a3, 8(t1)
    ld    t1, 0(sp)
    addi  sp, sp, 8
    lwu   a1, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a1, a1, t0
    ld    t1, 0(sp)
    addi  sp, sp, 8
    lhu   a0, 0(t1)
    lhu   t0, 2(t1)
    slli  t0, t0, 16
    or    a0, a0, t0
    lhu   t0, 4(t1)
    slli  t0, t0, 32
    or    a0, a0, t0
    addi  sp, sp, -8
    call  full
    addi  sp, sp, 8
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 49
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
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -56
    mv    t1, a0
    lwu   a2, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a2, a2, t0
    lwu   a3, 8(t1)
    ld    t1, 0(sp)
    addi  sp, sp, 8
    lwu   a0, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a0, a0, t0
    lwu   a1, 8(t1)
    addi  sp, sp, -8
    call  add_big
    addi  sp, sp, 8
    sd    a0, -504(s0)
    sd    a1, -496(s0)
    addi  a0, s0, -504
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -56
    mv    t1, a0
    lwu   a2, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a2, a2, t0
    lwu   a3, 8(t1)
    ld    t1, 0(sp)
    addi  sp, sp, 8
    lwu   a0, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a0, a0, t0
    lwu   a1, 8(t1)
    addi  sp, sp, -8
    call  add_big
    addi  sp, sp, 8
    sd    a0, -520(s0)
    sd    a1, -512(s0)
    addi  a0, s0, -520
    addi  a0, a0, 8
    lw    a0, 0(a0)
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
    li    a0, 3
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -56
    mv    t1, a0
    lwu   a1, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a1, a1, t0
    lwu   a2, 8(t1)
    ld    a0, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    call  scaled
    addi  sp, sp, 8
    sd    a0, -536(s0)
    sd    a1, -528(s0)
    addi  a0, s0, -536
    addi  a0, a0, 4
    lw    a0, 0(a0)
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
    addi  a0, s0, -80
    addi  sp, sp, -8
    sd    a0, 0(sp)
    ld    a0, -256(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -80
    mv    t1, a0
    lwu   a0, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a0, a0, t0
    ld    t1, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    jalr  t1
    addi  sp, sp, 8
    sd    a0, -544(s0)
    addi  a0, s0, -544
    ld    a1, 0(sp)
    addi  sp, sp, 8
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    mv    a0, a1
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -80
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
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
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 3
    ld    a1, 0(sp)
    addi  sp, sp, 8
    slt   a0, a0, a1
    beqz  a0, .L.else.12
    addi  a0, s0, -80
    j     .L.end.13
.L.else.12:
    addi  a0, s0, -80
    mv    t1, a0
    lwu   a0, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a0, a0, t0
    addi  sp, sp, -8
    call  swap
    addi  sp, sp, 8
    sd    a0, -552(s0)
    addi  a0, s0, -552
.L.end.13:
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    addi  a0, s0, -72
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -56
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -72
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -56
    mv    t1, a0
    lwu   a2, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a2, a2, t0
    lwu   a3, 8(t1)
    ld    t1, 0(sp)
    addi  sp, sp, 8
    lwu   a0, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a0, a0, t0
    lwu   a1, 8(t1)
    call  add_big
    sd    a0, -568(s0)
    sd    a1, -560(s0)
    addi  a0, s0, -568
    ld    a1, 0(sp)
    addi  sp, sp, 8
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    lw    t0, 4(a0)
    sw    t0, 4(a1)
    lw    t0, 8(a0)
    sw    t0, 8(a1)
    mv    a0, a1
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -56
    addi  a0, a0, 8
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 9
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
    beqz  a0, .L.and.14
    addi  a0, s0, -72
    addi  a0, a0, 8
    lw    a0, 0(a0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 9
    ld    a1, 0(sp)
    addi  sp, sp, 8
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.14:
    snez  a0, a0
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    ld    a1, 0(sp)
    addi  sp, sp, 8
    addw  a0, a1, a0
    sw    a0, -20(s0)
    lw    a0, -20(s0)
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 2
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -56
    mv    t1, a0
    lwu   a1, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a1, a1, t0
    lwu   a2, 8(t1)
    ld    a0, 0(sp)
    addi  sp, sp, 8
    addi  sp, sp, -8
    call  scaled
    addi  sp, sp, 8
    sd    a0, -584(s0)
    sd    a1, -576(s0)
    addi  a0, s0, -584
    addi  sp, sp, -8
    sd    a0, 0(sp)
    li    a0, 1
    addi  sp, sp, -8
    sd    a0, 0(sp)
    addi  a0, s0, -72
    mv    t1, a0
    lwu   a1, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a1, a1, t0
    lwu   a2, 8(t1)
    ld    a0, 0(sp)
    addi  sp, sp, 8
    call  scaled
    sd    a0, -600(s0)
    sd    a1, -592(s0)
    addi  a0, s0, -600
    mv    t1, a0
    lwu   a2, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a2, a2, t0
    lwu   a3, 8(t1)
    ld    t1, 0(sp)
    addi  sp, sp, 8
    lwu   a0, 0(t1)
    lwu   t0, 4(t1)
    slli  t0, t0, 32
    or    a0, a0, t0
    lwu   a1, 8(t1)
    addi  sp, sp, -8
    call  add_big
    addi  sp, sp, 8
    sd    a0, -616(s0)
    sd    a1, -608(s0)
    addi  a0, s0, -616
    lw    a0, 0(a0)
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
.L.return.main:
    addi  sp, s0, -16
    ld    ra, 8(sp)
    ld    s0, 0(sp)
    addi  sp, sp, 16
    ret
    .size main, . - main
