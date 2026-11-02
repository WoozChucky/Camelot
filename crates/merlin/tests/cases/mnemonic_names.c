// expect: 42
// Functions may be named like instructions, pseudo-instructions or CSRs: in
// the generated assembly they only appear as labels and call targets.
int add(int a, int b) { return a + b; }
int li(int x) { return x; }
int j(void) { return 1; }
int ret(void) { return 2; }
int call(int x) { return x * 2; }
int mstatus(void) { return 3; }
int cycle(void) { return 4; }
int text(void) { return 5; }
int main() {
    return add(li(j() + ret()), call(mstatus() + cycle() + text())) + 15;
}
