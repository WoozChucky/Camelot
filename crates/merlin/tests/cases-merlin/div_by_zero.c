// expect: 15
// Division by zero gives RISC-V's div/rem results: x / 0 is -1 and x % 0 is x
// (undefined in C).
int main() {
    int zero = 0;
    int r = 0;
    if (7 / zero == -1)
        r = r + 1;
    if (7 % zero == 7)
        r = r + 2;
    if (-7 / zero == -1)
        r = r + 4;
    if (-7 % zero == -7)
        r = r + 8;
    return r;
}
