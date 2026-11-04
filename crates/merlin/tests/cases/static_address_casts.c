// expect: 15
// An address converted to an integer type as wide as a pointer is an address
// constant, as clang has it: a global may be initialized with one, moved by
// adding or subtracting an integer (by bytes), or converted back to a
// pointer. long and the intptr types are that wide on both widths; int and
// unsigned only on RV32, long long only on RV64.
typedef __INTPTR_TYPE__ intptr_t;
typedef __UINTPTR_TYPE__ uintptr_t;

int x[4] = {1, 2, 3, 4};
struct pair {
    char c;
    int n;
} s = {'a', 7};
int f(void) { return 5; }

long a = (long)&x;
unsigned long b = (unsigned long)&x[1];
uintptr_t c = (uintptr_t)&s.n;
intptr_t d = (intptr_t)x + 8;
long e = 4 + (long)&x[0];
long g = (long)&x[3] - 4;
int *p = (int *)((uintptr_t)&x + 12);
long text = (long)"text";
long fn = (long)f;
long both[2] = {(long)&x, (long)&x[2]};
#if __riscv_xlen == 32
int w = (int)&x[1];
unsigned uw = (unsigned)&x + 4;
#else
long long w = (long long)&x[1];
unsigned long long uw = (unsigned long long)&x + 4;
#endif

int main(void) {
    static long local = (long)&s;
    int r = 0;
    r += a == (long)x;                                  // 1
    r += b == (unsigned long)&x[1] && *(int *)b == 2;   // 2
    r += *(int *)c == 7;                                // 3
    r += *(int *)d == 3;                                // 4
    r += *(int *)e == 2;                                // 5
    r += *(int *)g == 3;                                // 6
    r += *p == 4;                                       // 7
    r += *(char *)text == 't' && ((char *)text)[3] == 't'; // 8
    r += ((int (*)(void))fn)() == 5;                    // 9
    r += *(int *)both[0] == 1 && *(int *)both[1] == 3;  // 10
    r += *(int *)w == 2;                                // 11
    r += *(int *)uw == 2;                               // 12
    r += sizeof w == sizeof(void *) && sizeof uw == sizeof(void *); // 13
    r += ((struct pair *)local)->c == 'a';              // 14
    r += (long)&s == local;                             // 15
    return r;
}
