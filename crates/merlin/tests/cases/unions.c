// expect: 8
// Unions: members sharing storage, tagged unions, unions by value.

enum kind { INT, PAIR };

struct value {
    enum kind kind;
    union {
        int i;
        struct {
            short lo, hi;
        } pair;
    } as;
};

union bits {
    unsigned word;
    unsigned char bytes[4];
};

int total(struct value v) {
    return v.kind == INT ? v.as.i : v.as.pair.lo + v.as.pair.hi;
}

union bits flip(union bits b) {
    unsigned char t = b.bytes[0];
    b.bytes[0] = b.bytes[3];
    b.bytes[3] = t;
    return b;
}

int main(void) {
    int ok = 0;
    struct value a, b;
    union bits x, y;
    a.kind = INT;
    a.as.i = 40;
    b.kind = PAIR;
    b.as.pair.lo = 3;
    b.as.pair.hi = -1;
    ok += total(a) == 40;
    ok += total(b) == 2;
    ok += sizeof(struct value) == 8;
    x.word = 0x11223344;
    ok += x.bytes[0] == 0x44 && x.bytes[3] == 0x11;
    y = flip(x);
    ok += y.word == 0x44223311;
    ok += x.word == 0x11223344;
    b.as.i = 0x00050002;
    ok += total(b) == 7;
    ok += sizeof(union bits) == 4;
    return ok;
}
