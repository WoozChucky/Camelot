// expect: 30
// Calls with zero, one and two arguments, and calls nested in arguments.
int seven() {
    return 7;
}
int twice(int x) {
    return x * 2;
}
int sub(int a, int b) {
    return a - b;
}
int main() {
    return sub(twice(seven()) + 20, sub(10, 6));  // 34 - 4
}
