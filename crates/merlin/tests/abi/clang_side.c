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

/* Calls every m_ function; returns how many of its 11 checks pass. */
int c_calls_merlin(void) {
    int ok = 0;
    struct s6 s6 = {1, 2, 3};
    struct s8 p = {5, 7};
    struct s5 s5 = {{1, 2, 3, 4, 5}};
    struct s12 v = {10, 20, 30};
    struct s1 one = {'a'};
    struct mixed mx = {'x', 21};
    union u8 un;
    struct big b;
    un.i[0] = 41;
    un.i[1] = 0;
    ok += m_sum6(s6) == 6;
    ok += m_make12(4).c == 6;
    p = m_swap8(p);
    ok += p.x == 7 && p.y == 5;
    s5 = m_rev5(s5);
    ok += s5.b[0] == 5 && s5.b[4] == 1;
    ok += m_make3(1, 2, 3).b[2] == 3;
    ok += m_many(1, p, s6, v, 2, 3) == 1 + 35 + 6 + 60 + 2 + 3;
    b = m_big(3);
    ok += b.v[9] == 12 && m_sumbig(b) == 75;
    ok += m_inc1(one).c == 'b';
    mx = m_mixed(mx);
    ok += mx.c == 'y' && mx.i == 42;
    un = m_union(un);
    ok += un.i[1] == 42;
    ok += m_modify12(v) == 1020 && v.a == 10;
    return ok;
}
