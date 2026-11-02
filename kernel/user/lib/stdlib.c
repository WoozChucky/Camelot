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
    uint n = 0;
    const char *first = p;
    if (base >= 2 && base <= 36) {
        while (digit_value(*p) < base) {
            n = n * (uint)base + (uint)digit_value(*p);
            p++;
        }
    }
    if (end != NULL) {
        *end = (char *)(p == first ? s : p);
    }
    return negative ? (long)(0u - n) : (long)n;
}
