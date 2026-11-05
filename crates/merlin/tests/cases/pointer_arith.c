// expect: 131
// Pointer arithmetic counts elements, whatever their size; the difference of
// two pointers counts elements too. (A long is 8 bytes.)
int main() {
    char c[8];
    short s[8];
    int i[8];
    long l[8];
    char *pc = c + 3;
    short *ps = s + 3;
    int *pi = i + 3;
    long *pl = l + 3;
    // Byte distances: 3, 6, 12, 24.
    int bytes = ((char *)pc - (char *)c) + ((char *)ps - (char *)s) + ((char *)pi - (char *)i)
                + ((char *)pl - (char *)l);                       // 45
    int k;
    for (k = 0; k < 8; k++)
        i[k] = k * k;
    int *q = i;
    q += 2;                                                        // &i[2]
    int sum = *q + q[1] + *(q + 2) + 3[q] + *(1 + q);             // 4 + 9 + 16 + 25 + 9 = 63
    int *end = &i[8];                                              // one past the end
    int n = end - q;                                               // 6
    q--;                                                           // &i[1]
    int before = *--q;                                             // i[0] = 0
    int after = *q++;                                              // 0, then q = &i[1]
    int here = *q;                                                 // 1
    int order = (q < end) + (end > q) + (q <= q) + (end >= end) + (q > end);   // 4
    return bytes + sum + n + before + after + here + order + (pc - c) * 4;     // 45+63+6+0+0+1+4+12
}
