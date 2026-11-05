// expect: 10
// A variable named as a constant clang makes and merlin only counts: defs.c
// copies an array from a `constinit` before it defines `int constinit`,
// which keeps its own name (the definition takes it over), so this file
// links to it.
extern int constinit;
int first(int x);

int main(void) {
    return first(4) + constinit; // 4 + 6
}
