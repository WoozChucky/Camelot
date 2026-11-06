/*
 * void *memcpy(void *dst, const void *src, usize n): the n bytes at src
 * copied to dst, which must not overlap them (memmove.S is for those that
 * may); returns dst. The kernel's memcpy and the user library's
 * (kernel/user/lib/memcpy.S includes this file).
 *
 * A register at a time, the way Linux's riscv memcpy and the C libraries
 * do it. Excalibur traps on a misaligned load or store (spec section 7),
 * so whole registers are copied only when dst and src are co-aligned
 * (equally far past a register boundary): then the bytes up to the
 * boundary one at a time, whole registers eight to a round, single
 * registers, and the bytes after the last boundary. Pointers that are not
 * co-aligned never reach a boundary together: memmove copies those a
 * register at a time from pairs of aligned loads, shifted, and memcpy
 * hands them to it (when they do not overlap, memmove copies forwards, as
 * memcpy would). Fewer than 2 * SZREG bytes go a byte at a time.
 *
 * In assembly because merlin's C keeps every value on the stack: a
 * register's load and store would be some twenty instructions.
 *
 *     const u8 *s = src, *end = s + n;
 *     u8 *d = dst;
 *     if (n >= 2 * SZREG) {
 *         if ((uintptr)d % SZREG != (uintptr)s % SZREG)
 *             return memmove(dst, src, n);
 *         while ((uintptr)s % SZREG != 0)
 *             *d++ = *s++;
 *         const u8 *last = round_down(end, SZREG);
 *         const u8 *rounds = s + (last - s) / (8 * SZREG) * (8 * SZREG);
 *         for (; s != rounds; s += 8 * SZREG, d += 8 * SZREG)
 *             copy eight registers, four loads then their four stores, twice;
 *         for (; s != last; s += SZREG, d += SZREG)
 *             *(usize *)d = *(const usize *)s;
 *     }
 *     while (s != end)
 *         *d++ = *s++;
 *     return dst;
 *
 * Registers: a0 dst, kept to return; a1 the next source byte; t0 the next
 * destination byte; t1 the source's end; t2 scratch, then where the
 * rounds stop; t4 the source's last register boundary; a2 to a5 and t3
 * the values copied (n is not needed once t1 is set).
 */
#include <asm.h>

    .text
    .globl memcpy
memcpy:
    mv    t0, a0
    add   t1, a1, a2
    li    t2, 2*SZREG
    bltu  a2, t2, .Lcpy_bytes
    xor   t2, a0, a1
    andi  t2, t2, SZREG-1
    bnez  t2, .Lcpy_not_coaligned

.Lcpy_head:
    andi  t2, a1, SZREG-1
    beqz  t2, .Lcpy_aligned
    lb    t3, 0(a1)
    sb    t3, 0(t0)
    addi  a1, a1, 1
    addi  t0, t0, 1
    j     .Lcpy_head

.Lcpy_aligned:
    andi  t4, t1, -SZREG
    sub   t2, t4, a1
    andi  t2, t2, -(8*SZREG)
    add   t2, a1, t2
    beq   a1, t2, .Lcpy_singles
.Lcpy_round:
    REG_L a2, 0(a1)
    REG_L a3, 1*SZREG(a1)
    REG_L a4, 2*SZREG(a1)
    REG_L a5, 3*SZREG(a1)
    REG_S a2, 0(t0)
    REG_S a3, 1*SZREG(t0)
    REG_S a4, 2*SZREG(t0)
    REG_S a5, 3*SZREG(t0)
    REG_L a2, 4*SZREG(a1)
    REG_L a3, 5*SZREG(a1)
    REG_L a4, 6*SZREG(a1)
    REG_L a5, 7*SZREG(a1)
    REG_S a2, 4*SZREG(t0)
    REG_S a3, 5*SZREG(t0)
    REG_S a4, 6*SZREG(t0)
    REG_S a5, 7*SZREG(t0)
    addi  a1, a1, 8*SZREG
    addi  t0, t0, 8*SZREG
    bne   a1, t2, .Lcpy_round
.Lcpy_singles:
    beq   a1, t4, .Lcpy_bytes
.Lcpy_single:
    REG_L a2, 0(a1)
    REG_S a2, 0(t0)
    addi  a1, a1, SZREG
    addi  t0, t0, SZREG
    bne   a1, t4, .Lcpy_single

.Lcpy_bytes:
    beq   a1, t1, .Lcpy_done
.Lcpy_byte:
    lb    t3, 0(a1)
    sb    t3, 0(t0)
    addi  a1, a1, 1
    addi  t0, t0, 1
    bne   a1, t1, .Lcpy_byte
.Lcpy_done:
    ret

.Lcpy_not_coaligned:
    /* a0, a1 and a2 are still the arguments. */
    j     memmove
