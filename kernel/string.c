/* Byte-at-a-time implementations: simple and correct first. Building with
 * -ffreestanding keeps clang from turning these loops back into calls to
 * themselves. */
#include "string.h"

void *memset(void *dst, int c, usize n) {
    u8 *d = (u8 *)dst;
    while (n--) {
        *d++ = (u8)c;
    }
    return dst;
}

void *memcpy(void *dst, const void *src, usize n) {
    u8 *d = (u8 *)dst;
    const u8 *s = (const u8 *)src;
    while (n--) {
        *d++ = *s++;
    }
    return dst;
}

void *memmove(void *dst, const void *src, usize n) {
    u8 *d = (u8 *)dst;
    const u8 *s = (const u8 *)src;
    if (d < s) {
        while (n--) {
            *d++ = *s++;
        }
    } else {
        d += n;
        s += n;
        while (n--) {
            *--d = *--s;
        }
    }
    return dst;
}

int memcmp(const void *a, const void *b, usize n) {
    const u8 *x = (const u8 *)a;
    const u8 *y = (const u8 *)b;
    for (; n; n--, x++, y++) {
        if (*x != *y) {
            return *x < *y ? -1 : 1;
        }
    }
    return 0;
}

usize strlen(const char *s) {
    usize n = 0;
    while (s[n]) {
        n++;
    }
    return n;
}

int strcmp(const char *a, const char *b) {
    while (*a && *a == *b) {
        a++;
        b++;
    }
    return (int)(u8)*a - (int)(u8)*b;
}
