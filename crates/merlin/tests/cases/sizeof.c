// expect: 107
// sizeof gives the size of a type or of an expression's type at compile
// time; the expression is not evaluated. Its type is unsigned (size_t).
int g[7];

int main() {
    int n = 0;
    char c;
    int r = sizeof(int) + sizeof(char) + sizeof(short) + sizeof(long);    // 11
    r += sizeof(int *) + sizeof(char **) + sizeof(void *);                 // 12 -> 23
    r += sizeof g + sizeof g[0];                                           // 28 + 4 -> 55
    r += sizeof(int[2][3]);                                                // 24 -> 79
    r += sizeof c + sizeof(c + c) + sizeof +c;                             // 1 + 4 + 4 -> 88
    r += sizeof n++;                                                       // 4 -> 92, n stays 0
    r += sizeof "hello";                                                   // 6 -> 98
    int *p = g;
    r += sizeof p + sizeof *p;                                             // 8 -> 106
    r += sizeof(int) - 5 > 0;                                              // wraps: 107
    return r + n;
}
