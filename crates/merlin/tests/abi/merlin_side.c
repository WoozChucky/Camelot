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

/* Register-wide members: a long and a pointer are 8 bytes on RV64 (LP64),
 * so l1 takes one register, l2, li and cl (the int and the char padded) a
 * pair, and l3 goes by reference; on RV32 (ILP32), where they are 4 bytes,
 * the same classes come out. */
struct l1 {
    long a;
};

struct l2 {
    long a, b;
};

struct li {
    long a;
    int b;
};

struct cl {
    char c;
    long l;
};

struct l3 {
    long a, b, c;
};

/* A long long is 64 bits on both widths: on RV32 a pair of registers, as is
 * a struct holding one (aligned to 8, so it starts at an even register when
 * variadic and at an even word on the stack). */
struct q1 {
    long long v;
};

struct iq {
    int i;
    long long v;
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
long c_mixw(int i, unsigned u, long l, unsigned long ul, char *p);
int c_umax(unsigned u);
long c_sext(int i);
unsigned long c_zext(unsigned u);
int c_narrow(long l);
unsigned c_unarrow(unsigned long l);
struct l1 c_incl1(struct l1 s);
struct l2 c_swapl2(struct l2 s);
struct li c_li(struct li s);
struct cl c_cl(struct cl s);
struct l3 c_addl3(struct l3 x, struct l3 y);
long c_vmixw(const char *fmt, ...);
long c_stackw(int a, int b, int c, int d, int e, int f, int g, long h, struct l2 i, unsigned j, long k);
long long c_qadd(long long a, int b, long long c);
unsigned long long c_qhigh(unsigned long long x);
long long c_qlate(int a, int b, int c, int d, int e, int f, int g, long long h, long long i, int j, long long k);
struct q1 c_qtwice(struct q1 s);
long long c_qsum(struct iq s, struct q1 t);
long long c_vq(const char *fmt, ...);
long long c_vqlate(int a, int b, int c, int d, int e, int f, int g, ...);

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

/* Mixed widths. On RV64 an int and an unsigned int travel sign-extended to
 * 64 bits, as the psABI says and clang -O1 relies on (an unsigned
 * 0xffffffff is -1 in its register), a long and a pointer as they are; a
 * 32-bit result comes back sign-extended too. */
long m_mixw(int i, unsigned u, long l, unsigned long ul, char *p) {
    return i + (long)(u >> 4) + l + (long)(ul >> 4) + (p[1] - '0');
}

int m_umax(unsigned u) {
    return u == 0xffffffffu;
}

long m_sext(int i) {
    return i;
}

unsigned long m_zext(unsigned u) {
    return u;
}

int m_narrow(long l) {
    return l;
}

unsigned m_unarrow(unsigned long l) {
    return l;
}

struct l1 m_incl1(struct l1 s) {
    s.a++;
    return s;
}

struct l2 m_swapl2(struct l2 s) {
    long t = s.a;
    s.a = s.b;
    s.b = t;
    return s;
}

struct li m_li(struct li s) {
    s.a += s.b;
    s.b = -s.b;
    return s;
}

struct cl m_cl(struct cl s) {
    s.c++;
    s.l = -s.l;
    return s;
}

struct l3 m_addl3(struct l3 x, struct l3 y) {
    struct l3 r;
    r.a = x.a + y.a;
    r.b = x.b + y.b;
    r.c = x.c + y.c;
    return r;
}

/* Letters of fmt: i an int, u an unsigned and L an unsigned long (both
 * shifted right by 4, so that the sums stay in range on RV32 too), l a long,
 * p a char pointer (its first char), q a struct l2, t a struct li and T a
 * struct l3 (by reference). */
long m_vmixw(const char *fmt, ...) {
    va_list ap;
    long s = 0;
    va_start(ap, fmt);
    for (; *fmt; fmt++) {
        if (*fmt == 'i') {
            s += va_arg(ap, int);
        } else if (*fmt == 'u') {
            s += va_arg(ap, unsigned) >> 4;
        } else if (*fmt == 'l') {
            s += va_arg(ap, long);
        } else if (*fmt == 'L') {
            s += (long)(va_arg(ap, unsigned long) >> 4);
        } else if (*fmt == 'p') {
            s += *va_arg(ap, char *);
        } else if (*fmt == 'q') {
            struct l2 v = va_arg(ap, struct l2);
            s += v.a * 10 + v.b;
        } else if (*fmt == 't') {
            struct li v = va_arg(ap, struct li);
            s += v.a * 10 + v.b;
        } else if (*fmt == 'T') {
            struct l3 v = va_arg(ap, struct l3);
            s += v.a + v.b + v.c;
        }
    }
    va_end(ap);
    return s;
}

/* Past a7: h, a long, is the last register; i, a pair, j and k are on the
 * stack. */
long m_stackw(int a, int b, int c, int d, int e, int f, int g, long h, struct l2 i, unsigned j, long k) {
    return a + b + c + d + e + f + g + h * 10 + i.a * 100 + i.b + (long)(j >> 4) + k;
}

long long m_qadd(long long a, int b, long long c) {
    return a + b + c;
}

unsigned long long m_qhigh(unsigned long long x) {
    return x >> 32;
}

/* h starts in a7 on RV32, its high word at 0(sp); i, j and k are on the
 * stack, i and k from an even word. */
long long m_qlate(int a, int b, int c, int d, int e, int f, int g, long long h, long long i, int j, long long k) {
    return a + b + c + d + e + f + g + h * 2 + i * 3 + j + k * 5;
}

struct q1 m_qtwice(struct q1 s) {
    s.v *= 2;
    return s;
}

long long m_qsum(struct iq s, struct q1 t) {
    return s.i + s.v + t.v;
}

/* Letters of fmt: i an int, q a long long, s a struct q1. */
long long m_vq(const char *fmt, ...) {
    va_list ap;
    long long s = 0;
    va_start(ap, fmt);
    for (; *fmt; fmt++) {
        if (*fmt == 'i') {
            s += va_arg(ap, int);
        } else if (*fmt == 'q') {
            s += va_arg(ap, long long);
        } else {
            struct q1 v = va_arg(ap, struct q1);
            s += v.v;
        }
    }
    va_end(ap);
    return s;
}

/* Seven named ints: on RV32 the variadic pair skips a7. */
long long m_vqlate(int a, int b, int c, int d, int e, int f, int g, ...) {
    va_list ap;
    long long x;
    va_start(ap, g);
    x = va_arg(ap, long long);
    va_end(ap);
    return x + a + b + c + d + e + f + g;
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
    long big;
    unsigned long ubig;
    struct l1 lone;
    struct l2 ltwo;
    struct li lint;
    struct cl chl;
    struct l3 lthree;
    long long qbig;
    struct q1 qone;
    struct iq qmix;

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
    /* Mixed widths, register-wide structs, and their variadic and stack
     * arguments. */
    big = (long)1 << (8 * sizeof(long) - 4);
    ubig = ~0ul << 31;
    check(c_mixw(-5, 0xffffffffu, big, ubig, "x7") == -5 + 0x0fffffff + big + (long)(ubig >> 4) + 7);
    check(c_umax(0xffffffffu) && !c_umax(0x7fffffffu));
    check(c_sext(-5) == -5 && c_sext(0x7fffffff) == 0x7fffffff);
    check(c_zext(0xffffffffu) == 4294967295u && c_zext(0x80000000u) > 0);
    check(c_narrow((long)(ubig | 5)) == -2147483643);
    check(c_unarrow(ubig) == 0x80000000u && c_unarrow(ubig) > 0);
    lone.a = big;
    check(c_incl1(lone).a == big + 1);
    ltwo.a = big;
    ltwo.b = -7;
    ltwo = c_swapl2(ltwo);
    check(ltwo.a == -7 && ltwo.b == big);
    lint.a = big;
    lint.b = -3;
    lint = c_li(lint);
    check(lint.a == big - 3 && lint.b == 3);
    chl.c = 'a';
    chl.l = big;
    chl = c_cl(chl);
    check(chl.c == 'b' && chl.l == -big);
    lthree.a = big;
    lthree.b = 2;
    lthree.c = -big;
    lthree = c_addl3(lthree, lthree);
    check(lthree.a == 2 * big && lthree.b == 4 && lthree.c == -2 * big);
    ltwo.a = 3;
    ltwo.b = 4;
    lint.a = 5;
    lint.b = 6;
    check(c_vmixw("iulLpqtT", -1, 0xffffffffu, big, ubig, "z", ltwo, lint, lthree) ==
          -1 + 0x0fffffff + big + (long)(ubig >> 4) + 'z' + 34 + 56 + 4);
    check(c_vmixw("TtqpLlui", lthree, lint, ltwo, "z", ubig, big, 0xffffffffu, -1) ==
          4 + 56 + 34 + 'z' + (long)(ubig >> 4) + big + 0x0fffffff - 1);
    check(c_stackw(1, 2, 3, 4, 5, 6, 7, -2, ltwo, 0xffffffffu, big) == 28 - 20 + 304 + 0x0fffffff + big);
    /* long long: pairs in registers, across a7, on the stack, variadic. */
    qbig = 0x100000000LL;
    qone.v = 5000000000LL;
    qmix.i = 7;
    qmix.v = -3000000000LL;
    check(c_qadd(qbig, -1, qbig) == 0x1ffffffffLL && c_qadd(-1, -1, -1) == -3);
    check(c_qhigh(0xabcdef0123456789ULL) == 0xabcdef01ULL);
    check(c_qlate(1, 2, 3, 4, 5, 6, 7, qbig, -qbig, 9, qbig) == 28 + 4 * qbig + 9);
    check(c_qtwice(qone).v == 10000000000LL);
    check(c_qsum(qmix, qone) == 2000000007LL);
    check(c_vq("iqisq", 1, qbig, 2, qone, -qbig) == 3 + 5000000000LL);
    check(c_vq("iiiiiiq", 1, 2, 3, 4, 5, 6, qbig) == qbig + 21);
    check(c_vqlate(1, 2, 3, 4, 5, 6, 7, qbig) == qbig + 28);
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
