// expect: 15
// A constant condition is decided at compile time, as clang's code generator
// decides it at -O0: only what runs is generated (the arm of an if, the
// operand of ?:, the right operand of 1 && and 0 ||, the statements of a
// switch's case), and while (1) and do ... while (0) test nothing. Code a
// label makes reachable is kept. Each check that holds counts one.
int calls;

int bump(void) {
    calls++;
    return 1;
}

// Only ever called where a condition clang folds rules it out, so never
// emitted (a static function nothing emitted uses is left out).
static int never(void) {
    calls += 100;
    return 0;
}

int main() {
    int r = 0;
    int x = 3;
    int *p = &x;

    if (0) {
        int dead = (short)bump();
        r += dead;
    }
    if (1)
        r++;                                        // 1
    else
        bump();
    if (sizeof(int) == 4)
        r++;                                        // 2
    r += (1 ? x : bump()) == 3;                     // 3
    r += (0 ? bump() : x + 1) == 4;                 // 4
    r += (1 && x) == 1 && (0 || x - 3) == 0;        // 5
    while ((unsigned char)1) {
        if (++x == 6)
            break;
    }
    r += x == 6;                                    // 6
    do
        x++;
    while (0);
    r += x == 7;                                    // 7
    switch (2) {
    case 1:
        bump();
        break;
    case 2:
        x = 10;
    case 3:
        x++;
        break;
    default:
        bump();
    }
    r += x == 11;                                   // 8
    switch (5) {
    case 1:
        bump();
    }
    // A case in the arm not taken: it is kept, and the switch reaches it.
    switch (x) {
    case 1:
        if (0) {
        case 11:
            r++;                                    // 9
        }
    }
    // A switch on a constant whose case has a block of its own.
    switch (1) {
    case 1: {
        int y = 2;
        x = y;
    }
    }
    r += x == 2;                                    // 10
    // A label in the arm not taken: it is kept, and goto reaches it.
    if (0) {
    skipped:
        r++;                                        // 11
        goto done;
    }
    goto skipped;
done:
    // An object's address is not null, nor another object's: clang folds
    // such a condition too, and leaves out what it rules out. A pointer's
    // value is not known (p is tested).
    if (&calls == 0)
        r = never();
    if (&calls != 0 && &x != &calls && p != 0)
        r++;                                        // 12
    r += "s" != 0 ? 1 : never();                    // 13
    r += &x != 0 || never();                        // 14
    r += calls == 0;                                // 15
    return r;
}
