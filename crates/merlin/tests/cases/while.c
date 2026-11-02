// expect: 55
// Sum 1..10 with a while loop.
int main() {
    int i = 1;
    int sum = 0;
    while (i <= 10) {
        sum = sum + i;
        i = i + 1;
    }
    return sum;
}
