// expect: 56
// Constants that don't fit a 12-bit immediate, negative and hex ones included.
int main() {
    int a = 123456;
    int b = -123400;
    int c = 0x7fffffff;
    int d = c - 2147483600;  // 47
    return a + b + d - 47;
}
