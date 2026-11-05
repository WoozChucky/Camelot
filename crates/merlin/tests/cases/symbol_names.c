// expect: 36
// The names of the symbols merlin makes are clang's: string literals,
// compound literals at file scope (one of them a `static` global's, emitted
// at the end, where it is first used) and `static` locals of one name in two
// functions and twice in one. One counter numbers every name that is taken
// already, in the order they are emitted, the names of the constants clang
// copies a local's initializer from included, which merlin does not make
// (docs/excalibur-spec.md, "Symbol names"), and not those of code control
// cannot reach, which clang does not generate; the names are in the
// comments.

struct pair {
    int a, b;
};

struct named {
    char name[8];
    int k;
};

const char *greeting = "hi";                     // .L.str
int *ones = (int[]){1, 1};                       // .compoundliteral
static struct pair *late = &(struct pair){3, 4}; // .compoundliteral.13, at the end
const char *farewell = "bye";                    // .L.str.1
int *twos = (int[]){2};                          // .compoundliteral.2
const char *again = "hi";                        // .L.str again: the same bytes

int count(void) {
    static int n = 5;                 // count.n
    static const char *tag = "count"; // count.tag, then its .L.str.3
    {
        static int n = 7; // count.n.4
        n++;
    }
    return n + (tag[0] == 'c');
}

int other(void) {
    static int n = 1; // other.n
    {
        static int n; // other.n.5
        n++;
    }
    return n + late->a;
}

int copies(int k) {
    int small[3] = {1, 2, 3}; // clang's __const.copies.small
    {
        int small[3] = {4, 5, 6}; // clang's __const.copies.small.6
        k += small[0] - 4;
    }
    int few[10] = {1, 2};               // zeros and two stores: no name
    struct named nm = {"ab", k};        // its name copied from .L.str.7, "ab" padded to 8
    int *five = (int[]){1, 2, 3, 4, 5}; // clang's constinit
    int *more = (int[]){5, 4, 3, 2, 1}; // clang's constinit.8
    return small[2] + few[1] + nm.name[1] - 'b' + nm.k + five[4] + more[0] - 5;
}

// What follows a jump is not generated until a label: no names for its
// strings, but its static locals are emitted all the same.
int dead(int k) {
    switch (k) {
        k = *"before the first case"; // no code, no name
    case 1:
        return *"one" - 'o'; // .L.str.9
    default:
        break;
    }
    if (k == 2)
        return 2;
    else
        goto out;
    k += *"after the if"; // neither arm falls through: no name
    {
        static int n = 3; // dead.n, not reached but emitted
        int m = *"m";     // a slot, but no initialization: no name
        k += m;
    }
out:
    for (;;) {
        if (k++ > 3)
            break;
        continue;
        k += *"after continue"; // no name
    }
    static int n = 4; // dead.n.10
    while (1)
        return k + *"loop" - 'l' + n - 4; // .L.str.11
    return *"after the loop";            // no name
}

int main(void) {
    int sum = greeting[0] - 'h' + farewell[1] - 'y' + again[1] - 'i'; // 0
    sum += ones[0] + ones[1] + twos[0];                                 // 4
    sum += count();                                                     // 10
    sum += other();                                                     // 14
    sum += copies(1);                                                   // 25
    sum += dead(1) + dead(5);                                           // 31
    return sum + late->b + (*"x" == 'x');                               // 36, .L.str.12
}
