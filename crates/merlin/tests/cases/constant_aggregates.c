// expect: 15
// A local array or struct whose items are all constants is what clang
// copies from a constant its evaluator builds: a shift by the width or
// more has its value (by one less than the width), as a global's has.
int main(void) {
    int la[3] = { 1 << 40, 5, 1 << 33 };
    struct { int a; long long b; } ls = { 1 << 33, 1LL << 64 };
    int r = 0;
    r += la[0] == -2147483647 - 1;            // 1
    r += la[1] == 5;                          // 1
    r += la[2] == -2147483647 - 1;            // 1
    r += ls.a == -2147483647 - 1;             // 1
    r += ls.b == -9223372036854775807LL - 1;  // 1
    return r * 3;
}
