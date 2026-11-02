// expect: 42
// The preprocessor: object-like and function-like macros, # and ##,
// rescanning, conditional compilation and the predefined macros.
#define N 4
#define SQUARE(x) ((x) * (x))
#define MAX(a, b) ((a) > (b) ? (a) : (b))
#define CAT(a, b) a##b
#define STR(x) #x
#define XSTR(x) STR(x)
#define TWICE(f, x) f(f(x))
#define EMPTY
#define CALL(f, ...) f(__VA_ARGS__)

#if N * 2 == 8 && defined(SQUARE) && !defined(NOT_DEFINED)
#define OK 1
#elif N
#error "the #if above should have been taken"
#else
#define OK 0
#endif

#ifdef OK
int ok = OK;
#endif

#ifndef __STDC__
#error "__STDC__ is predefined"
#endif

#if __STDC_VERSION__ < 199901L
#error "C99"
#endif

/* A guard, as headers have. */
#ifndef GUARD
#define GUARD
int guarded = 1;
#endif
#ifndef GUARD
int guarded = 2;
#endif

#undef N
#define N 5

int CAT(my, func)(void) {
    return 7;
}

int strlen_(const char *s) {
    int n = 0;
    while (s[n])
        n++;
    return n;
}

#define LINE_AT_DEFINITION __LINE__
int main(void) {
    int line = __LINE__;
    int CAT(x, 1) = SQUARE(N - 1); /* 16: the argument is parenthesized */
    const char *s = XSTR(N + 1);   /* "5 + 1" */
    const char *t = STR( a  "b" ); /* "a \"b\"": spaces collapse, quotes escaped */
    if (!ok || guarded != 1)
        return 1;
    if (x1 != 16 || MAX(x1, 20) != 20)
        return 2;
    if (strlen_(s) != 5 || s[0] != '5' || s[4] != '1')
        return 3;
    if (strlen_(t) != 5 || t[2] != '"')
        return 4;
    if (TWICE(SQUARE, 2) != 16)
        return 5;
    if (myfunc() != 7 EMPTY)
        return 6;
    if (line != 59 || LINE_AT_DEFINITION != 75)
        return 7;
    if (CALL(MAX, 3, 9) != 9 || CALL(myfunc) != 7)
        return 8;
    return x1 + SQUARE(N) + 1;
}
