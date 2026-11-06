/*
 * int memcmp(const void *a, const void *b, usize n): -1, 0 or 1 as the
 * first of the n bytes that differ, read as unsigned, is lower in a, there
 * is none, or it is higher. The kernel's memcmp and the user library's
 * (kernel/user/lib/memcmp.S includes this file).
 *
 * A register at a time where a and b are co-aligned (equally far past a
 * register boundary), as the C libraries compare: the bytes before the
 * boundary one at a time, then whole registers while they are equal.
 * Equal registers are equal bytes; of two that differ, the first
 * differing byte (the lowest, Excalibur being little-endian) is found by
 * shifting both down a byte at a time until their low bytes differ, and
 * decides. The bytes after the last whole register, and everything when
 * the pointers are not co-aligned (they never reach a boundary together,
 * and Excalibur traps on a misaligned load, spec section 7) or there are
 * fewer than 2 * SZREG bytes, go a byte at a time.
 *
 * In assembly because merlin's C keeps every value on the stack, some 30
 * instructions a byte; here a byte, or a whole register, is 6.
 *
 *     const u8 *x = a, *y = b;
 *     if (n >= 2 * SZREG && (uintptr)x % SZREG == (uintptr)y % SZREG) {
 *         for (; (uintptr)x % SZREG != 0; x++, y++, n--)
 *             if (*x != *y)
 *                 return sign(*x - *y);
 *         const u8 *last = x + (n & ~(SZREG - 1));
 *         n %= SZREG;
 *         for (; x != last; x += SZREG, y += SZREG) {
 *             usize u = *(usize *)x, v = *(usize *)y;
 *             if (u != v) {
 *                 while ((u & 0xff) == (v & 0xff))
 *                     u >>= 8, v >>= 8;
 *                 return sign((u & 0xff) - (v & 0xff));
 *             }
 *         }
 *     }
 *     for (; n; n--, x++, y++)
 *         if (*x != *y)
 *             return sign(*x - *y);
 *     return 0;
 *
 * Registers: a0 x, a1 y, a2 the bytes left; t3 where the registers or
 * bytes being compared end; t1, t2 the bytes or registers; t4, t5 their
 * low bytes; t0 scratch.
 */
#include <asm.h>

    .text
    .globl memcmp
memcmp:
    li    t0, 2*SZREG
    bltu  a2, t0, .Lcmp_bytes
    xor   t0, a0, a1
    andi  t0, t0, SZREG-1
    bnez  t0, .Lcmp_bytes

.Lcmp_head:
    andi  t0, a0, SZREG-1
    beqz  t0, .Lcmp_aligned
    lbu   t1, 0(a0)
    lbu   t2, 0(a1)
    bne   t1, t2, .Lcmp_sign
    addi  a0, a0, 1
    addi  a1, a1, 1
    addi  a2, a2, -1
    j     .Lcmp_head

.Lcmp_aligned:
    /* At least 2 * SZREG - (SZREG - 1) bytes are left: one register. */
    andi  t3, a2, -SZREG
    add   t3, a0, t3
    andi  a2, a2, SZREG-1
.Lcmp_word:
    REG_L t1, 0(a0)
    REG_L t2, 0(a1)
    bne   t1, t2, .Lcmp_lanes
    addi  a0, a0, SZREG
    addi  a1, a1, SZREG
    bne   a0, t3, .Lcmp_word

.Lcmp_bytes:
    add   t3, a0, a2
    beq   a0, t3, .Lcmp_equal
.Lcmp_byte:
    lbu   t1, 0(a0)
    lbu   t2, 0(a1)
    bne   t1, t2, .Lcmp_sign
    addi  a0, a0, 1
    addi  a1, a1, 1
    bne   a0, t3, .Lcmp_byte
.Lcmp_equal:
    li    a0, 0
    ret

.Lcmp_lanes:
    /* The registers differ: down a byte at a time to the first that does. */
    andi  t4, t1, 0xff
    andi  t5, t2, 0xff
    srli  t1, t1, 8
    srli  t2, t2, 8
    beq   t4, t5, .Lcmp_lanes
    mv    t1, t4
    mv    t2, t5

.Lcmp_sign:
    /* (t1 > t2) - (t1 < t2): 1, or -1. */
    sltu  a0, t2, t1
    sltu  t0, t1, t2
    sub   a0, a0, t0
    ret
