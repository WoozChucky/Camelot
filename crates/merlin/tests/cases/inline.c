// expect: 30
// inline is accepted: a static inline function is an ordinary static one,
// an extern inline one an ordinary external definition.

static inline int sq(int x) {
    return x * x;
}

inline static int cube(int x) {
    return x * sq(x);
}

extern inline int inc(int x) {
    return x + 1;
}

int main(void) {
    return sq(3) + cube(2) + inc(12);
}
