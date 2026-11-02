// expect: 204
// Eight arguments (a0-a7), each weighted so that any mix-up changes the result:
// 1 + 4 + 9 + 16 + 25 + 36 + 49 + 64 = 204.
int weigh(int a, int b, int c, int d, int e, int f, int g, int h) {
    return a * 1 + b * 2 + c * 3 + d * 4 + e * 5 + f * 6 + g * 7 + h * 8;
}
int main() {
    return weigh(1, 2, 3, 4, 5, 6, 7, 8);
}
