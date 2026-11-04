/*
 * asm.h: the instructions, directives and sizes an assembly program writes
 * differently on RV32 and RV64, under one name each, so that one .S source
 * builds for both widths (#193). The examples, Grail's boot ROM and, later,
 * the kernel include it with -I include:
 *
 *   #include <asm.h>
 *
 *       addi   sp, sp, -16
 *       REG_S  ra, 16-SZREG(sp)        # sw on RV32, sd on RV64
 *
 * It uses the names Linux's arch/riscv/include/asm/asm.h gives these, so
 * code reads the same, and only those this repository's sources use; LWU,
 * ADDW and ADDIW, for 32-bit values, are this project's own.
 *
 * A .S file goes through the C preprocessor before the assembler (merlin -E,
 * which cargo xtask runs, or clang), and the preprocessor defines
 * __riscv_xlen as 32 or 64 from -march. Where the two widths need other
 * code, not just other names, a program says so itself with
 * #if __riscv_xlen == 64.
 */
#ifndef CAMELOT_ASM_H
#define CAMELOT_ASM_H

#if __riscv_xlen == 64

#define REG_L ld            /* load a register's worth */
#define REG_S sd            /* store a register's worth */
#define SZREG 8             /* a register's size in bytes */
#define RISCV_PTR .dword    /* the directive that holds a pointer */
#define RISCV_SZPTR 8       /* a pointer's size in bytes */
#define RISCV_LGPTR 3       /* log2 of it: an index shifted by it is an offset */

/* This project's own: a 32-bit value in a register. */
#define LWU lwu             /* load a 32-bit word, zero-extended */
#define ADDW addw           /* add, the 32-bit sum kept sign-extended */
#define ADDIW addiw         /* add an immediate, the same way */

#elif __riscv_xlen == 32

#define REG_L lw
#define REG_S sw
#define SZREG 4
#define RISCV_PTR .word
#define RISCV_SZPTR 4
#define RISCV_LGPTR 2

#define LWU lw
#define ADDW add
#define ADDIW addi

#else
#error "asm.h: __riscv_xlen is neither 32 nor 64 (is this a RISC-V -march?)"
#endif

#endif /* CAMELOT_ASM_H */
