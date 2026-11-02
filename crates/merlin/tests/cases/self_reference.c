// expect: 59
// A declaration's name is in scope from the end of its declarator, so inside
// its own initializer (C99 6.2.1p7): for globals and static locals as for
// automatic ones. And a null pointer constant in ?: takes the other side's
// type, even written (void *)0.
char x;
void *q;
void *self = &self;                          // points at itself
int a[3];
int a[] = {7};                               // still an int[3]: {7, 0, 0}

int size_of_own(void) {
    static int x = sizeof x;                 // this int, not the global char: 4
    return x;
}

int points_at_itself(void) {
    static void *q = &q;                     // this q, not the global one
    return q == &q;
}

int empty_list(void) {
    static void *head[2] = {head, head};     // an empty list links to itself
    return (head[0] == head) + (head[1] == (void *)head);
}

int second(int *p, int c) {
    return *((c ? (void *)0 : p) + 1);       // an int *, so + 1 is one int
}

int main() {
    int r = size_of_own() * 10;              // 40
    r += points_at_itself();                 // 41
    r += empty_list();                       // 43
    r += self == &self;                      // 44
    r += q == 0;                             // the global q is untouched: 45
    r += x == 0;                             // so is the global x: 46
    r += sizeof a / sizeof a[0];             // 49
    r += a[0] + a[2] - 7;                    // 49
    int arr[2] = {5, 10};
    r += second(arr, 0);                     // 59
    int *none = 1 ? (void *)0 : arr;
    return r + (none == 0) - 1;              // 59
}
