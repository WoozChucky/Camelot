/*
 * usize strlen(const char *s): the bytes before the first NUL. The
 * kernel's strlen and the user library's (kernel/user/lib/strlen.S
 * includes this file).
 *
 * A byte at a time, two to a round so that a round's branch back is
 * shared by two bytes: about three instructions a byte, where merlin's C
 * takes some twenty. A register at a time (the bytes to a boundary, then
 * the has-a-zero-byte arithmetic, (v - 0x01...01) & ~v & 0x80...80, on
 * each register) was tried and measured no better (#133): the strings the
 * kernel and the tools measure are a few bytes long.
 *
 *     const char *p = s;
 *     for (;; p += 2) {
 *         if (p[0] == 0)
 *             return p - s;
 *         if (p[1] == 0)
 *             return p + 1 - s;
 *     }
 *
 * Registers: a0 s, then the result; t0 p; t1 a byte.
 */
#include <asm.h>

    .text
    .globl strlen
strlen:
    mv    t0, a0
.Llen_pair:
    lbu   t1, 0(t0)
    beqz  t1, .Llen_end
    lbu   t1, 1(t0)
    addi  t0, t0, 2
    bnez  t1, .Llen_pair
    /* The NUL was the second byte of the pair. */
    addi  t0, t0, -1
.Llen_end:
    sub   a0, t0, a0
    ret
