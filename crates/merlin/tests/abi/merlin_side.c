/* The merlin side of the calling-convention round trip (tests/abi.rs): main
 * calls every c_ function of clang_side.c, which calls back every m_ function
 * here. Exits with 0 when every check passes, else with the number of the
 * first that fails: main's checks count from 1, and c_calls_merlin's from 101.
 */

struct s1 {
    char c;
};

struct s3 {
    char b[3];
};

struct s4 {
    short lo, hi;
};

struct s5 {
    char b[5];
};

struct s6 {
    short a, b, c;
};

struct s8 {
    int x, y;
};

struct mixed {
    char c;
    int i;
};

union u8 {
    int i[2];
    char c;
};

struct s12 {
    int a, b, c;
};

struct big {
    int v[10];
};

/* What <stdarg.h> is made of (clang's builtins, which merlin has too). */
typedef __builtin_va_list va_list;
#define va_start(ap, last) __builtin_va_start(ap, last)
#define va_arg(ap, type) __builtin_va_arg(ap, type)
#define va_end(ap) __builtin_va_end(ap)

/* Defined by clang, in clang_side.c. */
struct s1 c_inc1(struct s1 s);
struct s3 c_make3(char a, char b, char c);
int c_sum3(struct s3 s);
struct s4 c_swap4(struct s4 s);
struct s5 c_rev5(struct s5 s);
struct s6 c_make6(short a, short b, short c);
int c_sum6(struct s6 s);
struct s6 c_twice6(struct s6 s);
struct s8 c_swap8(struct s8 p);
struct mixed c_mixed(struct mixed m);
union u8 c_union(union u8 u);
struct s12 c_add12(struct s12 a, struct s12 b);
struct s12 c_scaled(int k, struct s12 v);
int c_modify12(struct s12 v);
int c_many(int a, struct s8 b, struct s6 c, struct s12 d, int e, char f);
struct big c_big(int seed);
int c_sumbig(struct big b);
struct s6 c_apply6(struct s6 (*f)(struct s6), struct s6 v);
int c_calls_merlin(void);
int c_widen(signed char sc, short s, unsigned short us, _Bool b);
signed char c_trunc_sc(int x);
short c_trunc_s(int x);
unsigned short c_trunc_us(int x);
_Bool c_tobool(int x);
int c_vmix(const char *fmt, ...);
int c_vsum(int n, ...);
int c_vlate(int a, int b, int c, int d, int e, int f, int g, ...);
int c_stack(int a, int b, int c, int d, int e, int f, int g, struct s8 h, struct s12 i, char j, short k,
            struct s6 l);
struct s12 c_stack_ret(int a, int b, int c, int d, int e, int f, int g, int h, int i);

static int checks, first_failure;

/* Counts a check, and remembers the first that fails. */
static void check(int ok) {
    checks++;
    if (!ok && !first_failure)
        first_failure = checks;
}

/* clang passes these extended to 32 bits; the sum shows they arrived right. */
int m_widen(signed char sc, short s, unsigned short us, _Bool b) {
    return sc + s + us + b;
}

/* clang compares the results without extending them again. */
signed char m_trunc_sc(int x) {
    return x;
}

short m_trunc_s(int x) {
    return x;
}

unsigned short m_trunc_us(int x) {
    return x;
}

_Bool m_tobool(int x) {
    return x;
}

int m_sum6(struct s6 s) {
    return s.a + s.b + s.c;
}

struct s12 m_make12(int base) {
    struct s12 v;
    v.a = base;
    v.b = base + 1;
    v.c = base + 2;
    return v;
}

struct s8 m_swap8(struct s8 p) {
    struct s8 r;
    r.x = p.y;
    r.y = p.x;
    return r;
}

struct s5 m_rev5(struct s5 s) {
    struct s5 r;
    int i;
    for (i = 0; i < 5; i++)
        r.b[i] = s.b[4 - i];
    return r;
}

struct s3 m_make3(char a, char b, char c) {
    struct s3 s;
    s.b[0] = a;
    s.b[1] = b;
    s.b[2] = c;
    return s;
}

