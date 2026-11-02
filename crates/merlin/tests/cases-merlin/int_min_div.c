// expect: 3
// INT_MIN / -1 overflows: it wraps to INT_MIN and the remainder is 0, as RISC-V's
// div/rem define (undefined in C).
int main() {
    int min = -2147483647 - 1;
    int minus_one = -1;
    int r = 0;
    if (min / minus_one == min)
        r = r + 1;
    if (min % minus_one == 0)
        r = r + 2;
    return r;
}
