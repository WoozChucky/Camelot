/* Strings, a byte at a time. strlen and strcmp, and memset, memcpy,
 * memmove and memcmp, are the kernel's assembly (strlen.S, strcmp.S and
 * mem*.S here include kernel's). */
#include "../user.h"

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
