// clang copies `s.b`, 24 constant bytes, from a private global it names
// `constinit` (EmitArrayInit); merlin stores them one by one and counts
// the name. The variable `constinit` defined after it takes the name over:
// clang makes it as `constinit.1` for a moment and renames it, one number,
// so `after`'s string is .L.str.2. (clang also points the copy at the
// variable then, which is why `first` does not read `s.b`.)
const char *before = "x"; // .L.str

int first(int x) {
    struct {
        int n;
        int b[6];
    } s = {x, {1, 2, 3, 4, 5, 6}}; // clang's constinit
    return s.n;
}

int constinit = 6;       // its own name, for main.c
const char *after = "a"; // .L.str.2
