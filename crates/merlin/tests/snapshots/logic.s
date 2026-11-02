    .text

    # int bump(v): frame 16 bytes
    #     -12(s0)  param v
    .globl bump
bump:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    sw    a0, -12(s0)
    lw    a0, -12(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
.L.return.bump:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret

    # int main(): frame 16 bytes
    #     -12(s0)  local hits
    #     -16(s0)  local r
    .globl main
main:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    s0, 8(sp)
    addi  s0, sp, 16
    li    a0, 0
    sw    a0, -12(s0)
    li    a0, 5
    beqz  a0, .L.and.1
    li    a0, 7
.L.and.1:
    snez  a0, a0
    sw    a0, -16(s0)
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    bnez  a0, .L.or.2
    li    a0, 3
    neg   a0, a0
.L.or.2:
    snez  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -16(s0)
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    beqz  a0, .L.and.3
    lw    a0, -12(s0)
    addi  sp, sp, -12
    call  bump
    addi  sp, sp, 12
    sw    a0, -12(s0)
.L.and.3:
    snez  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -16(s0)
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    bnez  a0, .L.or.4
    lw    a0, -12(s0)
    addi  sp, sp, -12
    call  bump
    addi  sp, sp, 12
    sw    a0, -12(s0)
.L.or.4:
    snez  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -16(s0)
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 1
    beqz  a0, .L.and.5
    lw    a0, -12(s0)
    addi  sp, sp, -12
    call  bump
    addi  sp, sp, 12
    sw    a0, -12(s0)
.L.and.5:
    snez  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -16(s0)
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    li    a0, 0
    bnez  a0, .L.or.6
    lw    a0, -12(s0)
    addi  sp, sp, -12
    call  bump
    addi  sp, sp, 12
    sw    a0, -12(s0)
.L.or.6:
    snez  a0, a0
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
    sw    a0, -16(s0)
    lw    a0, -16(s0)
    addi  sp, sp, -4
    sw    a0, 0(sp)
    lw    a0, -12(s0)
    lw    a1, 0(sp)
    addi  sp, sp, 4
    add   a0, a1, a0
.L.return.main:
    addi  sp, s0, -16
    lw    ra, 12(sp)
    lw    s0, 8(sp)
    addi  sp, sp, 16
    ret
