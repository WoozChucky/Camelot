    .text

    # int length(char *s): frame 16 bytes
    #     -12(s0)  param char *s
    #     -16(s0)  local int n
    .globl length
    .type length, @function
length:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    sw    a0, -12(s0)
    li    a0, 0
    sw    a0, -16(s0)
.L.continue.1:
    lw    a0, -12(s0)
    addi  a0, a0, 1
    sw    a0, -12(s0)
    addi  a0, a0, -1
    lbu   a0, 0(a0)
    beqz  a0, .L.break.2
    lw    a0, -16(s0)
    addi  a0, a0, 1
    sw    a0, -16(s0)
    addi  a0, a0, -1
    j     .L.continue.1
.L.break.2:
    lw    a0, -16(s0)
.L.return.length:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size length, . - length

    # int same(char *a, char *b): frame 16 bytes
    #     -12(s0)  param char *a
    #     -16(s0)  param char *b
    .globl same
    .type same, @function
same:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    sw    a0, -12(s0)
    sw    a1, -16(s0)
.L.continue.3:
    lw    a0, -12(s0)
    lbu   a0, 0(a0)
    beqz  a0, .L.and.5
    lw    a0, -12(s0)
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -16(s0)
    lbu   a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
.L.and.5:
    snez  a0, a0
    beqz  a0, .L.break.4
    lw    a0, -12(s0)
    addi  a0, a0, 1
    sw    a0, -12(s0)
    addi  a0, a0, -1
    lw    a0, -16(s0)
    addi  a0, a0, 1
    sw    a0, -16(s0)
    addi  a0, a0, -1
    j     .L.continue.3
.L.break.4:
    lw    a0, -12(s0)
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -16(s0)
    lbu   a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
.L.return.same:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size same, . - same

    # int main(void): frame 32 bytes
    #     -12(s0)  local char *greeting
    #     -16(s0)  local int r
    #     -17(s0)  local char quote
    .globl main
    .type main, @function
main:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    addi  sp, sp, -16
    la    a0, .L.str.0
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    call  length
    sw    a0, -16(s0)
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, .L.str.1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, .L.str.1
    mv    a1, a0
    lw    a0, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -12
    call  same
    addi  sp, sp, 12
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, .L.str.1
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, .L.str.2
    mv    a1, a0
    lw    a0, 0(sp)
    addi  sp, sp, 4
    addi  sp, sp, -8
    call  same
    addi  sp, sp, 8
    seqz  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -16(s0)
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, -10
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -16(s0)
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, .L.str.3
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 2
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 67
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, .L.str.4
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 3
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -16(s0)
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 6
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -16(s0)
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 12
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 10
    lw    a1, 0(sp)
    addi  sp, sp, 4
    xor   a0, a1, a0
    seqz  a0, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, .L.str.5
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
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
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -16(s0)
    li    a0, 34
    sb    a0, -17(s0)
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lbu   a0, -17(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    la    a0, .L.str.6
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    lbu   a0, 0(a0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 97
    lw    a1, 0(sp)
    addi  sp, sp, 4
    sub   a0, a1, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -16(s0)
    lw    a0, -16(s0)
.L.return.main:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
    .size main, . - main

    .section .rodata

.L.str.0:
    .asciz "hello, world\n"

.L.str.1:
    .asciz "abc"

.L.str.2:
    .asciz "abd"

.L.str.3:
    .asciz "ABC"

.L.str.4:
    .asciz "tab\there"

.L.str.5:
    .asciz "q"

.L.str.6:
    .asciz "\"\\"
