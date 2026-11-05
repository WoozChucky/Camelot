// expect: 11
// A function named as a constant clang makes and merlin only counts: defs.c
// copies an array from a `constinit` before it defines `int
// constinit(void)`, which keeps its own name (the definition takes it
// over), so this file calls it and the pointer to it links.
int constinit(void);
int first(int x);
extern int (*pointer)(void);

int main(void) {
    return first(4) + constinit() + pointer(); // 4 + 3 + 4
}
