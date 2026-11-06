# The framebuffer console: the pixel loops (fbcon.c), in assembly: merlin C
# keeps every value on the stack, some 35 instructions a store, and the
# console draws a glyph for each character printed and moves the pixels of
# the screen on each scroll. Here a glyph row is about 18 instructions and a
# word of a fill or copy one or two.
#
# VRAM is rows of `stride` bytes; the console cells are 8 pixels of 4
# bytes wide, so a row of cells is a whole number of 32-byte groups, 4 u64s,
# which the loops move at a time.
#
#   void fbcon_glyph(u64 *dst, const u8 *rows, const u64 *nibbles, u64 stride, u64 height)
#       A glyph 8 pixels wide: for each of `height` rows, of the byte rows[y]
#       the high nibble and the low nibble as nibbles[2 * n] and nibbles[2 * n + 1]
#       (4 pixels, two to a u64), at dst + y * stride.
#   void fbcon_fill(u64 *dst, u64 groups, u64 rows, u64 stride, u64 value)
#       `rows` rows from dst, each `groups` groups of 4 u64s set to value.
#   void fbcon_copy(u64 *dst, const u64 *src, u64 groups, u64 rows, u64 stride)
#       The same rows copied from src, which does not overlap them.
#   u64 fbcon_diff(const u16 *a, const u16 *b, u64 n)
#       The first index below n where a and b differ, or n.

#include <asm.h>

    .text

    .globl fbcon_glyph
fbcon_glyph:
    beqz  a4, 2f
1:
    lbu   t0, 0(a1)
    srli  t1, t0, 4
    andi  t0, t0, 15
    slli  t1, t1, 4
    slli  t0, t0, 4
    add   t1, t1, a2
    add   t0, t0, a2
    ld    t2, 0(t1)
    ld    t3, 8(t1)
    ld    t4, 0(t0)
    ld    t5, 8(t0)
    sd    t2, 0(a0)
    sd    t3, 8(a0)
    sd    t4, 16(a0)
    sd    t5, 24(a0)
    addi  a1, a1, 1
    add   a0, a0, a3
    addi  a4, a4, -1
    bnez  a4, 1b
2:
    ret

    .globl fbcon_fill
fbcon_fill:
    beqz  a1, 3f
    beqz  a2, 3f
    slli  a1, a1, 5              # bytes a row
1:
    mv    t0, a0
    add   t1, a0, a1             # the end of the row
2:
    sd    a4, 0(t0)
    sd    a4, 8(t0)
    sd    a4, 16(t0)
    sd    a4, 24(t0)
    addi  t0, t0, 32
    bltu  t0, t1, 2b
    add   a0, a0, a3
    addi  a2, a2, -1
    bnez  a2, 1b
3:
    ret

    .globl fbcon_copy
fbcon_copy:
    beqz  a2, 3f
    beqz  a3, 3f
    slli  a2, a2, 5              # bytes a row
1:
    mv    t0, a0
    mv    t1, a1
    add   t2, a0, a2             # the end of the row
2:
    ld    t3, 0(t1)
    ld    t4, 8(t1)
    ld    t5, 16(t1)
    ld    t6, 24(t1)
    sd    t3, 0(t0)
    sd    t4, 8(t0)
    sd    t5, 16(t0)
    sd    t6, 24(t0)
    addi  t0, t0, 32
    addi  t1, t1, 32
    bltu  t0, t2, 2b
    add   a0, a0, a4
    add   a1, a1, a4
    addi  a3, a3, -1
    bnez  a3, 1b
3:
    ret

    .globl fbcon_diff
fbcon_diff:
    li    t0, 0
1:
    bgeu  t0, a2, 2f
    lhu   t1, 0(a0)
    lhu   t2, 0(a1)
    bne   t1, t2, 2f
    addi  a0, a0, 2
    addi  a1, a1, 2
    addi  t0, t0, 1
    j     1b
2:
    mv    a0, t0
    ret
