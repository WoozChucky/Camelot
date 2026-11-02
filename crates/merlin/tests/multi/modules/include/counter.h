/* Shared by main.c and counter.c (found with -I include); util.s defines
 * the functions and data marked "assembly". */
#ifndef COUNTER_H
#define COUNTER_H

#include "limits.h"

struct counter {
    int value;
    int hits;
};

/* Defined in counter.c. */
extern int total;
void counter_add(struct counter *c, int n);
int counter_sum(int n, ...);
int counter_calls(void);

/* Assembly: util.s. */
extern const int table[TABLE_SIZE];
int triple(int x);
int square_plus_one(int x); /* calls square, in main.c */

/* Defined in main.c. */
int square(int x);

#endif
