// expect: 160
// Bitwise and shift operators, their compound assignments, and their
// precedence (& binds looser than ==, << looser than +).
int popcount(unsigned v) {
    int n = 0;
    while (v) {
        n += v & 1;
        v >>= 1;
    }
    return n;
}

int main() {
    int r = 0;
    r += (0xf0 | 0x0f) == 0xff;          // 1
    r += (0xff & 0x3c) == 0x3c;          // 2
    r += (0xff ^ 0x0f) == 0xf0;          // 3
    r += ~0 == -1;                       // 4
    r += (~0u >> 31) == 1;               // 5
    r += (1 << 10) == 1024;              // 6
    r += (-16 >> 2) == -4;               // arithmetic shift: 7
    int neg = -16;
    r += (neg >> 2u) == -4;              // the left operand's type decides: 8
    r += (-16 >> 2u) == -4;              // 9
    r += popcount(0xdeadbeef);           // 24 -> 33
    r += 6 & 3 == 3;                     // 6 & (3 == 3) is 0: 33
    r += (6 & 3) == 2;                   // 34
    r += 1 << 2 + 1;                     // 1 << 3 -> 42
    int x = 0x5a;
    x &= 0x0f;                           // 0x0a
    x |= 0x30;                           // 0x3a
    x ^= 0x01;                           // 0x3b
    x <<= 2;                             // 0xec
    x >>= 1;                             // 0x76 = 118
    return r + x;
}
