// expect: 42
// Falling off the end of a function returns 0 (using that value is undefined in C).
int bump(int x) {
    x = x + 1;
}
int empty() {}
int loop_then_fall(int n) {
    while (n > 0)
        n = n - 1;
}
int main() {
    return 42 + bump(5) + empty() + loop_then_fall(3);
}
