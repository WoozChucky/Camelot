// expect: 198
// Casts between integer types, between pointer types, and between pointers
// and integers; reading an int's bytes (Excalibur is little-endian).
int main() {
    int x = 0x11223344;
    unsigned char *b = (unsigned char *)&x;
    int r = 0;
    r += b[0] == 0x44;                       // 1
    r += b[3] == 0x11;                       // 2
    short *h = (short *)&x;
    r += h[0] == 0x3344;                     // 3
    r += h[1] == 0x1122;                     // 4
    r += (char)0x1234 == 0x34;               // 5
    r += (signed char)0x80 == -128;          // 6
    r += (unsigned short)-2 == 65534;        // 7
    r += (int)(char)-1 == 255;               // 8
    r += (short)(unsigned char)-1 == 255;    // 9
    void *v = &x;
    int *back = v;                           // void * converts to any pointer
    r += *back == x;                         // 10
    unsigned long addr = (unsigned long)&x;
    r += (int *)addr == &x;                  // 11
    r += (int *)0 == 0;                      // 12
    b[1] = 0;                                // x = 0x11220044
    r += x == 0x11220044;                    // 13
    (void)r;
    return r * 10 + (unsigned char)x;        // 130 + 0x44
}
