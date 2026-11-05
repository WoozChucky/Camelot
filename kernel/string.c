/* Byte-at-a-time implementations, simple and correct, but for memset,
 * which zeroes every page the kernel allocates (kzalloc()) and so is worth
 * register-wide stores. Building with -ffreestanding keeps clang from
 * turning these loops back into calls to themselves. */
#include "string.h"

/* As Linux's riscv memset (arch/riscv/lib/memset.S), which its clear_page()
 * is: bytes up to a register-size boundary, then whole registers, unrolled
 * (eight stores a round here, Linux's thirty-two), then the bytes left. A
 * page of zeros is 512 stores on RV64 (1024 on RV32), not 4096. */
void *memset(void *dst, int c, usize n) {
    u8 *d = (u8 *)dst;
    u8 byte = (u8)c;
    if (n >= 2 * sizeof(usize)) {
        while ((uintptr)d % sizeof(usize) != 0) {
            *d++ = byte;
            n--;
        }
        /* The byte in every byte of a register: 0x01...01 times it. */
        usize word = (usize)-1 / 0xff * byte;
        usize *w = (usize *)d;
        while (n >= 8 * sizeof(usize)) {
            w[0] = word;
            w[1] = word;
            w[2] = word;
            w[3] = word;
            w[4] = word;
            w[5] = word;
            w[6] = word;
            w[7] = word;
            w += 8;
            n -= 8 * sizeof(usize);
        }
        while (n >= sizeof(usize)) {
            *w++ = word;
            n -= sizeof(usize);
        }
        d = (u8 *)w;
    }
    while (n--) {
        *d++ = byte;
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
