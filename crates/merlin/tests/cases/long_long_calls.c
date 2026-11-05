// expect: 21
// long long arguments, results and variadic arguments: each is a register,
// and so is a struct holding one. (On RV32, until #159, each was a pair of
// words, which these calls placed at every odd and even word.)
typedef __builtin_va_list va_list;
#define va_start(ap, last) __builtin_va_start(ap, last)
#define va_arg(ap, type) __builtin_va_arg(ap, type)
#define va_end(ap) __builtin_va_end(ap)

struct ll {
    long long v;
};

struct ill {
    int i;
    long long v;
};

long long add3(long long a, int b, long long c) {
    return a + b + c;
}

unsigned long long high(unsigned long long x) {
    return x >> 32;
}

/* The eighth argument: a7. */
long long last(int a, int b, int c, int d, int e, int f, int g, long long h) {
    return h - (a + b + c + d + e + f + g);
}

/* Past the registers: an int, then a pair from an even word. */
long long stacked(long long a, long long b, long long c, long long d, int e, long long f) {
    return a + b + c + d + e + f;
}

struct ll twice(struct ll s) {
    s.v *= 2;
    return s;
}

long long struct_sum(struct ill s, struct ll t) {
    return s.i + s.v + t.v;
}

/* Letters of fmt: i an int, q a long long, s a struct ll. */
long long vsum(const char *fmt, ...) {
    va_list ap;
    long long sum = 0;
    va_start(ap, fmt);
    for (; *fmt; fmt++) {
        if (*fmt == 'i') {
            sum += va_arg(ap, int);
        } else if (*fmt == 'q') {
            sum += va_arg(ap, long long);
        } else {
            struct ll s = va_arg(ap, struct ll);
            sum += s.v;
        }
    }
    va_end(ap);
    return sum;
}

/* Seven named ints: the variadic pair skips a7. */
long long vlate(int a, int b, int c, int d, int e, int f, int g, ...) {
    va_list ap;
    long long x;
    va_start(ap, g);
    x = va_arg(ap, long long);
    va_end(ap);
    return x + a + b + c + d + e + f + g;
}

long long (*pick(int which))(long long, int, long long) {
    return which ? add3 : 0;
}

int main() {
    int r = 0;
    long long big = 0x100000000LL;
    struct ll s = {5000000000LL};
    struct ill t = {7, -3000000000LL};
    r += add3(big, -1, big) == 0x1ffffffffLL;                         // 1
    r += add3(-1, -1, -1) == -3;                                     // 2
    r += high(0xabcdef0123456789ULL) == 0xabcdef01ULL;               // 3
    r += last(1, 2, 3, 4, 5, 6, 7, big) == big - 28;                  // 4
    r += last(0, 0, 0, 0, 0, 0, 0, -big) == -big;                    // 5
    r += stacked(1, 2, 3, 4, 5, big) == big + 15;                    // 6
    r += stacked(big, big, big, big, -1, big) == 5 * big - 1;        // 7
    r += twice(s).v == 10000000000LL;                                // 8
    r += struct_sum(t, s) == 7 + 2000000000LL;                       // 9
    r += vsum("") == 0;                                              // 10
    r += vsum("q", big) == big;                                      // 11
    r += vsum("iq", 1, big) == big + 1;                              // 12: an aligned pair
    r += vsum("iqiq", 1, big, 2, -big) == 3;                         // 13
    r += vsum("isq", 3, s, -1LL) == 5000000002LL;                    // 14
    r += vsum("iiiiiiq", 1, 2, 3, 4, 5, 6, big) == big + 21;         // 15: on the stack
    r += vsum("iiiiiiiq", 1, 2, 3, 4, 5, 6, 7, big) == big + 28;     // 16
    r += vlate(1, 2, 3, 4, 5, 6, 7, big) == big + 28;                // 17: skipping a7
    r += pick(1)(big, 1, big) == 2 * big + 1;                         // 18: through a pointer
    r += add3(add3(1, 2, 3), 4, add3(5, 6, big)) == big + 21;        // 19: nested
    r += 1 + (int)(add3(big, 2, 3) - big) == 6;                      // 20: with temporaries pushed
    r += (vsum("q", 0xffffffffLL) + 1) == big;                       // 21
    return r;
}
