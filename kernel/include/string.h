/* The handful of string and memory routines the kernel needs, in
 * assembly: memset.S, memcpy.S, memmove.S and memcmp.S work a register
 * at a time where they can, as Linux's and the C libraries' do, strlen.S
 * and strcmp.S two bytes a round (spec section 10, "Kernel memory").
 * clang may call memcpy/memset/memmove/memcmp on its own even with
 * -ffreestanding (struct copies, large zero-initialisations), so those
 * names must exist. */
#ifndef CAMELOT_STRING_H
#define CAMELOT_STRING_H

#include "types.h"

void *memset(void *dst, int c, usize n);
void *memcpy(void *dst, const void *src, usize n);
void *memmove(void *dst, const void *src, usize n);
int memcmp(const void *a, const void *b, usize n);
usize strlen(const char *s);
int strcmp(const char *a, const char *b);

#endif
