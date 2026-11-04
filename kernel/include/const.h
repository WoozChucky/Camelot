/* Constants that C and assembly share (Linux's include/uapi/linux/const.h).
 *
 * A header that assembly includes too (a .S file, or the linker script,
 * kernel.lds.S) keeps its C, the types, structures and prototypes, under
 * #ifndef __ASSEMBLER__, which the preprocessor defines for a .S file, and
 * its constants outside, so one definition serves both (Linux's
 * asm-offsets idea, without the generated file: the layouts that assembly
 * reads are spelled out and checked by _Static_assert on the C side).
 *
 * A constant that C needs as an unsigned long, such as a bit mask that must
 * keep the upper half of a 64-bit address, is written _UL(4096): 4096ul in
 * C, plain 4096 in assembly, which has no integer suffixes. */
#ifndef CAMELOT_CONST_H
#define CAMELOT_CONST_H

#ifdef __ASSEMBLER__
#define _AC(x, suffix) x
#else
#define __AC(x, suffix) (x##suffix)
#define _AC(x, suffix) __AC(x, suffix)
#endif

#define _U(x) _AC(x, u)
#define _UL(x) _AC(x, ul)
#define _ULL(x) _AC(x, ull)

#endif
