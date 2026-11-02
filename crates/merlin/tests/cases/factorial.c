// expect: 120
// Recursive and iterative factorial agree.
int fact(int n) {
    if (n <= 1)
        return 1;
    return n * fact(n - 1);
}
int fact_loop(int n) {
    int r = 1;
    while (n > 1) {
        r = r * n;
        n = n - 1;
    }
    return r;
}
int main() {
    if (fact(10) != fact_loop(10))
        return 1;
    return fact(5);
}
