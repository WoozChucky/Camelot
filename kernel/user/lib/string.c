/* Strings and memory: a byte at a time, simple first. */
#include "../user.h"

size_t strlen(const char *s) {
    size_t n = 0;
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
    return (int)(uchar)*a - (int)(uchar)*b;
}

int strncmp(const char *a, const char *b, size_t n) {
    for (; n > 0; n--, a++, b++) {
        if (*a != *b || *a == 0) {
            return (int)(uchar)*a - (int)(uchar)*b;
        }
    }
    return 0;
}

char *strcpy(char *dst, const char *src) {
    char *d = dst;
    while ((*d++ = *src++) != 0) {
    }
    return dst;
}

char *strcat(char *dst, const char *src) {
    strcpy(dst + strlen(dst), src);
    return dst;
}

char *strchr(const char *s, int c) {
    for (;; s++) {
        if (*s == (char)c) {
            return (char *)s;
        }
        if (*s == 0) {
            return NULL;
        }
    }
}

char *strrchr(const char *s, int c) {
    const char *last = NULL;
    for (;; s++) {
        if (*s == (char)c) {
            last = s;
        }
        if (*s == 0) {
            return (char *)last;
        }
    }
}

char *strstr(const char *haystack, const char *needle) {
    size_t n = strlen(needle);
    for (; *haystack; haystack++) {
        if (strncmp(haystack, needle, n) == 0) {
            return (char *)haystack;
        }
    }
    /* The empty needle is found at the end of an empty haystack too. */
    return n == 0 ? (char *)haystack : NULL;
}

void *memset(void *dst, int c, size_t n) {
    uchar *d = (uchar *)dst;
    while (n--) {
        *d++ = (uchar)c;
    }
    return dst;
}

void *memcpy(void *dst, const void *src, size_t n) {
    uchar *d = (uchar *)dst;
    const uchar *s = (const uchar *)src;
    while (n--) {
        *d++ = *s++;
    }
    return dst;
}

/* Like memcpy, but the two may overlap: copies backwards when `dst` is
 * above `src`, so no byte is overwritten before it is copied. */
void *memmove(void *dst, const void *src, size_t n) {
    uchar *d = (uchar *)dst;
    const uchar *s = (const uchar *)src;
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

int memcmp(const void *a, const void *b, size_t n) {
    const uchar *x = (const uchar *)a;
    const uchar *y = (const uchar *)b;
    for (; n; n--, x++, y++) {
        if (*x != *y) {
            return *x < *y ? -1 : 1;
        }
    }
    return 0;
}
