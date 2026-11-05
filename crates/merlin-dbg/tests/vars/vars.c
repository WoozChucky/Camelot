/* The program of merlin-dbg's variable tests (tests/session.rs), built with
 * merlin -g for RV32 and RV64, assembled with merlin-as and linked with
 * tests/rv64/crt0.s by merlin-ld; and with clang -g for the LLDB test, which
 * compares what LLDB reads of the two. Every kind of type merlin describes:
 * base types, pointers, arrays of one and two dimensions, structs, a union,
 * enums (one with negative constants) and typedefs, const, members of
 * anonymous struct and union types; globals, static locals (one in a block),
 * arguments, locals in nested blocks, a function without a prototype, and a
 * recursion three frames deep. */

enum color { RED, GREEN = 5, BLUE };

enum sign { NEG = -2, ZERO, POS };

struct point {
    int x;
    int y;
};

typedef struct node {
    int value;
    struct node *next;
} Node;

union word {
    unsigned int u;
    unsigned char bytes[4];
};

struct shape {
    enum color color;
    struct point corners[2];
    const char *name;
};

typedef unsigned long count_t;

/* Bigger than any number. */
struct big {
    int words[11];
};

struct outer {
    int k;
    struct {
        int a;
        char c;
    } in;
    union {
        int i;
        short s;
    } un;
};

int counter = 3;
Node nodes[3];
struct shape box = {BLUE, {{1, 2}, {30, 40}}, "box"};
short grid[2][3] = {{1, 2, 3}, {4, 5, 6}};
long long big = -5;
count_t total;
enum sign sv = NEG;
struct big blob;
struct outer nest = {1, {2, 'c'}, {3}};
char hi[4] = "hi";

void putc(int c) {
    *(volatile char *)0x10000000 = c;
}

/* The sum of the list's values, from its end. */
int walk(Node *list, int depth) {
    static int calls;
    int sum = 0;
    calls++;
    if (list->next) {
        sum = walk(list->next, depth + 1);
    }
    sum += list->value;
    return sum;
}

int area(struct point a, struct point b) {
    int w = b.x - a.x;
    int h = b.y - a.y;
    return w * h;
}

int old() {
    return 7;
}

int main(void) {
    union word w;
    char letter = 'A';
    int i;
    w.u = 0x11223344;
    for (i = 0; i < 3; i++) {
        nodes[i].value = (i + 1) * 10;
        nodes[i].next = i < 2 ? &nodes[i + 1] : 0;
    }
    {
        int inner = counter * 2;
        static int seen;
        seen++;
        counter = walk(&nodes[0], 0) + inner;
    }
    total = (count_t)area(box.corners[0], box.corners[1]);
    putc(letter);
    putc('\n');
    return counter + (int)total - 66 - 1102 + w.bytes[0] - 0x44 + old() - 7;
}
