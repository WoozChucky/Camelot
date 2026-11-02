// expect: 66
// A static local keeps its value between calls; each function, and each
// block, has its own.
int next(void) {
    static int n = 10;
    n++;
    return n;
}

int other(void) {
    static int n;            // zero, not shared with next's n
    {
        static int n = 100;  // another one again
        n--;
    }
    return ++n;
}

int main() {
    next();
    next();
    int a = next();          // 13
    other();
    int b = other();         // 2
    int c = next();          // 14
    return a + b + c + 37;   // 66
}
