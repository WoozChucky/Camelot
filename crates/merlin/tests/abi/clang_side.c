/* The clang side of the calling-convention round trip (tests/abi.rs).
 *
 * clang compiles this file; the object is committed as clang_side.o (and
 * its assembly as clang_side.s, for reading). merlin compiles merlin_side.c,
 * which calls every c_ function here, and c_calls_merlin calls every m_
 * function there, so structs of every size class travel both ways:
 *
 *   up to 4 bytes   in one register         s1, s3, s4
 *   5 to 8 bytes    in a register pair      s5, s6, s8, mixed, u8
 *   more            by reference to a copy  s12, big (results through a
 *                                            hidden pointer in a0)
 *
 * and so do the narrow scalars (_Bool, signed char, short, unsigned short),
 * which the caller extends to 32 bits as an argument and the callee as a
 * result: clang -O1 relies on both. So do arguments past a7, on the stack
 * (a pair may straddle a7 and the stack), and the variadic arguments of
 * functions with `...`, every kind of them.
 *
 * Each side numbers its checks from 1 and reports the first that fails (0
 * when all pass); tests/abi.rs prints which one it was.
 *
 * Regenerate after changing it (needs clang on PATH):
 *
 *   MERLIN_BLESS=1 cargo test -p merlin --test abi -- --ignored clang_side
 *
 * It stays inside merlin's C subset, so the same program also runs with both
 * sides compiled by merlin.
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

/* Defined by merlin, in merlin_side.c. */
int m_sum6(struct s6 s);
struct s12 m_make12(int base);
struct s8 m_swap8(struct s8 p);
struct s5 m_rev5(struct s5 s);
struct s3 m_make3(char a, char b, char c);
int m_many(int a, struct s8 b, struct s6 c, struct s12 d, int e, char f);
struct big m_big(int seed);
int m_sumbig(struct big b);
struct s1 m_inc1(struct s1 s);
struct mixed m_mixed(struct mixed m);
union u8 m_union(union u8 u);
int m_modify12(struct s12 v);
int m_widen(signed char sc, short s, unsigned short us, _Bool b);
signed char m_trunc_sc(int x);
short m_trunc_s(int x);
unsigned short m_trunc_us(int x);
_Bool m_tobool(int x);
int m_vmix(const char *fmt, ...);
int m_vsum(int n, ...);
int m_vlate(int a, int b, int c, int d, int e, int f, int g, ...);
int m_stack(int a, int b, int c, int d, int e, int f, int g, struct s8 h, struct s12 i, char j, short k,
            struct s6 l);
struct s12 m_stack_ret(int a, int b, int c, int d, int e, int f, int g, int h, int i);

static int checks, first_failure;

/* Counts a check, and remembers the first that fails. */
static void check(int ok) {
    checks++;
    if (!ok && !first_failure)
        first_failure = checks;
}

/* clang adds the registers as they are: the caller must have extended them. */
int c_widen(signed char sc, short s, unsigned short us, _Bool b) {
    return sc + s + us + b;
}

/* The callee extends a narrow result: these hand back x converted. */
signed char c_trunc_sc(int x) {
    return x;
}

short c_trunc_s(int x) {
    return x;
}

unsigned short c_trunc_us(int x) {
    return x;
}

_Bool c_tobool(int x) {
    return x;
}

struct s1 c_inc1(struct s1 s) {
    s.c++;
    return s;
}

struct s3 c_make3(char a, char b, char c) {
    struct s3 s;
    s.b[0] = a;
    s.b[1] = b;
    s.b[2] = c;
    return s;
}

int c_sum3(struct s3 s) {
    return s.b[0] + s.b[1] + s.b[2];
}

struct s4 c_swap4(struct s4 s) {
    short t = s.lo;
    s.lo = s.hi;
    s.hi = t;
    return s;
}

struct s5 c_rev5(struct s5 s) {
    struct s5 r;
    int i;
    for (i = 0; i < 5; i++)
        r.b[i] = s.b[4 - i];
    return r;
}

struct s6 c_make6(short a, short b, short c) {
    struct s6 s;
    s.a = a;
    s.b = b;
    s.c = c;
    return s;
}

int c_sum6(struct s6 s) {
    return s.a + s.b + s.c;
}

struct s6 c_twice6(struct s6 s) {
    s.a *= 2;
    s.b *= 2;
    s.c *= 2;
    return s;
}

struct s8 c_swap8(struct s8 p) {
    int t = p.x;
    p.x = p.y;
    p.y = t;
    return p;
}

struct mixed c_mixed(struct mixed m) {
    m.c += 1;
    m.i *= 2;
    return m;
}

union u8 c_union(union u8 u) {
    u.i[1] = u.i[0] + 1;
    return u;
}

struct s12 c_add12(struct s12 a, struct s12 b) {
    struct s12 s;
    s.a = a.a + b.a;
    s.b = a.b + b.b;
    s.c = a.c + b.c;
    return s;
}

