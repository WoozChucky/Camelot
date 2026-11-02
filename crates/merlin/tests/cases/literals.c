// expect: 54
// Integer literals in every base, their suffixes, and the types C gives
// them: a hex literal too big for int is unsigned.
int main() {
    int r = 0;
    r += 0x10 + 010 + 10;             // 16 + 8 + 10 = 34
    r += 0XaB == 171;                 // 35
    r += 0xffffffff > 0;              // unsigned int: 36
    r += -1 < 0x7fffffff;             // int: 37
    r += 2147483647 + 0u > 0;         // 38
    r += 10u / 3 == 3;                // 39
    r += 4000000000u / 1000000000u;   // 4 -> 43
    r += sizeof 1L + sizeof 'a';      // 4 + 4 -> 51
    r += 017777777777 == 0x7fffffff;  // 52
    r += 037777777777 == -1;          // compared as unsigned: 53
    r += 0 == 00;                     // 54
    return r;
}
