// expect: 18
// Several locals, declarations with and without initializers, chained assignment,
// and an assignment used as a value.
int main() {
    int a = 1, b, c = 3;
    int d;
    int e;
    b = 2;
    d = e = c + 1;
    int f = a + b + c + d + e;
    return f + (b = 4) - 0;
}