int m_many(int a, struct s8 b, struct s6 c, struct s12 d, int e, char f) {
    return a + b.x * b.y + c.a + c.b + c.c + d.a + d.b + d.c + e + f;
}

struct big m_big(int seed) {
    struct big b;
    int i;
    for (i = 0; i < 10; i++)
        b.v[i] = seed + i;
    return b;
}

int m_sumbig(struct big b) {
    int i, s = 0;
    for (i = 0; i < 10; i++)
        s += b.v[i];
    b.v[0] = -1;
    return s;
}

struct s1 m_inc1(struct s1 s) {
    s.c++;
    return s;
}

struct mixed m_mixed(struct mixed m) {
    m.c++;
    m.i *= 2;
    return m;
}

union u8 m_union(union u8 u) {
    u.i[1] = u.i[0] + 1;
    return u;
}

/* Changes its copy only. */
int m_modify12(struct s12 v) {
    v.a = 1000;
    return v.a + v.b;
}

struct s6 m_negate6(struct s6 s) {
    s.a = -s.a;
    s.b = -s.b;
    s.c = -s.c;
    return s;
}

/* Variadic functions: `...` arguments travel like named ones, in a0-a7
 * and then on the stack; va_list walks them. Letters of fmt: i an int, c a
 * signed char (promoted to int), s a struct s6, p a struct s8 (a register
 * pair), b a struct s12 and B a struct big (both by reference). */
static int vmix(const char *fmt, va_list ap) {
    int s = 0;
    for (; *fmt; fmt++) {
        if (*fmt == 'i') {
            s += va_arg(ap, int);
        } else if (*fmt == 'c') {
            s += (signed char)va_arg(ap, int);
        } else if (*fmt == 's') {
            struct s6 v = va_arg(ap, struct s6);
            s += v.a + v.b + v.c;
        } else if (*fmt == 'p') {
            struct s8 v = va_arg(ap, struct s8);
            s += v.x * v.y;
        } else if (*fmt == 'b') {
            struct s12 v = va_arg(ap, struct s12);
            s += v.a + v.b + v.c;
        } else if (*fmt == 'B') {
            struct big v = va_arg(ap, struct big);
            int i;
            for (i = 0; i < 10; i++)
                s += v.v[i];
        }
    }
    return s;
}

int m_vmix(const char *fmt, ...) {
    va_list ap;
    int s;
    va_start(ap, fmt);
    s = vmix(fmt, ap);
    va_end(ap);
    return s;
}

int m_vsum(int n, ...) {
    va_list ap;
    int s = 0;
    va_start(ap, n);
    while (n-- > 0)
        s += va_arg(ap, int);
    va_end(ap);
    return s;
}

/* The first variadic argument, a pair, straddles a7 and the stack. */
int m_vlate(int a, int b, int c, int d, int e, int f, int g, ...) {
    va_list ap;
    struct s8 p;
    int x;
    va_start(ap, g);
    p = va_arg(ap, struct s8);
    x = va_arg(ap, int);
    va_end(ap);
    return a + b + c + d + e + f + g + p.x * 10 + p.y + x;
}

/* Named arguments past a7: h straddles a7 and 0(sp), then i's copy's
 * address, j, k and l are on the stack. */
int m_stack(int a, int b, int c, int d, int e, int f, int g, struct s8 h, struct s12 i, char j, short k,
            struct s6 l) {
    i.a = 1000; /* the callee's copy */
    return a + b + c + d + e + f + g + h.x * 10 + h.y + i.a / 100 + i.b + i.c + j + k + l.a + l.b + l.c;
}

/* A big result's address in a0 pushes the ninth argument to the stack. */
struct s12 m_stack_ret(int a, int b, int c, int d, int e, int f, int g, int h, int i) {
    struct s12 r;
    r.a = a + b + c;
    r.b = d + e + f;
    r.c = g * h + i;
    return r;
}

