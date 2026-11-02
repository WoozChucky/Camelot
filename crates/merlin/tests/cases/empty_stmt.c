// expect: 5
// Empty statements: an empty loop body, a stray `;` and an empty block.
int main() {
    int i;
    for (i = 0; i < 5; i = i + 1)
        ;
    ;
    while (0) {}
    return i;
}
