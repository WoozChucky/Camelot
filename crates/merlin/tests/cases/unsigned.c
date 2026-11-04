// expect: 30
// expect-lp64: 29
// Unsigned arithmetic wraps, divides and compares as unsigned, and shifts in
// zeros; mixing int and unsigned converts the int (the usual arithmetic
// conversions). long is 32 bits on ILP32, like int; on LP64 it is 64, and
// sizeof l == 4 fails there.
int main() {
    unsigned u = 0;
    u = u - 1;                                   // 4294967295
    int r = 0;
    r += u == 0xffffffff;                        // 1
    r += u > 0;                                  // 2
    r += u / 2 == 0x7fffffff;                    // 3
    r += u % 10 == 5;                            // 4
    r += u >> 28;                                // 15 -> 19
    int neg = -1;
    r += (neg < 0u) ? 0 : 1;                     // -1 becomes UINT_MAX: 20
    r += neg < 0;                                // 21
    r += (-8 / 3 == -2) + (-8 % 3 == -2);        // 23
    r += (unsigned)-8 / 3 == 1431655762;         // 24
    unsigned long ul = 3000000000u;
    r += (ul > 2000000000) + (ul / 1000000 == 3000);   // 26
    long l = -5;
    r += (l * 2 == -10) + (sizeof l == 4);       // 28
    unsigned big = 0x80000000;
    r += (big >> 31) + ((int)big >> 31 == -1);   // 30
    return r;
}
