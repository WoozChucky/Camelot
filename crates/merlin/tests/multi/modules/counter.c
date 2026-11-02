#include "counter.h"
#include "limits.h" /* again: #pragma once makes it empty */

#define va_list __builtin_va_list
#define va_start(ap, last) __builtin_va_start(ap, last)
#define va_arg(ap, type) __builtin_va_arg(ap, type)
#define va_end(ap) __builtin_va_end(ap)

int total;

/* Not main.c's `calls` or `helper`: static names stay in their file. */
static int calls;

static int helper(int n) {
    return n > COUNTER_MAX ? COUNTER_MAX : n;
}

void counter_add(struct counter *c, int n) {
    calls++;
    c->value += helper(n);
    c->hits++;
    total += helper(n);
}

int counter_sum(int n, ...) {
    va_list ap;
    int s = 0;
    calls++;
    va_start(ap, n);
    while (n--)
        s += va_arg(ap, int);
    va_end(ap);
    return s;
}

int counter_calls(void) {
    return calls;
}
