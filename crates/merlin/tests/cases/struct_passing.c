// expect: 20
// Structs passed and returned by value, each size the calling convention
// treats its own way: up to 4 bytes in one register, 5 to 8 in a pair, more
// by reference to a copy (results through a hidden pointer in a0).

struct one {
    char c;
};

struct word {
    short a, b;
};

struct six {
    short a, b, c;
};

struct pair {
    int x, y;
};

struct big {
    int a, b, c;
};

struct huge {
    int v[20];
};

struct one mk_one(char c) {
    struct one o;
    o.c = c;
    return o;
}

struct six mk_six(short a, short b, short c) {
    struct six s;
    s.a = a;
    s.b = b;
    s.c = c;
    return s;
}

struct pair swap(struct pair p) {
    int t = p.x;
    p.x = p.y;
    p.y = t;
    return p;
}

struct big add_big(struct big l, struct big r) {
    struct big s;
    s.a = l.a + r.a;
    s.b = l.b + r.b;
    s.c = l.c + r.c;
    return s;
}

int sum_six(struct six s) {
    return s.a + s.b + s.c;
}

/* Changes only its own copy. */
void clobber(struct big b) {
    b.a = 99;
    b.b = 99;
}

int sum_huge(struct huge h) {
    int i, s = 0;
    for (i = 0; i < 20; i++)
        s += h.v[i];
    h.v[0] = 1000;
    return s;
}

struct huge make_huge(int base) {
    struct huge h;
    int i;
    for (i = 0; i < 20; i++)
        h.v[i] = base + i;
    return h;
}

int mixed(int a, struct six s, int b, struct big c, char d) {
    return a + sum_six(s) + b + c.a + c.b + c.c + d;
}

struct word id_word(struct word w) {
    return w;
}

/* Exactly a0-a7: two pairs, a reference and three scalars. */
int full(struct six s, struct pair p, struct big b, int x, int y, int z) {
    return s.c + p.y + b.c + x + y + z;
}

/* The hidden result pointer moves the arguments up a register. */
struct big scaled(int k, struct big b) {
    b.a *= k;
    b.b *= k;
    b.c *= k;
    return b;
}

int main(void) {
    int ok = 0;
    struct six arr[3]; /* arr[1] is not word-aligned */
    struct big b = {1, 2, 3}, c;
    struct pair p = {1, 2};
    struct huge h = make_huge(10);
    struct word w = {-3, 4};
    struct pair (*fp)(struct pair) = swap;
    ok += mk_one('z').c == 'z';
    ok += mk_six(1, 2, 3).c == 3;
    arr[1] = mk_six(10, 20, 30);
    ok += sum_six(arr[1]) == 60;
    ok += sum_six(mk_six(1, 2, 3)) == 6;
    p = swap(p);
    ok += p.x == 2 && p.y == 1;
    c = add_big(b, b);
    ok += c.a == 2 && c.b == 4 && c.c == 6;
    clobber(b);
    ok += b.a == 1 && b.b == 2;
    ok += sum_huge(h) == 390;
    ok += h.v[0] == 10;
    ok += make_huge(5).v[19] == 24;
    ok += mixed(1, arr[1], 2, b, 3) == 72;
    ok += id_word(w).a == -3 && id_word(w).b == 4;
    ok += full(arr[1], p, b, 4, 5, 6) == 30 + 1 + 3 + 15;
    ok += add_big(add_big(b, b), b).c == 9;
    ok += scaled(3, b).b == 6;
    p = fp(p);
    ok += p.x == 1;
    ok += (ok > 3 ? p : swap(p)).x == 1;
    /* Chained: c = (b = sum), both get it. */
    c = b = add_big(c, b);
    ok += b.c == 9 && c.c == 9;
    ok += sizeof(struct six) == 6 && sizeof(struct one) == 1;
    ok += add_big(scaled(2, b), scaled(1, c)).a == 9;
    return ok;
}
