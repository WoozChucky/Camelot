// expect: 15
// int arithmetic is 32-bit arithmetic, though registers are 64
// bits: the W instructions (addw, mulw, sllw, divw, negw...) wrap at 32 bits
// and leave the result sign-extended, so a wrapped int converts to long as
// the int it is, and an unsigned int stays below 2^32. (Signed overflow is
// undefined in C; merlin wraps.)
long widen(long v) {
    return v;
}

int main() {
    int r = 0;
    int max = 2147483647;
    int one = 1;
    int m = 65536;
    int x;
    unsigned u = 0;
    long l;
    unsigned long ul;

    x = max + one;                      // addw
    l = x;
    r += l == -2147483647L - 1;         // 1
    r += widen(max + 1) < 0;            // 2: an argument, sign-extended
    l = m * m;                          // mulw: 0
    r += l == 0;                        // 3
    x = one << 31;                      // sllw
    l = x;
    r += l < 0;                         // 4
    x = -2147483647 - 1;
    x = x / -1;                         // divw: INT_MIN / -1 wraps
    r += x == -2147483647 - 1;          // 5
    x = -x;                             // negw
    r += x < 0;                         // 6
    x = max;
    x++;                                // addiw
    r += x < 0;                         // 7
    x = 0x40000000;
    x += x;                             // a compound assignment, addw
    r += x < 0;                         // 8
    u = u - 1;                          // subw: 0xffffffff, sign-extended in its register
    ul = u;                             // and zero-extended to unsigned long
    r += u > 0x7fffffffu;               // 9
    r += ul == 4294967295u;             // 10
    r += u / 3 == 0x55555555u;          // 11: divuw
    r += (u >> 31) == 1;                // 12: srlw
    r += ((int)u >> 31) == -1;          // 13: sraw
    u = 0xffffffffu;
    u = u * u;                          // mulw: 1
    r += u == 1;                        // 14
    r += (unsigned)(max + max) == 0xfffffffeu; // 15
    return r;
}
