/* Kernel formatted output.
 *
 * Conversions: %d %u %x %p %s %c and %%. An optional `0` flag and a decimal
 * width may come before the conversion (`%08x`, `%5d`). %d, %u and %x take an
 * int; with `l` a long (a register's width: 32 bits on RV32, 64 on RV64:
 * `%lx` of a uintptr or a usize) and with `ll` a long long (64 bits on both:
 * `%llx` of a u64), as in Linux's printk. %p prints `0x` and every hex digit
 * of a pointer (8 on RV32, 16 on RV64). %s of NULL prints `(null)`. Anything
 * else is copied verbatim. */
#ifndef CAMELOT_KPRINTF_H
#define CAMELOT_KPRINTF_H

#include "types.h"
#include "varargs.h"

/* How the kernel prints a register-wide value (an address, a CSR, a saved
 * register) in hex, every digit: "%08lx" on RV32, "%016lx" on RV64, for a
 * uintptr. REG_FMT is the same with its 0x. PTE_FMT is a pte_t (vm.h): a u32
 * on RV32, a u64 (long long) on RV64. */
#if __riscv_xlen == 64
#define REG_HEX "%016lx"
#define PTE_FMT "0x%016llx"
#else
#define REG_HEX "%08lx"
#define PTE_FMT "0x%08x"
#endif
#define REG_FMT "0x" REG_HEX

/* Sink for formatted characters. */
typedef void (*kputc_fn)(char c, void *ctx);

/* Formats into `put`; returns the number of characters produced. */
int kvformat(kputc_fn put, void *ctx, const char *fmt, va_list ap);

/* Prints to the UART; returns the number of characters printed. */
int kprintf(const char *fmt, ...);

/* Formats into `buf`, which always ends with a NUL when `size` > 0. Returns
 * the full length, which is >= `size` when the output was truncated. */
int ksnprintf(char *buf, usize size, const char *fmt, ...);

#endif
