// expect: 63
// Parentheses override precedence: 21 * 3 - (21 - 21) = 63.
int main() {
    int a = (1 + 2) * (3 + 4);
    int b = ((((a))));
    return b * (5 - (2 - 0)) - (a - (a - 0));
}