int main(void) {
    int n = 0x1fffd; /* -3 as a signed char or short, 65533 as an unsigned short */
    int inner;
    struct s1 one = {'a'};
    struct s4 w = {1, -2};
    struct s5 s5 = {{1, 2, 3, 4, 5}};
    struct s6 six[2];
    struct s8 p = {3, 4};
    struct mixed mx = {'m', 50};
    union u8 un;
    struct s12 v = {1, 2, 3}, sum;
    struct big b;
    signed char sc = -3;

    check(c_inc1(one).c == 'b');
    check(one.c == 'a');
    check(c_make3(7, 8, 9).b[1] == 8);
    check(c_sum3(c_make3(1, 2, 3)) == 6);
    w = c_swap4(w);
    check(w.lo == -2 && w.hi == 1);
    s5 = c_rev5(s5);
    check(s5.b[0] == 5 && s5.b[2] == 3 && s5.b[4] == 1);
    /* six[1] is at offset 6: not word-aligned. */
    six[1] = c_make6(10, -20, 30);
    check(six[1].a == 10 && six[1].b == -20 && six[1].c == 30);
    check(c_sum6(six[1]) == 20);
    six[0] = c_twice6(six[1]);
    check(six[0].c == 60);
    p = c_swap8(p);
    check(p.x == 4 && p.y == 3);
    mx = c_mixed(mx);
    check(mx.c == 'n' && mx.i == 100);
    un.i[0] = 7;
    un.i[1] = 0;
    un = c_union(un);
    check(un.i[1] == 8);
    sum = c_add12(v, v);
    check(sum.a == 2 && sum.b == 4 && sum.c == 6);
    check(c_add12(sum, v).c == 9);
    check(c_scaled(5, v).b == 10);
    check(c_modify12(v) == 1002);
    check(v.a == 1);
    check(c_many(1, p, six[1], v, 2, 3) == 1 + 12 + 20 + 6 + 2 + 3);
    b = c_big(3);
    check(b.v[9] == 27);
    check(c_sumbig(b) == 135);
    check(c_sumbig(c_big(1)) == 45);
    check(c_apply6(m_negate6, six[1]).b == 20);
    check(c_apply6(c_twice6, six[1]).a == 20);
    /* Narrow scalars: merlin converts the arguments, clang the results. */
    check(c_widen(n, n, n, n) == -3 - 3 + 65533 + 1);
    check(c_trunc_sc(n) == -3);
    check(c_trunc_s(0x18000) == -32768);
    check(c_trunc_us(n) == 65533);
    check(c_tobool(256) == 1);
    /* Variadic functions and arguments on the stack. */
    check(c_vsum(0) == 0);
    check(c_vsum(10, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10) == 55);
    check(c_vmix("icspbB", 7, sc, six[1], p, v, b) == 7 - 3 + 20 + 12 + 6 + 135);
    check(c_vmix("Bbpsci", b, v, p, six[1], sc, 7) == 177);
    check(c_vlate(1, 2, 3, 4, 5, 6, 7, p, 100) == 28 + 43 + 100);
    check(c_stack(1, 2, 3, 4, 5, 6, 7, p, v, 'a', -5, six[1]) == 28 + 43 + 5 + 10 + 97 - 5 + 20);
    check(v.a == 1);
    sum = c_stack_ret(1, 2, 3, 4, 5, 6, 7, 8, 9);
    check(sum.a == 6 && sum.b == 15 && sum.c == 65);
    inner = c_calls_merlin();
    /* merlin calling itself, the same way. */
    check(m_sumbig(m_big(0)) == 45);
    check(m_swap8(p).x == 3);
    check(m_many(1, p, six[1], v, 2, 3) == 44);
    check(m_rev5(s5).b[0] == 1);
    check(m_make12(1).c == 3);
    check(m_union(un).i[1] == 8);
    check(m_widen(n, n, n, n) == 65528);
    check(m_vmix("pBi", p, b, 1) == 12 + 135 + 1);
    check(m_vlate(0, 0, 0, 0, 0, 0, 0, p, 1) == 44);
    check(m_stack_ret(0, 0, 0, 0, 0, 0, 2, 3, 4).c == 10);
    if (first_failure)
        return first_failure;
    return inner ? 100 + inner : 0;
}
