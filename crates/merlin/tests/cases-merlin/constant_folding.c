// expect: 12
// Folded constant expressions keep merlin's definitions of what C leaves
// undefined (spec section 10): signed overflow wraps, INT_MIN / -1 wraps, a
// shift uses the low 5 or 6 bits of its amount; a constant is what the
// instructions compute at run time. Division by zero is not folded: it stays
// a division, and gives RISC-V's results. Each check that holds counts one.
int main() {
    int r = 0;
    int one = 1, m1 = -1, imax = 2147483647, i33 = 33, i40 = 40, i65 = 65, zero = 0;
    long lmax = 2147483647L, lone = 1;
    long long llmax = 9223372036854775807LL, llmin = -9223372036854775807LL - 1, llone = 1;

    r += 2147483647 + 1 == imax + one;                                // 1
    r += 2147483647L + 1 == lmax + lone;     // 2: wraps on RV32 only
    r += 9223372036854775807LL + 1 == llmax + llone;                  // 3
    r += (-2147483647 - 1) / -1 == (-imax - one) / m1;                // 4
    r += (-9223372036854775807LL - 1) / -1 == llmin / m1;             // 5
    r += (-9223372036854775807LL - 1) % -1 == llmin % m1;             // 6
    r += (1 << 33) == one << i33 && (1 << 33) == 2;                   // 7
    r += (1L << 40) == lone << i40;          // 8: 256 on RV32, 2^40 on RV64
    r += (1LL << 65) == llone << i65 && (1LL << 65) == 2;             // 9
    r += (-1 >> 33) == m1 >> i33;                                     // 10
    r += 7 / 0 == -1 && 7 % 0 == 7 && 7 / zero == -1;                 // 11
    r += 7LL / 0 == -1 && 7LL % 0 == 7;                               // 12
    return r;
}
