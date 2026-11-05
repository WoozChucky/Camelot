// expect: 127
// sizeof gives the size of a type or of an expression's type at compile
// time; the expression is not evaluated. Its type is unsigned (size_t); long
// and the pointers take 8 bytes each (LP64).
int g[7];

int main() {
    int n = 0;
    char c;
    int r = sizeof(int) + sizeof(char) + sizeof(short) + sizeof(long);    // 15
    r += sizeof(int *) + sizeof(char **) + sizeof(void *);                 // 24 -> 39
    r += sizeof g + sizeof g[0];                                           // 28 + 4 -> 71
    r += sizeof(int[2][3]);                                                // 24 -> 95
    r += sizeof c + sizeof(c + c) + sizeof +c;                             // 1 + 4 + 4 -> 104
    r += sizeof n++;                                                       // 4 -> 108, n stays 0
    r += sizeof "hello";                                                   // 6 -> 114
    int *p = g;
    r += sizeof p + sizeof *p;                                             // 8 + 4 -> 126
    r += sizeof(int) - 5 > 0;                                              // wraps: 127
    return r + n;
}
