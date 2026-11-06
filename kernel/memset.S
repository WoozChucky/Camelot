/*
 * void *memset(void *dst, int c, usize n): the n bytes at dst set to c's
 * low byte; returns dst. The kernel's memset (kzalloc() zeroes every page
 * with it) and the user library's (kernel/user/lib/memset.S includes this
 * file).
 *
 * A register at a time, the way Linux's riscv memset and the C libraries
 * do it: the bytes up to the first register boundary one at a time, then
 * whole registers, eight to a round, then single registers, then the
 * bytes after the last boundary. Excalibur traps on a misaligned store
 * (spec section 7), so a register is only ever stored at a multiple of
 * SZREG. Fewer than 2 * SZREG bytes are set one at a time: finding the
 * boundaries would cost more than it saves.
 *
 * In assembly because merlin's C keeps every value on the stack, which
 * makes `*w++ = word` a dozen instructions; a 4096-byte page here is 64
 * rounds of ten instructions.
 *
 *     u8 *p = dst, *end = p + n;
 *     if (n >= 2 * SZREG) {
 *         while ((uintptr)p % SZREG != 0)
 *             *p++ = c;
 *         usize word = (u8)c * 0x0101010101010101;   the byte in each lane
 *         u8 *last = round_down(end, SZREG);
 *         u8 *rounds = p + (last - p) / (8 * SZREG) * (8 * SZREG);
 *         for (; p != rounds; p += 8 * SZREG)
 *             eight stores of word;
 *         for (; p != last; p += SZREG)
 *             *(usize *)p = word;
 *     }
 *     while (p != end)
 *         *p++ = c;
 *     return dst;
 *
 * Registers: a0 dst, kept to return; a1 the byte; a3 the word; t0 the
 * next byte to set; t1 end; t2 scratch, then where the rounds stop; t3
 * last.
 */
#include <asm.h>

    .text
    .globl memset
memset:
    mv    t0, a0
    add   t1, a0, a2
    andi  a1, a1, 0xff
    li    t2, 2*SZREG
    bltu  a2, t2, .Lset_bytes

.Lset_head:
    andi  t2, t0, SZREG-1
    beqz  t2, .Lset_aligned
    sb    a1, 0(t0)
    addi  t0, t0, 1
    j     .Lset_head

.Lset_aligned:
    /* The byte times 0x0101...01 is the byte in all eight lanes. */
    li    t2, 0x0101010101010101
    mul   a3, a1, t2
    andi  t3, t1, -SZREG
    sub   t2, t3, t0
    andi  t2, t2, -(8*SZREG)
    add   t2, t0, t2
    beq   t0, t2, .Lset_singles
.Lset_round:
    REG_S a3, 0(t0)
    REG_S a3, 1*SZREG(t0)
    REG_S a3, 2*SZREG(t0)
    REG_S a3, 3*SZREG(t0)
    REG_S a3, 4*SZREG(t0)
    REG_S a3, 5*SZREG(t0)
    REG_S a3, 6*SZREG(t0)
    REG_S a3, 7*SZREG(t0)
    addi  t0, t0, 8*SZREG
    bne   t0, t2, .Lset_round
.Lset_singles:
    beq   t0, t3, .Lset_bytes
.Lset_single:
    REG_S a3, 0(t0)
    addi  t0, t0, SZREG
    bne   t0, t3, .Lset_single

.Lset_bytes:
    /* a1 is the byte, for the short ones and the tail alike. */
    beq   t0, t1, .Lset_done
.Lset_byte:
    sb    a1, 0(t0)
    addi  t0, t0, 1
    bne   t0, t1, .Lset_byte
.Lset_done:
    ret
