// expect: 7
// && and || produce 0 or 1 and short-circuit: the right side runs only when needed.
int bump(int v) {
    return v + 1;
}
int main() {
    int hits = 0;
    int r = 5 && 7;                      // 1
    r = r + (0 || -3);                   // 2
    r = r + (0 && (hits = bump(hits)));  // skipped
    r = r + (1 || (hits = bump(hits)));  // skipped, r = 3
    r = r + (1 && (hits = bump(hits)));  // runs: hits = 1, r = 4
    r = r + (0 || (hits = bump(hits)));  // runs: hits = 2, r = 5
    return r + hits;
}