struct s12 c_scaled(int k, struct s12 v) {
    v.a *= k;
    v.b *= k;
    v.c *= k;
    return v;
}

/* Changes its copy only. */
int c_modify12(struct s12 v) {
    v.a = 1000;
    return v.a + v.b;
}

/* Exactly a0-a7. */
int c_many(int a, struct s8 b, struct s6 c, struct s12 d, int e, char f) {
    return a + b.x * b.y + c.a + c.b + c.c + d.a + d.b + d.c + e + f;
}

struct big c_big(int seed) {
    struct big b;
    int i;
    for (i = 0; i < 10; i++)
        b.v[i] = seed * i;
    return b;
}

int c_sumbig(struct big b) {
    int i, s = 0;
    for (i = 0; i < 10; i++)
        s += b.v[i];
    return s;
}

/* A call through a pointer, maybe to a merlin function. */
struct s6 c_apply6(struct s6 (*f)(struct s6), struct s6 v) {
    return f(v);
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

int c_vmix(const char *fmt, ...) {
    va_list ap;
    int s;
    va_start(ap, fmt);
    s = vmix(fmt, ap);
    va_end(ap);
    return s;
}

int c_vsum(int n, ...) {
    va_list ap;
    int s = 0;
    va_start(ap, n);
    while (n-- > 0)
        s += va_arg(ap, int);
    va_end(ap);
    return s;
}

/* The first variadic argument, a pair, straddles a7 and the stack. */
int c_vlate(int a, int b, int c, int d, int e, int f, int g, ...) {
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
int c_stack(int a, int b, int c, int d, int e, int f, int g, struct s8 h, struct s12 i, char j, short k,
            struct s6 l) {
    i.a = 1000; /* the callee's copy */
    return a + b + c + d + e + f + g + h.x * 10 + h.y + i.a / 100 + i.b + i.c + j + k + l.a + l.b + l.c;
}

/* A big result's address in a0 pushes the ninth argument to the stack. */
struct s12 c_stack_ret(int a, int b, int c, int d, int e, int f, int g, int h, int i) {
    struct s12 r;
    r.a = a + b + c;
    r.b = d + e + f;
    r.c = g * h + i;
    return r;
}

/* Calls every m_ function; returns 0 if all its checks pass, else the number
 * of the first that fails. */
int c_calls_merlin(void) {
    int w = 0x1fffd; /* -3 as a signed char or short, 65533 as an unsigned short */
    struct s6 s6 = {1, 2, 3};
    struct s8 p = {5, 7};
    struct s5 s5 = {{1, 2, 3, 4, 5}};
    struct s12 v = {10, 20, 30};
    struct s1 one = {'a'};
    struct mixed mx = {'x', 21};
    union u8 un;
    struct big b;
    signed char sc = -3;
    un.i[0] = 41;
    un.i[1] = 0;
    check(m_sum6(s6) == 6);
    check(m_make12(4).c == 6);
    p = m_swap8(p);
    check(p.x == 7 && p.y == 5);
    s5 = m_rev5(s5);
    check(s5.b[0] == 5 && s5.b[4] == 1);
    check(m_make3(1, 2, 3).b[2] == 3);
    check(m_many(1, p, s6, v, 2, 3) == 1 + 35 + 6 + 60 + 2 + 3);
    b = m_big(3);
    check(b.v[9] == 12 && m_sumbig(b) == 75);
    check(m_inc1(one).c == 'b');
    mx = m_mixed(mx);
    check(mx.c == 'y' && mx.i == 42);
    un = m_union(un);
    check(un.i[1] == 42);
    check(m_modify12(v) == 1020 && v.a == 10);
    check(m_widen(w, w, w, w) == -3 - 3 + 65533 + 1);
    check(m_trunc_sc(w) == -3);
    check(m_trunc_s(0x18000) == -32768);
    check(m_trunc_us(w) == 65533);
    check(m_tobool(256) == 1);
    /* Variadic functions and arguments on the stack. */
    check(m_vsum(0) == 0);
    check(m_vsum(10, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10) == 55);
    check(m_vmix("icspbB", 7, sc, s6, p, v, b) == 7 - 3 + 6 + 35 + 60 + 75);
    check(m_vmix("Bbpsci", b, v, p, s6, sc, 7) == 180);
    check(m_vlate(1, 2, 3, 4, 5, 6, 7, p, 100) == 28 + 75 + 100);
    check(m_stack(1, 2, 3, 4, 5, 6, 7, p, v, 'a', -5, s6) == 28 + 75 + 60 + 97 - 5 + 6);
    check(v.a == 10);
    v = m_stack_ret(1, 2, 3, 4, 5, 6, 7, 8, 9);
    check(v.a == 6 && v.b == 15 && v.c == 65);
    return first_failure;
}
