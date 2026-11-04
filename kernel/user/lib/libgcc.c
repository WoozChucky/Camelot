/* The routines a compiler calls for 64-bit arithmetic on RV32, where a
 * long long is a pair of registers: libgcc's names and calling convention
 * (compiler-rt has the same), so code from merlin and from clang links with
 * them. merlin and clang do the rest of the long long arithmetic inline (with
 * M, a product is mul and mulhu); they call __divdi3, __udivdi3, __moddi3 and
 * __umoddi3, and __muldi3 is here for code built without M.
 *
 * Written in the C both compilers take, with no division, so nothing here
 * calls itself. Division by zero and LLONG_MIN / -1 give RISC-V's results,
 * as the div and rem instructions do on RV64 (spec section 10): a quotient
 * of all ones and the dividend as the remainder, and LLONG_MIN and 0.
 *
 * The kernel has them too (kernel/libgcc.c includes this file). On RV64 a
 * long long is a register, and nothing calls them. */

/* n / d and n % d (in *rem when rem is not 0), unsigned: long division, a
 * bit at a time. */
static unsigned long long udivmod(unsigned long long n, unsigned long long d, unsigned long long *rem) {
    unsigned long long q = 0;
    unsigned long long r = 0;
    int i;
    if (d == 0) {
        if (rem)
            *rem = n;
        return ~0ull;
    }
    for (i = 63; i >= 0; i--) {
        r = (r << 1) | ((n >> i) & 1);
        if (r >= d) {
            r -= d;
            q |= 1ull << i;
        }
    }
    if (rem)
        *rem = r;
    return q;
}

unsigned long long __udivdi3(unsigned long long n, unsigned long long d) {
    return udivmod(n, d, 0);
}

unsigned long long __umoddi3(unsigned long long n, unsigned long long d) {
    unsigned long long r;
    udivmod(n, d, &r);
    return r;
}

/* |v|, as an unsigned long long (LLONG_MIN's is 2^63). */
static unsigned long long magnitude(long long v) {
    return v < 0 ? -(unsigned long long)v : (unsigned long long)v;
}

long long __divdi3(long long a, long long b) {
    unsigned long long q;
    if (b == 0)
        return -1;
    q = udivmod(magnitude(a), magnitude(b), 0);
    /* Converted back, 2^63 is LLONG_MIN: LLONG_MIN / -1 wraps. */
    return (a < 0) != (b < 0) ? (long long)-q : (long long)q;
}

long long __moddi3(long long a, long long b) {
    unsigned long long r;
    if (b == 0)
        return a;
    udivmod(magnitude(a), magnitude(b), &r);
    return a < 0 ? (long long)-r : (long long)r;
}

/* a * b, from 32-bit halves: the low words' product in full (each half of
 * it, 16 bits at a time), plus the cross products' low words in the high
 * word. */
long long __muldi3(long long a, long long b) {
    unsigned long long ua = a;
    unsigned long long ub = b;
    unsigned int alo = (unsigned int)ua;
    unsigned int ahi = (unsigned int)(ua >> 32);
    unsigned int blo = (unsigned int)ub;
    unsigned int bhi = (unsigned int)(ub >> 32);
    unsigned int a0 = alo & 0xffff, a1 = alo >> 16;
    unsigned int b0 = blo & 0xffff, b1 = blo >> 16;
    unsigned long long lo = (unsigned long long)(a0 * b0) + ((unsigned long long)(a1 * b0) << 16) +
                            ((unsigned long long)(a0 * b1) << 16) + ((unsigned long long)(a1 * b1) << 32);
    unsigned int hi = ahi * blo + alo * bhi;
    return (long long)(lo + ((unsigned long long)hi << 32));
}
