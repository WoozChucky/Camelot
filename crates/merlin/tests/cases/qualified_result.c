// expect: 42
// A qualified result is part of a function's type, but a call's value is
// unqualified; a pointer to an array of const elements takes one to an
// array of plain ones, and the two compare and subtract alike.
struct pair { long long a, b; };
const int seven(void) { return 7; }
const struct pair make(long long a, long long b) {
    struct pair p = {a, b};
    return p;
}
const int (*pick)(void) = seven;
int rows[2][3] = {{1, 2, 3}, {4, 5, 6}};
int main(void) {
    int v = pick() + 1;
    v = v * 2;
    struct pair p = make(1, 2);
    const int (*r)[3] = rows;
    int (*w)[3] = rows + 1;
    long d = w - r;
    if (r == w || d != 1)
        return 1;
    return v + (int)(p.a + p.b) + r[1][2] + (*r)[0] + 16;
}
