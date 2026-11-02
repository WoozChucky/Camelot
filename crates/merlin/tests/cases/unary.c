// expect: 14
// Unary minus and logical not, nested and mixed with binary operators.
int main() {
    int x = 5;
    int y = -x;
    int z = - -x;
    // (-5 + 5 + 3) * 4 + 0 + 1 + 1 = 14
    return (y + z + 3) * 4 + !x + !!x + !0 + -!x;
}
