/* Kernel formatted output.
 *
 * Conversions: %d %u %x %p %s %c and %%. An optional `0` flag and a decimal
 * width may come before the conversion (`%08x`, `%5d`). An `l` length modifier
 * is accepted and ignored, since long is 32 bits. %p prints `0x` and eight hex
 * digits. %s of NULL prints `(null)`. Anything else is copied verbatim. */
#ifndef CAMELOT_KPRINTF_H
#define CAMELOT_KPRINTF_H

#include "types.h"
#include "varargs.h"

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
