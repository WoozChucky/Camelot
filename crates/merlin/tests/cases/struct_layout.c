// expect: 16
// Struct and union layout, as clang lays them out for ILP32: each member at
// the next offset aligned for it, the size rounded up to the alignment.

struct a {
    char c;
    int i;
};

struct b {
    char c;
    short s;
    char d;
};

struct c {
    char x[3];
};

struct d {
    short s;
    struct c inner;
    int tail;
};

struct e {
    struct b b;
    char z;
};

union u {
    char c;
    int i;
    short s[3];
};

int main(void) {
    int ok = 0;
    struct d dd;
    union u v;
    ok += sizeof(struct a) == 8;
    ok += sizeof(struct b) == 6;
    ok += sizeof(struct c) == 3;
    ok += sizeof(struct d) == 12;
    ok += sizeof(struct e) == 8;
    ok += sizeof(union u) == 8;
    // offsetof, spelled out.
    ok += (unsigned)&((struct a *)0)->i == 4;
    ok += (unsigned)&((struct b *)0)->d == 4;
    ok += (unsigned)&((struct d *)0)->inner == 2;
    ok += (unsigned)&((struct d *)0)->tail == 8;
    ok += (char *)&dd.tail - (char *)&dd == 8;
    ok += (char *)&dd.inner.x[2] - (char *)&dd == 4;
    ok += sizeof(struct e[3]) == 24;
    // Every member of a union starts at its beginning (little-endian).
    v.i = 0x11223344;
    ok += v.c == 0x44;
    ok += v.s[1] == 0x1122;
    ok += (char *)&v.s[2] - (char *)&v.c == 4;
    return ok;
}
