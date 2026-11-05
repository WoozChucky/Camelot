// expect: 77
// An array redeclared without its size has the composite type of the
// declarations (C99 6.2.7): the size of the one, the prototype of the
// other's function pointers, so `vp[0](1)` passes a `long long`.
long long ll(long long x) { return x == 1 ? 7 : 1; }
long long (*vp[2])() = {ll, ll};
long long (*vp[])(long long);
int main(void) { return (int)vp[0](1) * 11; }
