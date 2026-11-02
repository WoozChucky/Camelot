// expect: 7
// goto and labels: forward and backward, out of nested loops, into a loop.

int find(int *a, int n, int want) {
    int i;
    for (i = 0; i < n; i++)
        if (a[i] == want)
            goto found;
    return -1;
found:
    return i;
}

int count_down(int n) {
    int steps = 0;
again:
    if (n > 0) {
        n--;
        steps++;
        goto again;
    }
    return steps;
}

int nested_exit(void) {
    int i, j, hits = 0;
    for (i = 0; i < 10; i++)
        for (j = 0; j < 10; j++) {
            hits++;
            if (i * j == 12)
                goto out;
        }
out:
    return hits;
}

int into_loop(void) {
    int i = 5, s = 0;
    goto inside;
    while (i < 8) {
        s += 100;
    inside:
        s += i;
        i++;
    }
    return s;
}

/* The same label names in another function are other labels. */
int again_and_out(int x) {
    if (x)
        goto out;
    x = 10;
again:
    x++;
out:
    if (x < 3)
        goto again;
    return x;
}

int main(void) {
    int ok = 0;
    int a[5] = {4, 8, 15, 16, 23};
    ok += find(a, 5, 15) == 2;
    ok += find(a, 5, 42) == -1;
    ok += count_down(7) == 7;
    ok += nested_exit() == 27;
    ok += into_loop() == 218;
    ok += again_and_out(1) == 3;
    ok += again_and_out(0) == 11;
end:;
    return ok;
}
