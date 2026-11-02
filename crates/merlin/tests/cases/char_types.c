// expect: 57
// Plain char is unsigned on RISC-V; signed char and short sign-extend; a
// store keeps the low bytes; arithmetic happens in int (integer promotion).
// Converting an out-of-range value to a signed type is implementation-
// defined: clang and merlin both keep the low bytes.
int main() {
    char c = 200;
    signed char sc = 200;          // -56
    unsigned char uc = 300;        // 44
    short s = 40000;               // -25536
    unsigned short us = -1;        // 65535
    int r = 0;
    r += c > 0;                    // 1
    r += sc < 0;                   // 2
    r += uc;                       // 46
    r += s == -25536;              // 47
    r += us == 65535;              // 48
    char a = 100, b = 100;
    r += a + b == 200;             // in int, no wrap: 49
    char sum = a + b;
    r += sum == 200;               // 50
    signed char x = 100, y = 100;
    signed char wrapped = x + y;
    r += wrapped == -56;           // 51
    r += -c == -200;               // 52
    unsigned char bytes[4] = {0xff, 0x80, 0x7f, 0};
    r += bytes[0] + bytes[1] - bytes[2] == 256;   // 53
    signed char *sp = (signed char *)bytes;
    r += sp[0] + sp[1];            // -1 + -128 -> -76
    unsigned short one = 1;
    r += -1 < one;                 // promoted to int, not unsigned: -75
    unsigned char uc2 = 200;
    signed char sc2 = -2;
    r += uc2 / sc2 == -100;        // -74
    return r + 129 + sizeof(short);
}
