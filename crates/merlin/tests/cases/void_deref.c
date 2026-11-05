// expect: 6
// `*p` of a `void *` is GNU C's, which clang takes with a warning: an
// expression of no type, whose operand is evaluated and nothing loaded; a
// subscript of one is GNU's too, and `sizeof` evaluates nothing.
int calls;
void *next(void *p) {
    calls++;
    return p;
}
int main(void) {
    int x = 5;
    void *p = &x;
    volatile void *q = &x;
    int *ip = &x;
    char *cp = (char *)&x;
    *next(p);
    (void)*next(p);
    next(p)[0];
    (*next(p), 1);
    calls ? *next(p) : *next(p);
    *q;
    *(calls ? ip : cp);
    return calls + sizeof(*p) + sizeof p[0] - 1;
}
