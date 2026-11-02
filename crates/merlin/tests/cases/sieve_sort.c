// expect: 95
// A bigger program: a sieve of Eratosthenes in a global array, then an
// insertion sort of a local array through pointers.
char composite[100];

int primes_below(int n) {
    int count = 0;
    for (int i = 2; i < n; i++) {
        if (composite[i])
            continue;
        count++;
        for (int j = i * i; j < n; j += i)
            composite[j] = 1;
    }
    return count;
}

void sort(int *a, int n) {
    for (int i = 1; i < n; i++) {
        int v = a[i];
        int *p = a + i;
        while (p > a && p[-1] > v) {
            *p = p[-1];
            p--;
        }
        *p = v;
    }
}

int main() {
    int a[8] = {42, -7, 19, 0, 5, 5, 100, -30};
    sort(a, 8);
    for (int i = 1; i < 8; i++)
        if (a[i - 1] > a[i])
            return 1;
    return primes_below(100) + a[0] + a[7];   // 25 - 30 + 100
}
