// expect: 172
// ++ and --, before and after: the value is the new or the old one. Narrow
// types wrap (out-of-range conversions to signed char keep the low byte, as
// clang does), pointers move by whole elements.
int main() {
    int i = 5;
    int a = i++;              // a 5, i 6
    int b = ++i;              // b 7, i 7
    int c = i--;              // c 7, i 6
    int d = --i;              // d 5, i 5
    unsigned char uc = 255;
    uc++;                     // 0
    signed char sc = -128;
    sc--;                     // 127
    short arr[4] = {1, 2, 3, 4};
    short *p = arr;
    int s = *p++;             // 1, p = &arr[1]
    s += *++p;                // arr[2] = 3 -> 4
    s += (*p)++;              // 3, arr[2] = 4 -> 7
    s += ++*p;                // arr[2] = 5 -> 12
    s += p - arr;             // 2 -> 14
    p--;                      // &arr[1]
    s += *p;                  // 2 -> 16
    return a + b + c + d + i + uc + sc + s;   // 5 + 7 + 7 + 5 + 5 + 0 + 127 + 16
}
