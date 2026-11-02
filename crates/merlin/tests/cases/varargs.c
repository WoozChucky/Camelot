// expect: 77
// Variadic functions: the arguments `...` takes, in registers and then on the
// stack, read with clang's builtins (what <stdarg.h> is made of).
#define va_list __builtin_va_list
#define va_start(ap, last) __builtin_va_start(ap, last)
#define va_arg(ap, type) __builtin_va_arg(ap, type)
#define va_end(ap) __builtin_va_end(ap)
#define va_copy(dst, src) __builtin_va_copy(dst, src)

struct pair {
    short a, b;
    int c;
};

struct big {
    int v[4];
};

/* n ints. */
int sum(int n, ...) {
    va_list ap;
    int s = 0;
    va_start(ap, n);
    while (n-- > 0)
        s += va_arg(ap, int);
    va_end(ap);
    return s;
}

/* A tiny printf: each letter of fmt says what the next argument is. */
int mix(const char *fmt, ...) {
    va_list ap, again;
    int s = 0;
    const char *p;
    va_start(ap, fmt);
    va_copy(again, ap);
    for (p = fmt; *p; p++) {
        if (*p == 'i') {
            s += va_arg(ap, int);
        } else if (*p == 'c') {
            s += (signed char)va_arg(ap, int); /* a char arrives as an int */
        } else if (*p == 'u') {
            s += va_arg(ap, unsigned) % 100;
        } else if (*p == 's') {
            s += *va_arg(ap, char *);
        } else if (*p == 'p') {
            struct pair q = va_arg(ap, struct pair);
            s += q.a + q.b + q.c;
        } else if (*p == 'b') {
            struct big b = va_arg(ap, struct big);
            s += b.v[0] + b.v[3];
        }
    }
    /* The copy reads the first argument again. */
    if (*fmt == 'i')
        s += 1000 * va_arg(again, int);
    va_end(again);
    va_end(ap);
    return s;
}

/* Seven named parameters: the pair straddles a7 and the stack. */
int late(int a, int b, int c, int d, int e, int f, int g, ...) {
    va_list ap;
    struct pair q;
    int x;
    va_start(ap, g);
    q = va_arg(ap, struct pair);
    x = va_arg(ap, int);
    va_end(ap);
    return a + b + c + d + e + f + g + q.a + q.b + q.c + x;
}

/* Every named parameter is in a register or beyond: va_start points at
 * the caller's stack words. */
int beyond(int a, int b, int c, int d, int e, int f, int g, int h, int i, ...) {
    va_list ap;
    int s;
    va_start(ap, i);
    s = va_arg(ap, int) * 10;
    s += va_arg(ap, int);
    va_end(ap);
    return s - (a + b + c + d + e + f + g + h + i);
}

/* Passes its own list on. */
int vsum(int n, va_list ap) {
    int s = 0;
    while (n-- > 0)
        s += va_arg(ap, int);
    return s;
}

int forward(int n, ...) {
    va_list ap;
    int s;
    va_start(ap, n);
    s = vsum(n, ap);
    va_end(ap);
    return s;
}

int main(void) {
    struct pair q = {1, 2, 3};
    struct big b = {{10, 20, 30, 40}};
    signed char c = -2;
    unsigned short us = 65535;
    int (*fp)(int, ...) = sum;
    int r = 0;
    if (sum(0) != 0)
        return 1;
    if (sum(3, 1, 2, 3) != 6)
        return 2;
    /* Twelve: past a7 onto the stack. */
    if (sum(12, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12) != 78)
        return 3;
    if (fp(2, 40, 2) != 42)
        return 4;
    /* char and unsigned short are promoted to int; a struct goes whole. */
    if (mix("ciupb", c, 5, 4000000000u, q, b) != -2 + 5 + 0 + 6 + 50)
        return 5;
    if (mix("is", 7, "A") != 7 + 'A' + 7000)
        return 6;
    if (mix("pppp", q, q, q, q) != 24)
        return 7;
    if (late(1, 2, 3, 4, 5, 6, 7, q, 100) != 28 + 6 + 100)
        return 8;
    if (beyond(1, 2, 3, 4, 5, 6, 7, 8, 9, 7, 3) != 73 - 45)
        return 9;
    if (forward(4, 1, 2, 3, 4) != 10)
        return 10;
    r = sum(2, us, c);
    return r - 65533 + sum(4, 1, sum(1, 1), 3, sum(2, 1, 1)) + 70;
}
