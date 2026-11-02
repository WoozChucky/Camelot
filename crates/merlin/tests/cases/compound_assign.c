// expect: 112
// Compound assignment evaluates its target once, works through pointers and
// on narrow types (the result is converted back), and moves pointers by
// whole elements.
int calls;

int pick(void) {
    calls++;
    return 1;
}

int main() {
    int a[3] = {10, 20, 30};
    a[pick()] += 5;           // a[1] = 25, calls 1
    a[pick()] *= 2;           // a[1] = 50, calls 2
    int i = 0;
    a[i++] -= 3;              // a[0] = 7, i = 1
    int x = 100;
    x /= 7;                   // 14
    x %= 5;                   // 4
    x += a[0] + a[1];         // 4 + 7 + 50 = 61
    char c = 250;
    c += 10;                  // 260 -> 4
    int *p = a;
    p += 2;                   // &a[2]
    *p -= 1;                  // 29
    unsigned u = 10;
    u -= 11;                  // UINT_MAX
    u >>= 28;                 // 15
    return x + c + *p + calls + i + u;   // 61 + 4 + 29 + 2 + 1 + 15
}
