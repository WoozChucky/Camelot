// expect: 14
// Function pointers: declarators, typedefs, tables, calls through them,
// passing and returning them.

int add(int a, int b) {
    return a + b;
}

int sub(int a, int b) {
    return a - b;
}

int mul(int a, int b) {
    return a * b;
}

int (*ops[3])(int, int) = {add, sub, mul};

int (*pick(int which))(int, int) {
    return ops[which];
}

int apply(int (*f)(int, int), int a, int b) {
    return f(a, b);
}

typedef int (*binop)(int, int);

struct vtable {
    binop op;
    int unit;
};

static int twice(int x) {
    return 2 * x;
}

void (*nothing)(void);

int fold(binop f, const int *a, int n, int acc) {
    int i;
    for (i = 0; i < n; i++)
        acc = f(acc, a[i]);
    return acc;
}

int main(void) {
    int ok = 0;
    binop f = add;
    int (*g)(int) = &twice;
    struct vtable vt = {mul, 1};
    struct vtable *pv = &vt;
    int nums[4] = {1, 2, 3, 4};
    ok += f(2, 3) == 5;
    ok += (*f)(2, 3) == 5;
    ok += ops[1](10, 4) == 6;
    ok += apply(mul, 6, 7) == 42;
    ok += pick(2)(3, 3) == 9;
    ok += g(21) == 42;
    ok += vt.op(4, 5) == 20;
    ok += pv->op(pv->unit, 9) == 9;
    ok += f == add && f != sub;
    ok += nothing == 0;
    f = 0;
    ok += !f;
    ok += apply(pick(0), pick(1)(5, 2), 1) == 4;
    ok += fold(mul, nums, 4, 1) == 24 && fold(ops[0], nums, 4, 0) == 10;
    ok += (ok > 0 ? add : sub)(1, 1) == 2;
    return ok;
}
