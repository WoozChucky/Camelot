// expect: 37
// if, if/else, else-if chains and the dangling else.
int classify(int n) {
    if (n < 0)
        return 1;
    else if (n == 0)
        return 2;
    else if (n < 10)
        return 3;
    return 4;
}
int main() {
    int r = classify(-5) + classify(0) * 2 + classify(7) * 3 + classify(99) * 4;  // 30
    if (1)
        if (0)
            r = 0;
        else
            r = r + 7;  // the else belongs to the inner if
    if (0) {
        r = 0;
    }
    return r;
}
