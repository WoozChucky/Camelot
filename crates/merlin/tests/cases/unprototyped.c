// expect: 42
// A function declared with `()` has no prototype: a call passes its
// arguments promoted (`char` and `short` as `int`), as named ones, which
// its definition takes as its parameters; clang warns of it, as C23 has
// no such functions.
int mul();
long long add3();
int (*pf)();

int main(void) {
    char c = 2;
    short s = 3;
    pf = mul;
    int r = mul(c, s);                         // 6
    r += (int)add3(c, 30LL, s);                // 35 -> 41
    return r + pf(1, 1) - 0;                   // 42
}

int mul(int a, int b) {
    return a * b;
}

long long add3(int a, long long b, int c) {
    return a + b + c;
}
