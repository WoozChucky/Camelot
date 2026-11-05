/*
 * asm.h: the names an assembly program gives a register's worth and a
 * pointer, as Linux's arch/riscv/include/asm/asm.h gives them, so that the
 * kernel's, the firmware's and the examples' assembly reads as Linux's does:
 *
 *   #include <asm.h>
 *
 *       addi   sp, sp, -16
 *       REG_S  ra, 16-SZREG(sp)        # sd: a saved register
 *
 * Excalibur is RV64 only, so each name has one meaning; they stay because
 * they say what a value is (a saved register, a pointer) where ld, sd and
 * .dword say only how big it is, and because Linux's code, which this
 * project's follows, is written with them. Only the names this repository's
 * sources use are here.
 *
 * A .S file goes through the C preprocessor before the assembler (merlin -E,
 * which cargo xtask runs, or clang), and the preprocessor defines
 * __riscv_xlen from -march: 64.
 */
#ifndef CAMELOT_ASM_H
#define CAMELOT_ASM_H

#if __riscv_xlen != 64
#error "asm.h: Excalibur is RV64 (is this an rv64 -march?)"
#endif

#define REG_L ld            /* load a register's worth */
#define REG_S sd            /* store a register's worth */
#define SZREG 8             /* a register's size in bytes */
#define RISCV_PTR .dword    /* the directive that holds a pointer */
#define RISCV_SZPTR 8       /* a pointer's size in bytes */
#define RISCV_LGPTR 3       /* log2 of it: an index shifted by it is an offset */

#endif /* CAMELOT_ASM_H */
