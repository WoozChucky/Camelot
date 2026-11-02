// expect: 35
// void functions: return without a value, results through pointers and
// globals.
int total;

void add(int v) {
    if (v < 0)
        return;
    total += v;
}

void fill(int *a, int n, int v) {
    for (int i = 0; i < n; i++)
        a[i] = v + i;
}

void nothing(void) {}

int main() {
    add(5);
    add(-3);
    add(7);
    int a[4];
    fill(a, 4, 10);
    nothing();
    return total + a[0] + a[3];    // 12 + 10 + 13
}
