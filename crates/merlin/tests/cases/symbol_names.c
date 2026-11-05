// expect: 19
// The names of the symbols merlin makes are clang's: string literals,
// compound literals at file scope (one of them a `static` global's, emitted
// at the end, where it is first used) and `static` locals of one name in two
// functions and twice in one. One counter numbers every name that is taken
// already, in the order they are emitted (docs/excalibur-spec.md, "Symbol
// names"); the names are in the comments.

struct pair {
    int a, b;
};

const char *greeting = "hi";                     // .L.str
int *ones = (int[]){1, 1};                       // .compoundliteral
static struct pair *late = &(struct pair){3, 4}; // .compoundliteral.7, at the end
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

int main(void) {
    int sum = greeting[0] - 'h' + farewell[1] - 'y' + again[1] - 'i'; // 0
    sum += ones[0] + ones[1] + twos[0];                                 // 4
    sum += count();                                                     // 10
    sum += other();                                                     // 14
    return sum + late->b + (*"x" == 'x');                               // 19, .L.str.6
}
