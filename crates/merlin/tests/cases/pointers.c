// expect: 93
// Address-of and dereference: swapping through pointers, a pointer to a
// pointer, and pointers compared for equality.
void swap(int *a, int *b) {
    int t = *a;
    *a = *b;
    *b = t;
}

void set_through(int **pp, int *target) {
    *pp = target;
}

int main() {
    int x = 3;
    int y = 7;
    swap(&x, &y);                 // x = 7, y = 3
    int *p = &x;
    *p = *p * 10;                 // x = 70
    int **pp = &p;
    **pp = **pp + 1;              // x = 71
    set_through(pp, &y);          // p = &y
    *p = *p + 20;                 // y = 23
    int *null = 0;
    int r = x + y;                // 94
    if (p == &y && null == 0 && p != null && !null)
        r = r - 1;                // 93
    return r;
}
