// expect: 22
// + - * / % on locals: 7 + 6 - 2 + 10 - 4 - 3 * 0 + 5 = 22.
int main() {
    int a = 7;
    int b = 3;
    return a + b * 2 - 10 / 5 + a % b * 10 - (a - b) - b * 0 + 5;
}
