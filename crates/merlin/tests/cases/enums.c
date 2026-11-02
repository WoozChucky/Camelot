// expect: 12
// Enumerations: constants with implicit and explicit values, enum types.

enum color { RED, GREEN = 5, BLUE, ALPHA = BLUE * 2 };
enum { ONE = 1, TWO };
enum sign { NEG = -1, POS = 1 };
typedef enum { OFF, ON } state;

enum color next(enum color c) {
    return c == RED ? GREEN : c == GREEN ? BLUE : RED;
}

int main(void) {
    int ok = 0;
    enum color c = RED;
    state s = ON;
    enum sign g = NEG;
    int arr[TWO + 1];
    ok += RED == 0 && GREEN == 5 && BLUE == 6 && ALPHA == 12;
    ok += ONE == 1 && TWO == 2;
    ok += sizeof(enum color) == 4;
    ok += sizeof arr == 12;
    c = next(c);
    ok += c == GREEN;
    c = next(next(c));
    ok += c == RED;
    ok += s == 1;
    ok += NEG < 0;
    /* No negative constant: clang makes the type unsigned int. */
    ok += (enum color)-1 > 0;
    /* With one, it is int. */
    ok += g < 0;
    switch (c) {
    case RED:
        ok++;
        break;
    case GREEN:
        break;
    default:
        break;
    }
    {
        enum color { RED = 40 }; /* shadows the outer constant */
        ok += RED == 40;
    }
    return ok;
}
