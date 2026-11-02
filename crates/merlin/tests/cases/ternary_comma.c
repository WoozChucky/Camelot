// expect: 77
// The conditional operator evaluates one side only and converts both to a
// common type; the comma operator evaluates left to right.
int abs_value(int x) { return x < 0 ? -x : x; }
int sign(int x) { return x > 0 ? 1 : x < 0 ? -1 : 0; }

int side;

int touch(int v) {
    side += v;
    return v;
}

int main() {
    int r = abs_value(-7) + abs_value(3);             // 10
    r += sign(5) + sign(-5) * 2 + sign(0);            // 1 - 2 + 0 -> 9
    r += 1 ? touch(1) : touch(100);                   // side 1 -> 10
    r += 0 ? touch(100) : touch(2);                   // side 3 -> 12
    int x = (touch(4), touch(5), 6);                  // side 12, x 6
    r += x;                                           // 18
    int a[2] = {30, 40};
    int *p = r > 0 ? a : 0;
    r += p[1];                                        // 58
    unsigned one = 1 ? 1 : 2u;
    r += (1 ? -1 : 0u) > 0;                           // -1 as unsigned: 59
    for (int i = 0, j = 10; i < j; i++, j--)
        r++;                                          // 5 times -> 64
    return r + side + one;                            // 64 + 12 + 1
}
