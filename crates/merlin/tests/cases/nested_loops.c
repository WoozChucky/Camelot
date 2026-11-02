// expect: 140
// A multiplication table summed with nested loops: the sum of i * j for i, j in
// 1..4 is (1 + 2 + 3 + 4)^2 = 100.
int main() {
    int sum = 0;
    for (int i = 1; i <= 4; i = i + 1) {
        int j = 1;
        while (j <= 4) {
            sum = sum + i * j;
            j = j + 1;
        }
    }
    return sum + 40;
}
