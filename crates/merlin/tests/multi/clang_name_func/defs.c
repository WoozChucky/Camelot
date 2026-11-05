// clang copies `s.b`, 24 constant bytes, from a private global it names
// `constinit` (EmitArrayInit); merlin stores them one by one and counts
// the name. The function `constinit` defined after it takes the name over:
// clang makes it without a name and renames it, no number, so `after`'s
// string is .L.str.1. (clang also points the copy at the function then,
// which is why `first` does not read `s.b`.)
const char *before = "x"; // .L.str

int first(int x) {
    struct {
        int n;
        int b[6];
    } s = {x, {1, 2, 3, 4, 5, 6}}; // clang's constinit
    return s.n;
}

int constinit(void) { // its own name, for main.c
    return 3;
}

static int four(void) {
    return 4;
}

int (*pointer)(void) = four;
int (*itself)(void) = constinit; // the data refers to it by its name
const char *after = "a";         // .L.str.1
