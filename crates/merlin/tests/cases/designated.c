// expect: 99
// Designated initializers (C99 6.7.8) and compound literals (6.5.2.5).

struct point {
    int x, y;
};

struct shape {
    char kind;
    struct point at[2];
    int flags;
};

union value {
    char c;
    int i;
    struct point p;
};

enum { READ, WRITE, CLOSE, NCALLS };

int do_read(void) {
    return 1;
}

int do_write(void) {
    return 2;
}

/* A table indexed by constants, as kernels write them. */
int (*calls[NCALLS])(void) = {
    [WRITE] = do_write,
    [READ] = do_read,
};

/* The size comes from the highest index. */
int sparse[] = {[5] = 50, [1] = 10, 20};

struct shape global_shape = {.flags = 3, .at[1] = {7, 8}, .kind = 'g'};
union value global_union = {.p = {4, 5}};

/* A designator into a string changes one character and keeps the others. */
char global_words[2][4] = {"ab", [0][3] = 'z', [1] = "cd", [1][0] = 'x'};

int sum_points(const struct point *p, int n) {
    int s = 0;
    int i;
    for (i = 0; i < n; i++)
        s += p[i].x + p[i].y;
    return s;
}

int len(struct point p) {
    return p.x + p.y;
}

/* Leaves non-zero words on the stack, where the next call's frame goes. */
int dirty(void) {
    volatile int junk[24];
    int i;
    for (i = 0; i < 24; i++)
        junk[i] = -1;
    return junk[5];
}

/* A compound literal's left-out parts are zero, whatever was on the stack. */
int gaps(void) {
    struct point *q = (struct point[]){[1].y = 5, [3].x = 6};
    return q[0].x | q[0].y | q[1].x | q[2].x | q[2].y | q[3].y;
}

int main(void) {
    struct point p = {.y = 2, .x = 1};
    /* Going on after a designator: .at[0].y = 6, then .at[1] = {7, 8}. */
    struct shape s = {.at[0].y = 6, 7, 8, .kind = 's'};
    int m[3][3] = {[1] = {1, 2, 3}, [2][2] = 9, [0][1] = 4};
    union value u = {.i = 0x01020304};
    char words[2][4] = {"ab", [0][3] = 'z', [1] = "cd", [1][1] = 'y'};
    int i;
    int total = 0;

    if (sizeof sparse != 6 * sizeof(int) || sparse[1] != 10 || sparse[2] != 20 || sparse[5] != 50 ||
        sparse[0] != 0 || sparse[4] != 0)
        return 1;
    if (calls[READ]() != 1 || calls[WRITE]() != 2 || calls[CLOSE] != 0)
        return 2;
    if (p.x != 1 || p.y != 2)
        return 3;
    if (s.kind != 's' || s.at[0].x != 0 || s.at[0].y != 6 || s.at[1].x != 7 || s.at[1].y != 8 || s.flags)
        return 4;
    if (m[0][0] || m[0][1] != 4 || m[1][2] != 3 || m[2][2] != 9 || m[2][0])
        return 5;
    if (global_shape.kind != 'g' || global_shape.at[1].y != 8 || global_shape.flags != 3 ||
        global_shape.at[0].x)
        return 6;
    if (global_union.p.y != 5 || u.i != 0x01020304)
        return 7;
    /* Compound literals: unnamed objects, lvalues, made anew each time. */
    for (i = 0; i < 3; i++) {
        int *a = (int[]){i, i + 1, [3] = 10};
        /* a[2] is not in the list: zero again on every pass. */
        if (a[2] != 0)
            return 13;
        a[2] = 99;
        a[0] += 100;
        total += a[0] + a[1] + a[3];
    }
    if (total != 3 * 110 + 3 + 6)
        return 8;
    if (sum_points((struct point[]){{1, 2}, {.y = 3}, [3].x = 4}, 4) != 10)
        return 9;
    if (len((struct point){.x = 20, 22}) != 42)
        return 10;
    p = (struct point){5};
    if (p.x != 5 || p.y != 0)
        return 11;
    if (*&(int){12} != 12 || sizeof (char[]){"abc"} != 4)
        return 12;
    if (words[0][0] != 'a' || words[0][1] != 'b' || words[0][2] || words[0][3] != 'z' ||
        words[1][0] != 'c' || words[1][1] != 'y' || words[1][2])
        return 14;
    if (global_words[0][0] != 'a' || global_words[0][1] != 'b' || global_words[0][2] ||
        global_words[0][3] != 'z' || global_words[1][0] != 'x' || global_words[1][1] != 'd')
        return 15;
    dirty();
    if (gaps() != 0)
        return 16;
    return sparse[5] + len(global_union.p) + (struct point){.y = 40}.y;
}
