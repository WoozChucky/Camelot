// expect: 74
// Global variables: zero-initialized and initialized, pointers to globals in
// initializers, static globals, and declarations before the definition.
extern int later;             // declared here, defined below
int counter;                  // zero (a tentative definition)
int counter;                  // the same variable again
int base = 40;
int *pbase = &base;
static int hidden = 5;
int table[4] = {1, 2, 3, 4};
int *third = &table[2];
int *past = table + 4;

void bump(void) {
    counter++;
    later += 2;
}

int later = 10;

int main() {
    bump();
    bump();                                   // counter 2, later 14
    *pbase += 1;                              // base 41
    int r = counter + later + base + hidden;  // 2 + 14 + 41 + 5 = 62
    r += *third + (past - third);             // 3 + 2 -> 67
    r += past[-1];                            // 4 -> 71
    return r + (pbase == &base) + 2;          // 74
}
