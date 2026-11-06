/*
 * void *memmove(void *dst, const void *src, usize n): the n bytes at src
 * copied to dst, which may overlap them; returns dst. The kernel's memmove
 * and the user library's (kernel/user/lib/memmove.S includes this file);
 * memcpy.S hands it the copies whose pointers are not co-aligned.
 *
 * Direction: a forward copy overwrites a source byte before reading it
 * only when dst is inside (src, src + n), so that case alone is copied
 * from the ends down. One unsigned comparison tells: dst - src < n.
 *
 * Each direction works a register of dst at a time, the way Linux's riscv
 * memmove and the C libraries do it, a byte at a time below 2 * SZREG
 * bytes. First the bytes up to dst's register boundary (going down, its
 * end's), one at a time; then whole registers of dst; then the bytes left.
 * Excalibur traps on a misaligned load or store (spec section 7), so the
 * registers of src are loaded at aligned addresses too:
 *
 *  - src on a boundary as well (co-aligned): a load and a store each.
 *
 *  - src `off` bytes past one: a register of dst is the last 8 - off
 *    bytes of one aligned register of src and the first off bytes of the
 *    next. Little-endian, the lower address is the lower byte, so
 *
 *        lo = *(usize *)(s - off);
 *        while (d != stop) {
 *            hi = *(usize *)(s - off + SZREG);
 *            *(usize *)d = lo >> (8 * off) | hi << (8 * (SZREG - off));
 *            lo = hi;
 *            s += SZREG, d += SZREG;
 *        }
 *
 *    one load, three ALU instructions and a store for 8 bytes, where the
 *    bytes would take eight of each. Going down it is the same with the
 *    roles of the two loads swapped. An aligned load stays within its 8
 *    bytes, so it never reaches into a page the copy does not touch; it
 *    may read bytes just outside src, which the shifts drop. With the
 *    pointers overlapping, every register is loaded before a store can
 *    reach it: the loads run ahead of the stores in the copy's direction.
 *
 * In assembly because merlin's C keeps every value on the stack.
 *
 * Registers: a0 dst, kept to return; a1 the next source byte (forwards)
 * or the end of the source bytes left (backwards); t2 the same for dst;
 * t4 dst's end; t6 where dst's whole registers stop; t1 scratch, then
 * off; a3 and a4 the right and left shifts; a5 the aligned source
 * register's address; a6, a7 the two loaded registers; t5 a value copied
 * or a shifted half.
 */
#include <asm.h>

    .text
    .globl memmove
memmove:
    beq   a0, a1, .Lmv_done
    sub   t1, a0, a1
    bltu  t1, a2, .Lmv_down

    /* ---- Forwards ---- */
    mv    t2, a0
    add   t4, a0, a2
    li    t1, 2*SZREG
    bltu  a2, t1, .Lup_bytes
.Lup_head:
    andi  t1, t2, SZREG-1
    beqz  t1, .Lup_aligned
    lb    t5, 0(a1)
    sb    t5, 0(t2)
    addi  a1, a1, 1
    addi  t2, t2, 1
    j     .Lup_head
.Lup_aligned:
    /* dst is on a boundary, with at least one whole register before t6. */
    andi  t6, t4, -SZREG
    andi  t1, a1, SZREG-1
    bnez  t1, .Lup_shifted
.Lup_word:
    REG_L t5, 0(a1)
    REG_S t5, 0(t2)
    addi  a1, a1, SZREG
    addi  t2, t2, SZREG
    bne   t2, t6, .Lup_word
    j     .Lup_bytes
.Lup_shifted:
    slli  a3, t1, 3
    li    a4, 8*SZREG
    sub   a4, a4, a3
    sub   a5, a1, t1
    REG_L a6, 0(a5)
.Lup_pair:
    REG_L a7, SZREG(a5)
    srl   a6, a6, a3
    sll   t5, a7, a4
    or    a6, a6, t5
    REG_S a6, 0(t2)
    mv    a6, a7
    addi  a5, a5, SZREG
    addi  t2, t2, SZREG
    bne   t2, t6, .Lup_pair
    /* The source byte that goes to t2. */
    add   a1, a5, t1
.Lup_bytes:
    beq   t2, t4, .Lmv_done
.Lup_byte:
    lb    t5, 0(a1)
    sb    t5, 0(t2)
    addi  a1, a1, 1
    addi  t2, t2, 1
    bne   t2, t4, .Lup_byte
.Lmv_done:
    ret

    /* ---- Backwards: dst inside (src, src + n) ---- */
.Lmv_down:
    add   a1, a1, a2
    add   t2, a0, a2
    li    t1, 2*SZREG
    bltu  a2, t1, .Ldown_bytes
.Ldown_head:
    andi  t1, t2, SZREG-1
    beqz  t1, .Ldown_aligned
    lb    t5, -1(a1)
    sb    t5, -1(t2)
    addi  a1, a1, -1
    addi  t2, t2, -1
    j     .Ldown_head
.Ldown_aligned:
    /* t6: dst rounded up to a boundary, where going down stops. */
    addi  t6, a0, SZREG-1
    andi  t6, t6, -SZREG
    andi  t1, a1, SZREG-1
    bnez  t1, .Ldown_shifted
.Ldown_word:
    REG_L t5, -SZREG(a1)
    REG_S t5, -SZREG(t2)
    addi  a1, a1, -SZREG
    addi  t2, t2, -SZREG
    bne   t2, t6, .Ldown_word
    j     .Ldown_bytes
.Ldown_shifted:
    slli  a3, t1, 3
    li    a4, 8*SZREG
    sub   a4, a4, a3
    sub   a5, a1, t1
    REG_L a7, 0(a5)
.Ldown_pair:
    REG_L a6, -SZREG(a5)
    sll   a7, a7, a4
    srl   t5, a6, a3
    or    a7, a7, t5
    REG_S a7, -SZREG(t2)
    mv    a7, a6
    addi  a5, a5, -SZREG
    addi  t2, t2, -SZREG
    bne   t2, t6, .Ldown_pair
    add   a1, a5, t1
.Ldown_bytes:
    beq   t2, a0, .Lmv_done
.Ldown_byte:
    lb    t5, -1(a1)
    sb    t5, -1(t2)
    addi  a1, a1, -1
    addi  t2, t2, -1
    bne   t2, a0, .Ldown_byte
    ret
