// expect: 39
// continue jumps to the step (for) or the condition (while); break leaves only
// the innermost loop.
int main() {
    int sum = 0;
    for (int i = 0; i < 100; i = i + 1) {  // 1 + 3 + 5 + 7 + 9 = 25
        if (i % 2 == 0)
            continue;
        if (i > 9)
            break;
        sum = sum + i;
    }
    int n = 0;
    while (1) {                            // n = 5
        n = n + 1;
        if (n < 5)
            continue;
        break;
    }
    int k = 0;
    while (k < 3) {                        // 3 * 2 = 6, k = 3
        for (int m = 0; ; m = m + 1) {
            if (m == 2)
                break;
            sum = sum + 1;
        }
        k = k + 1;
    }
    return sum + n + k;
}
