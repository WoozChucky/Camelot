// expect: 63
// All six comparisons, true and false; each result must be exactly 0 or 1.
int main() {
    int a = 3;
    int b = 5;
    int r = 0;
    r = r + (a < b) + (b < a);                     // 1
    r = r + 2 * ((a <= b) + (a <= a) - (b <= a));  // 4
    r = r + 8 * ((b > a) + (a > b));               // 8
    r = r + 16 * ((b >= a) + (b >= b) - 1);        // 16
    r = r + 32 * ((a == 3) + (a == b) + (a != a)); // 32
    return r + (a != b) + 1;                       // 61 + 2
}
