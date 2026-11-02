/* Numbers from strings: atoi and strtol. */
#include "../user.h"

int atoi(const char *s) {
    int negative = *s == '-';
    if (negative) {
        s++;
    }
    uint n = 0;
    while (*s >= '0' && *s <= '9') {
        n = n * 10 + (uint)(*s - '0');
        s++;
    }
    /* Negated as unsigned, so "-2147483648" works. */
    return negative ? (int)(0u - n) : (int)n;
}

/* The value of digit `c` in any base up to 36, or 36 if it is none. */
static int digit_value(char c) {
    if (c >= '0' && c <= '9') {
        return c - '0';
    }
    if (c >= 'a' && c <= 'z') {
        return c - 'a' + 10;
    }
    if (c >= 'A' && c <= 'Z') {
        return c - 'A' + 10;
    }
    return 36;
}

long strtol(const char *s, char **end, int base) {
    const char *p = s;
    while (*p == ' ' || *p == '\t') {
        p++;
    }
    int negative = 0;
    if (*p == '-' || *p == '+') {
        negative = *p == '-';
        p++;
    }
    /* "0x" counts only if a hex digit follows it: "0xg" is the number 0,
     * ending at the x. */
    if ((base == 0 || base == 16) && p[0] == '0' && (p[1] == 'x' || p[1] == 'X') && digit_value(p[2]) < 16) {
        p += 2;
        base = 16;
    } else if (base == 0) {
        base = p[0] == '0' ? 8 : 10;
    }
    /* The magnitude may reach LONG_MAX, or one more for a negative number
     * (LONG_MIN). Past that the value stays at the limit, but the digits
     * are still read to their end. */
    uint limit = negative ? 0x80000000u : 0x7fffffffu;
    uint n = 0;
    const char *first = p;
    if (base >= 2 && base <= 36) {
        while (digit_value(*p) < base) {
            uint d = (uint)digit_value(*p);
            /* n * base + d > limit, without overflowing to find out. */
            if (n > (limit - d) / (uint)base) {
                n = limit;
            } else {
                n = n * (uint)base + d;
            }
            p++;
        }
    }
    if (end != NULL) {
        *end = (char *)(p == first ? s : p);
    }
    /* Negated as unsigned, so LONG_MIN works. */
    return negative ? (long)(0u - n) : (long)n;
}
