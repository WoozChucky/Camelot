/* Variable arguments without libc's <stdarg.h>.
 *
 * These map straight onto clang's builtins. Merlin requirement (#15): to
 * compile kprintf, Merlin must accept `...` in prototypes and provide
 * `__builtin_va_list`, `__builtin_va_start`, `__builtin_va_arg` and
 * `__builtin_va_end` with the ILP32 calling convention: variadic arguments are
 * passed in a0-a7 then on the stack exactly like named ones, so a variadic
 * callee spills a0-a7 next to the caller's stack arguments and `va_list` is a
 * plain pointer that walks that area in 4-byte steps. */
#ifndef CAMELOT_VARARGS_H
#define CAMELOT_VARARGS_H

typedef __builtin_va_list va_list;
#define va_start(ap, last) __builtin_va_start(ap, last)
#define va_arg(ap, type) __builtin_va_arg(ap, type)
#define va_end(ap) __builtin_va_end(ap)

#endif
