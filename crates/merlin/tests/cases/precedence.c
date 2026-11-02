// expect: 31
// Each line checks one precedence or associativity rule and sets one bit; a
// mistake changes the result.
int main() {
    int r = 0;
    r = r + (2 + 3 * 4 == 14);       // * before +
    r = r + 2 * (10 - 4 - 3 == 3);   // - is left-associative
    r = r + 4 * (64 / 4 / 2 == 8);   // / is left-associative
    r = r + 8 * (1 < 2 == 1);        // relational before equality
    r = r + 16 * (1 || 0 && 0);      // && before ||
    r = r + 32 * (7 - 2 * 3 < 0);    // stays 0: 7 - 6 = 1
    return r;
}
