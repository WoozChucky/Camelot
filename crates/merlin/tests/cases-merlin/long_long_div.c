// expect: 6
// long long division where C leaves it undefined gives RISC-V's results
// (div and rem): dividing by zero gives a quotient of all ones and the
// dividend as the remainder; LLONG_MIN / -1 wraps.
long long nothing(void) {
    return 0;
}

int main() {
    long long min = -9223372036854775807LL - 1;
    long long a = 0x123456789LL;
    unsigned long long u = 0x8000000000000001ULL;
    int r = 0;
    r += a / nothing() == -1;                    // 1
    r += a % nothing() == a;                     // 2
    r += u / (unsigned long long)nothing() == 0xffffffffffffffffULL; // 3
    r += u % (unsigned long long)nothing() == u; // 4
    r += min / (nothing() - 1) == min;           // 5
    r += min % (nothing() - 1) == 0;             // 6
    return r;
}
