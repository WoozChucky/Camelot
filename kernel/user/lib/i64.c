/* 64-bit integers for C without long long (user.h): two words in two's
 * complement, with Rust's i64 semantics where they differ from C's
 * (wrapping arithmetic, checked division, shifts by the low 6 bits). */
#include "../user.h"

i64 i64_of(int v) {
    i64 r;
    r.lo = (uint)v;
    r.hi = v < 0 ? 0xffffffffu : 0;
    return r;
}

i64 i64_u(uint v) {
    i64 r;
    r.lo = v;
    r.hi = 0;
    return r;
}

i64 i64_add(i64 a, i64 b) {
    i64 r;
    r.lo = a.lo + b.lo;
    r.hi = a.hi + b.hi + (r.lo < a.lo ? 1 : 0);
    return r;
}

i64 i64_not(i64 a) {
    i64 r;
    r.lo = ~a.lo;
    r.hi = ~a.hi;
    return r;
}

i64 i64_neg(i64 a) {
    return i64_add(i64_not(a), i64_u(1));
}

i64 i64_sub(i64 a, i64 b) {
    return i64_add(a, i64_neg(b));
}

/* The full 64-bit product of two 32-bit words, from 16-bit halves. */
static i64 mul32(uint a, uint b) {
    uint al = a & 0xffff, ah = a >> 16, bl = b & 0xffff, bh = b >> 16;
    uint ll = al * bl, lh = al * bh, hl = ah * bl, hh = ah * bh;
    /* a*b = hh<<32 + (lh + hl)<<16 + ll */
    uint mid = (ll >> 16) + (lh & 0xffff) + (hl & 0xffff);
    i64 r;
    r.lo = (ll & 0xffff) | (mid << 16);
    r.hi = hh + (lh >> 16) + (hl >> 16) + (mid >> 16);
    return r;
}

i64 i64_mul(i64 a, i64 b) {
    /* Modulo 2^64 only the low words' full product and the cross terms'
     * low halves count. */
    i64 r = mul32(a.lo, b.lo);
    r.hi += a.lo * b.hi + a.hi * b.lo;
    return r;
}

i64 i64_and(i64 a, i64 b) {
    a.lo &= b.lo;
    a.hi &= b.hi;
    return a;
}

i64 i64_or(i64 a, i64 b) {
    a.lo |= b.lo;
    a.hi |= b.hi;
    return a;
}

i64 i64_xor(i64 a, i64 b) {
    a.lo ^= b.lo;
    a.hi ^= b.hi;
    return a;
}

i64 i64_shl(i64 a, uint n) {
    n &= 63;
    i64 r;
    if (n == 0) {
        return a;
    }
    if (n >= 32) {
        r.hi = a.lo << (n - 32);
        r.lo = 0;
    } else {
        r.hi = (a.hi << n) | (a.lo >> (32 - n));
        r.lo = a.lo << n;
    }
    return r;
}

/* A logical shift: zeros come in at the top (Rust's u64 wrapping_shr). */
i64 i64_shr(i64 a, uint n) {
    n &= 63;
    i64 r;
    if (n == 0) {
        return a;
    }
    if (n >= 32) {
        r.lo = a.hi >> (n - 32);
        r.hi = 0;
    } else {
        r.lo = (a.lo >> n) | (a.hi << (32 - n));
        r.hi = a.hi >> n;
    }
    return r;
}

i64 i64_sar(i64 a, uint n) {
    n &= 63;
    i64 r;
    uint sign = (a.hi & 0x80000000u) ? 0xffffffffu : 0;
    if (n == 0) {
        return a;
    }
    if (n >= 32) {
        uint s = n - 32;
        r.lo = s == 0 ? a.hi : ((a.hi >> s) | (sign << (32 - s)));
        r.hi = sign;
    } else {
        r.lo = (a.lo >> n) | (a.hi << (32 - n));
        r.hi = (a.hi >> n) | (sign << (32 - n));
    }
    return r;
}

int i64_is_neg(i64 a) {
    return (a.hi & 0x80000000u) != 0;
}

