/*
 * int strcmp(const char *a, const char *b): the difference of the first
 * bytes that differ, as unsigned bytes (negative when a's is lower), or 0
 * for the same string. The kernel's strcmp and the user library's
 * (kernel/user/lib/strcmp.S includes this file).
 *
 * A byte of each at a time, two to a round so that a round's branch back
 * is shared by two bytes. The strings compared (names, symbols) are short,
 * rarely co-aligned and mostly differ in their first bytes, so a register
 * at a time would win little. At the end the two bytes are either different or
 * both NUL, and their difference is the answer either way.
 *
 *     for (;; a += 2, b += 2) {
 *         if (a[0] != b[0] || a[0] == 0)
 *             return (u8)a[0] - (u8)b[0];
 *         if (a[1] != b[1] || a[1] == 0)
 *             return (u8)a[1] - (u8)b[1];
 *     }
 *
 * Registers: a0 a, a1 b; t0, t1 their bytes.
 */
#include <asm.h>

    .text
    .globl strcmp
strcmp:
.Lstr_round:
    lbu   t0, 0(a0)
    lbu   t1, 0(a1)
    bne   t0, t1, .Lstr_end
    beqz  t0, .Lstr_end
    lbu   t0, 1(a0)
    lbu   t1, 1(a1)
    addi  a0, a0, 2
    addi  a1, a1, 2
    bne   t0, t1, .Lstr_end
    bnez  t0, .Lstr_round
.Lstr_end:
    sub   a0, t0, t1
    ret
