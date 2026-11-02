// expect: 168
// Loop-heavy: count the primes below 1000 by trial division.
int is_prime(int n) {
    if (n < 2)
        return 0;
    for (int d = 2; d * d <= n; d = d + 1)
        if (n % d == 0)
            return 0;
    return 1;
}
int main() {
    int count = 0;
    for (int n = 0; n < 1000; n = n + 1)
        count = count + is_prime(n);
    return count;
}