int i64_eq(i64 a, i64 b) {
    return a.lo == b.lo && a.hi == b.hi;
}

int i64_cmp(i64 a, i64 b) {
    int na = i64_is_neg(a), nb = i64_is_neg(b);
    if (na != nb) {
        return na ? -1 : 1;
    }
    if (a.hi != b.hi) {
        return a.hi < b.hi ? -1 : 1;
    }
    if (a.lo != b.lo) {
        return a.lo < b.lo ? -1 : 1;
    }
    return 0;
}

int i64_in(i64 v, i64 lo, i64 hi) {
    return i64_cmp(v, lo) >= 0 && i64_cmp(v, hi) < 0;
}

/* Unsigned 64-bit division by shifting and subtracting, bit by bit. */
static void udivmod(i64 n, i64 d, i64 *q, i64 *r) {
    i64 quot = i64_u(0), rem = i64_u(0);
    for (int i = 63; i >= 0; i--) {
        rem = i64_shl(rem, 1);
        uint bit = i >= 32 ? (n.hi >> (i - 32)) & 1 : (n.lo >> i) & 1;
        rem.lo |= bit;
        /* rem >= d, unsigned */
        if (rem.hi > d.hi || (rem.hi == d.hi && rem.lo >= d.lo)) {
            rem = i64_sub(rem, d);
            if (i >= 32) {
                quot.hi |= 1u << (i - 32);
            } else {
                quot.lo |= 1u << i;
            }
        }
    }
    *q = quot;
    *r = rem;
}

/* The magnitude of a (i64::MIN stays 2^63, as an unsigned value). */
static i64 magnitude(i64 a) {
    return i64_is_neg(a) ? i64_neg(a) : a;
}

static int no_result(i64 a, i64 b) {
    if (b.lo == 0 && b.hi == 0) {
        return 1;
    }
    /* i64::MIN / -1 overflows. */
    return a.hi == 0x80000000u && a.lo == 0 && b.hi == 0xffffffffu && b.lo == 0xffffffffu;
}

int i64_div(i64 a, i64 b, i64 *q) {
    if (no_result(a, b)) {
        return 0;
    }
    i64 r;
    udivmod(magnitude(a), magnitude(b), q, &r);
    if (i64_is_neg(a) != i64_is_neg(b)) {
        *q = i64_neg(*q);
    }
    return 1;
}

int i64_rem(i64 a, i64 b, i64 *r) {
    if (no_result(a, b)) {
        return 0;
    }
    i64 q;
    udivmod(magnitude(a), magnitude(b), &q, r);
    /* The remainder has the dividend's sign. */
    if (i64_is_neg(a)) {
        *r = i64_neg(*r);
    }
    return 1;
}

/* A few buffers for formatted numbers, so that several can be in one
 * message. */
static char numbufs[4][24];
static int numbuf;

static char *next_numbuf(void) {
    numbuf = (numbuf + 1) % 4;
    return numbufs[numbuf];
}

const char *i64_dec(i64 v) {
    char *out = next_numbuf();
    char digits[24];
    int n = 0;
    i64 m = magnitude(v);
    i64 ten = i64_u(10);
    do {
        i64 q, r;
        udivmod(m, ten, &q, &r);
        digits[n++] = (char)('0' + r.lo);
        m = q;
    } while (m.lo != 0 || m.hi != 0);
    int k = 0;
    if (i64_is_neg(v)) {
        out[k++] = '-';
    }
    while (n > 0) {
        out[k++] = digits[--n];
    }
    out[k] = 0;
    return out;
}

const char *i64_hex(i64 v) {
    char *out = next_numbuf();
    static const char hex[] = "0123456789abcdef";
    char digits[20];
    int n = 0;
    do {
        digits[n++] = hex[v.lo & 15];
        v.lo = (v.lo >> 4) | (v.hi << 28);
        v.hi >>= 4;
    } while (v.lo != 0 || v.hi != 0);
    int k = 0;
    out[k++] = '0';
    out[k++] = 'x';
    while (n > 0) {
        out[k++] = digits[--n];
    }
    out[k] = 0;
    return out;
}
