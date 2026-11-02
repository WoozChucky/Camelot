// expect: 123
// More argument words than a0-a7: the rest go on the stack, from 0(sp) at
// the call. A pair may straddle a7 and the stack; a big struct goes as the
// address of a copy, on the stack too when the registers are taken.

struct s8 {
    int x, y;
};

struct s12 {
    int a, b, c;
};

struct s3 {
    char b[3];
};

/* Weighted, so that any mix-up shows. */
int twelve(int a, int b, int c, int d, int e, int f, int g, int h, int i, int j, int k, int l) {
    return a * 1 + b * 2 + c * 3 + d * 4 + e * 5 + f * 6 + g * 7 + h * 8 + i * 9 + j * 10 + k * 11 + l * 12;
}

/* p starts in a7 and ends at 0(sp); s, the address of a copy, is at 4(sp);
 * the narrow scalars after it are on the stack too. */
int straddle(int a, int b, int c, int d, int e, int f, int g, struct s8 p, struct s12 s, char ch, short sh,
             struct s3 t) {
    s.a = 1000; /* the callee's copy */
    return a + b + c + d + e + f + g + p.x * 10 + p.y + s.b + s.c + ch + sh + t.b[0] + t.b[2];
}

/* A big result: its address is a hidden first argument, which pushes
 * everything one word up. */
struct s12 make(int a, int b, int c, int d, int e, int f, int g, int h) {
    struct s12 r;
    r.a = a + b + c;
    r.b = d + e + f;
    r.c = g * h;
    return r;
}

int nested(int a, int b, int c, int d, int e, int f, int g, int h, int i) {
    return a - b + c - d + e - f + g - h + i;
}

int main(void) {
    struct s8 p = {3, 4};
    struct s12 s = {5, 6, 7};
    struct s3 t = {{1, 2, 3}};
    int (*fp)(int, int, int, int, int, int, int, int, int, int, int, int) = twelve;
    struct s12 m;
    if (twelve(1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1) != 78)
        return 1;
    if (fp(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2) != 24)
        return 2;
    if (straddle(1, 1, 1, 1, 1, 1, 1, p, s, 'a', -5, t) != 7 + 34 + 13 + 97 - 5 + 4)
        return 3;
    if (s.a != 5)
        return 4;
    m = make(1, 2, 3, 4, 5, 6, 7, 8);
    if (m.a != 6 || m.b != 15 || m.c != 56)
        return 5;
    /* Calls among the arguments, and temporaries pushed around the call. */
    if (1 + nested(nested(1, 1, 1, 1, 1, 1, 1, 1, 1), 2, 3, 4, 5, 6, 7, 8, twelve(1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)) != 1 + 1 - 2 + 3 - 4 + 5 - 6 + 7 - 8 + 1)
        return 6;
    return 100 + nested(1, 2, 3, 4, 5, 6, 7, 8, 9) + make(1, 1, 1, 1, 1, 1, 3, 6).c;
}
