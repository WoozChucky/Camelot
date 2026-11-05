// expect: 26
// Integer constant expressions of every integer type are folded: each is one
// constant in the code, as clang's front end has it at -O0. Each must be
// what the same operations give at run time, on variables, and what C says
// it is. Each check that holds counts one.
int calls;

unsigned bump(void) {
    calls++;
    return 1;
}

int main() {
    int r = 0;
    int one = 1, three = 3, forty = 40, sixty = 60;
    long l8 = 8, l1024 = 1024, lm1 = -1;
    unsigned long ul0 = 0, ul1024 = 1024;
    long long ll3 = 3, llm7 = -7;
    unsigned long long ull0 = 0;
    unsigned u1 = 1, umax = 0xffffffffu;

    // long and unsigned long.
    r += 8 * (1024ul * 1024ul) == l8 * (ul1024 * ul1024);             // 1
    r += 8 * (1024ul * 1024ul) == 8388608;                            // 2
    r += 8 * (1024L * 1024L) == l8 * (l1024 * l1024);                 // 3
    r += (0ul - 1) / 3 == (ul0 - one) / three;                        // 4
    r += (0ul - 1) >> 1 == (ul0 - one) >> one;                        // 5
    r += (-1L < 1u) == (lm1 < u1);    // 6: unsigned long on RV32, long on RV64
    r += (-8L >> 1) == -4L;                                           // 7
    r += (unsigned long)-1 == ul0 - 1;                                // 8

    // long long and unsigned long long.
    r += (3LL << 40) == ll3 << forty;                                 // 9
    r += (3LL << 40) == 3298534883328LL;                              // 10
    r += (0ull - 1) / 3 + 1 == (ull0 - one) / three + one;            // 11
    r += (0ull - 1) % 1000 == (ull0 - one) % 1000;                    // 12
    r += (0ull - 1) % 1000 == 615;                                    // 13
    r += (-7LL / 2) == llm7 / 2 && (-7LL % 2) == llm7 % 2;            // 14
    r += (-1LL >> 60) == -1 && (0ull - 1) >> 60 == 15;                // 15
    r += ((0ull - 1) >> sixty) == (ull0 - one) >> sixty;              // 16
    r += (int)(1LL << 32 | 7) == 7;                                   // 17
    r += (unsigned)(0x123456789LL) == 0x23456789u;                    // 18
    r += (0LL - 1 < 0ull) == 0;                       // 19: unsigned comparison
    r += 0xffffffffu + 1LL == umax + 1LL;                             // 20

    // Mixed, and the operators that decide by their first operand.
    r += (1 ? 2LL : 3u) * 5000000000LL == 10000000000LL;              // 21
    r += (0 ? 1 / 0 : 2L) == 2;                                       // 22
    r += (1LL && 0ull) == 0 && (0LL || 4294967296LL) == 1;            // 23
    r += sizeof(long long) * 3 == 24;                                 // 24
    // The operand ?: does not choose need not be constant: only its type
    // counts (unsigned int here), and it is not called.
    r += (1 ? -1 : bump()) == 0xffffffffu && (1 ? -1 : bump()) > 0;   // 25
    r += (0 ? bump() : 2) == 2 && calls == 0;                         // 26
    return r;
}
