// expect: 100
// for with a declaration, with an expression init, and with every clause empty.
int main() {
    int sum = 0;
    for (int i = 0; i < 10; i = i + 1)  // 45
        sum = sum + i;
    int j;
    for (j = 0; j < 5; j = j + 1) {     // 20
        sum = sum + j * 2;
    }
    for (;;) {                          // 5 + 6 + 7 + 8 + 9
        sum = sum + j;
        j = j + 1;
        if (j == 10)
            break;
    }
    return sum - 0;
}
