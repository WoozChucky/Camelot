// expect: 15
// int arithmetic wraps around at 32 bits (signed overflow is undefined in C).
int main() {
    int max = 2147483647;
    int min = -2147483647 - 1;
    int big = 65536;
    int r = 0;
    if (max + 1 == min)
        r = r + 1;
    if (min - 1 == max)
        r = r + 2;
    if (big * big == 0)
        r = r + 4;
    if (-min == min)
        r = r + 8;
    return r;
}
