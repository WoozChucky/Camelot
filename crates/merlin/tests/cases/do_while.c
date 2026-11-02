// expect: 57
// do/while runs its body before the first test; continue goes to the test.
int main() {
    int i = 0, n = 0;
    do {
        n += i;
        i++;
    } while (i < 5);              // n = 10
    do n++; while (0);            // once: 11
    int k = 0;
    do {
        k++;
        if (k == 2)
            continue;             // to the test, not past it
        if (k == 6)
            break;
        n += 10;                  // k = 1, 3, 4, 5
    } while (k < 10);             // n = 51
    return n + k;                 // 51 + 6
}
