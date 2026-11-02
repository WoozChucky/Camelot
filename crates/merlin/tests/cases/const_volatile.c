// expect: 10
// const and volatile: const objects in read-only data, volatile accesses all
// performed.

const int table[4] = {1, 2, 4, 8};
const char *const names[2] = {"zero", "one"};
volatile int counter;
const struct {
    int a;
    char b;
} cs = {5, 'c'};
const int zero_const;

int sum(const int *p, int n) {
    int s = 0;
    while (n--)
        s += *p++;
    return s;
}

int main(void) {
    int ok = 0;
    const int k = 7;
    volatile int v = 0;
    int i;
    int *const fixed = &i;
    const volatile int cv = 3;
    volatile char buf[4];
    for (i = 0; i < 10; i++)
        v += i;
    ok += v == 45;
    counter = 1;
    counter = counter + 1;
    ok += counter == 2;
    ok += sum(table, 4) == 15;
    ok += names[1][1] == 'n';
    ok += k * 2 == 14;
    *fixed = 3;
    ok += i == 3;
    ok += cs.a == 5 && cs.b == 'c';
    ok += cv == 3;
    buf[0] = 1;
    buf[0] = buf[0] + 1;
    ok += buf[0] == 2;
    ok += zero_const == 0;
    return ok;
}
