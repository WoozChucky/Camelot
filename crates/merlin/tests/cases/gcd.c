// expect: 21
// Euclid's algorithm with %.
int gcd(int a, int b) {
    while (b != 0) {
        int t = a % b;
        a = b;
        b = t;
    }
    return a;
}
int main() {
    return gcd(1071, 462);
}
