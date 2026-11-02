// expect: 123
// Arguments that are themselves calls must not clobber each other's registers.
int pair(int a, int b) {
    return a * 10 + b;
}
int add3(int a, int b, int c) {
    return a * 100 + b * 10 + c;
}
int main() {
    // add3(1, 2, 3)
    return add3(pair(0, 1), pair(0, 3) - 1, pair(add3(0, 0, 0), 3));
}
