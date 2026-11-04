// The RV64 program of merlin-dbg's tests (tests/session.rs), which builds it
// with merlin -g for RV64 (LP64), assembles it with merlin-as and links it
// with crt0.s at 0x80000000 with merlin-ld: source lines, stepping into and
// over calls, every kind of frame (a leaf, a recursive function, a variadic
// one) in a backtrace, and a watchpoint on a 64-bit global.
#define va_list __builtin_va_list
#define va_start(ap, last) __builtin_va_start(ap, last)
#define va_arg(ap, type) __builtin_va_arg(ap, type)
#define va_end(ap) __builtin_va_end(ap)

long total;

void putc(int c) {
    *(volatile char *)0x10000000 = c;
}

void puts(char *s) {
    while (*s) {
        putc(*s);
        s++;
    }
}

/* n longs. */
long sum(int n, ...) {
    va_list ap;
    long s = 0;
    va_start(ap, n);
    while (n-- > 0)
        s += va_arg(ap, long);
    va_end(ap);
    return s;
}

long fact(long n) {
    if (n <= 1)
        return sum(1, 1L);
    return n * fact(n - 1);
}

int main(void) {
    long f;
    puts("fact\n");
    f = fact(5);
    total = total + f;
    total = total + sum(3, 0x100000000L, 2L, 3L);
    puts("done\n");
    return (int)(total - 0x100000000L - 125);
}
