// expect: 140
// Local and global arrays, indexing, decay to a pointer when passed, sizeof,
// and a two-dimensional array.
int squares[10];

int sum(int *a, int n) {
    int s = 0;
    for (int k = 0; k < n; k++)
        s += a[k];
    return s;
}

int trace(int m[3][3]) {
    return m[0][0] + m[1][1] + m[2][2];
}

int main() {
    for (int k = 0; k < 10; k++)
        squares[k] = k * k;
    int local[5];
    for (int k = 0; k < 5; k++)
        local[k] = squares[k + 1];                 // 1 4 9 16 25
    int m[3][3];
    for (int r = 0; r < 3; r++)
        for (int c = 0; c < 3; c++)
            m[r][c] = r * 3 + c;                   // trace 0 + 4 + 8 = 12
    int sizes = sizeof squares + sizeof local + sizeof m + sizeof m[0];   // 40 + 20 + 36 + 12 = 108
    return sum(local, 5) + trace(m) + sizes - 35;  // 55 + 12 + 108 - 35
}
