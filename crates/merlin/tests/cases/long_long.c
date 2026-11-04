// expect: 40
// long long and unsigned long long are 64 bits on both widths: on RV32 a
// register pair, with carries, borrows, products and shifts across the two
// words, and division by the C library's __divdi3 and friends; on RV64 a
// register. Each check that holds counts one.
long long id(long long x) {
    return x;
}

int main() {
    int r = 0;
    long long a = 0x123456789LL;
    long long b = -5;
    unsigned long long u = 0xffffffffffffffffULL;
    long long big = 9223372036854775807LL;
    int i = -7;
    unsigned w = 0xfffffff0u;

    // Literals, sizes and conversions.
    r += sizeof(long long) == 8 && sizeof 1LL == 8 && sizeof 4294967296 == 8;   // 1
    r += a == 4886718345LL;                                         // 2
    r += (int)a == 0x23456789;                                      // 3: the low word
    r += (int)(a >> 32) == 1;                                       // 4
    r += (long long)i == -7 && (long long)w == 4294967280LL;        // 5: sign and zero extension
    r += (unsigned long long)i == 0xfffffffffffffff9ULL;            // 6
    r += (char)a == (char)0x89 && (short)a == 0x6789;               // 7
    r += (_Bool)(1LL << 40) == 1;                                   // 8: the high word counts
    // Arithmetic, with carries and borrows between the words.
    r += a + 0xffffffffLL == 0x223456788LL;                         // 9
    r += a - 0x23456790LL == 0xfffffff9LL;                          // 10
    r += -a == -4886718345LL && -b == 5;                            // 11
    r += a * 1000 == 4886718345000LL;                               // 12
    r += a * b == -24433591725LL;                                   // 13
    r += 0x100000000LL * 0x100000000LL == 0;                        // 14: wraps
    r += u + 1 == 0 && u * u == 1;                                  // 15
    r += (long long)((unsigned long long)big + 1) < 0;              // 16
    // Division and remainder.
    r += a / 1000 == 4886718 && a % 1000 == 345;                    // 17
    r += a / b == -977343669 && a % b == 0;                         // 18
    r += -a / 7 == -698102620 && -a % 7 == -5;                      // 19
    r += u / 3 == 0x5555555555555555ULL && u % 10 == 5;             // 20
    r += (u / id(0x100000000LL)) == 0xffffffffULL;                  // 21
    // Shifts: by the amount's low 6 bits, both ways across the words.
    r += (1LL << 32) == 4294967296LL && (1LL << 63) < 0;            // 22
    r += (a << 4) == 0x1234567890LL && (a >> 4) == 0x12345678LL;    // 23
    r += (b >> 1) == -3 && (b >> 40) == -1;                         // 24: arithmetic
    r += (u >> 1) == 0x7fffffffffffffffULL && (u >> 63) == 1;       // 25: logical
    r += (a << 0) == a && (a >> 0) == a;                            // 26
    i = 36;
    r += (1ULL << i) == 0x1000000000ULL && (u >> i) == 0xfffffffULL; // 27
    // Bitwise operators.
    r += (a & 0xff00000000LL) == 0x100000000LL;                     // 28
    r += (long long)(a | 0xf000000000000000ULL) < 0;                // 29
    r += (a ^ a) == 0 && ~0LL == -1;                                // 30
    // Comparisons: the high words decide, then the low ones unsigned.
    r += a > 0x123456788LL && a < 0x12345678aLL;                    // 31
    r += (long long)-1 < 0 && (unsigned long long)-1 > 0;           // 32
    r += 0x80000000LL > 0x7fffffffLL;                               // 33: the low word is unsigned
    r += -0x100000000LL < -0xffffffffLL;                            // 34
    r += a >= a && a <= a && a != b;                                // 35
    // Mixed with int: the int converts first.
    r += a + i == 0x123456789LL + 36;                               // 36
    r += i < a && w < a;                                            // 37
    // Increments, compound assignment, conditions.
    a = 0xffffffffLL;
    a++;
    r += a == 0x100000000LL;                                        // 38
    a--;
    a += 2;
    a *= 3;
    a -= 1;
    r += a == 0x300000002LL;                                        // 39
    r += (a ? 1 : 0) + !a + (a && b) == 2;                          // 40
    return r;
}
