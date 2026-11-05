// expect: 211
// Several translation units: main.c and counter.c share counter.h (from
// -I include), util.s is hand-written assembly. Calls
// go across files both ways, `total` is an extern global, and each file's
// statics are its own.
#include "counter.h"
#include "counter.h" /* the guard makes the second one empty */

static int calls = 50;

static int helper(int n) {
    return -n;
}

int square(int x) {
    return x * x;
}

int main(void) {
    struct counter c = {0, 0};
    int i;
    int t = 0;
    counter_add(&c, 5);
    counter_add(&c, 500); /* counter.c's helper caps it at COUNTER_MAX */
    if (c.value != 105 || c.hits != 2 || total != 105)
        return 1;
    if (counter_sum(3, 1, 2, 3) != 6)
        return 2;
    /* Three calls in counter.c; this file's `calls` is untouched. */
    if (counter_calls() != 3 || calls != 50 || helper(1) != -1)
        return 3;
    for (i = 0; i < TABLE_SIZE; i++)
        t += table[i];
    if (t != 10 || triple(7) != 21 || square_plus_one(3) != 10)
        return 4;
    total += 1;
    return total + calls + square_plus_one(7) + triple(1) + helper(c.hits) + 4;
}
