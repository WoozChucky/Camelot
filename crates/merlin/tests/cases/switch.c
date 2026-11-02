// expect: 16
// switch: case values, fall-through, default, nesting, break and continue,
// and case labels inside other statements (Duff's device).

int classify(int x) {
    switch (x) {
    case 0:
        return 10;
    case 1:
    case 2:
        return 20;
    case -5:
        return 30;
    case 100: {
        int y = x / 10;
        return y;
    }
    default:
        return -1;
    }
}

int fall(int x) {
    int r = 0;
    switch (x) {
    case 1:
        r += 1;
    case 2:
        r += 2;
    case 3:
        r += 3;
        break;
    case 4:
        r += 4;
    }
    return r;
}

int nested(int a, int b) {
    switch (a) {
    case 1:
        switch (b) {
        case 1:
            return 11;
        case 2:
            break;
        default:
            return 19;
        }
        return 12;
    case 2:
        while (b < 10) {
            b++;
            if (b == 5)
                break; /* leaves the loop, not the switch */
        }
        return b;
    }
    return 0;
}

int loop_continue(void) {
    int i, s = 0;
    for (i = 0; i < 6; i++) {
        switch (i % 3) {
        case 0:
            continue; /* the loop's */
        case 1:
            s += 1;
            break;
        default:
            s += 10;
        }
        s += 100;
    }
    return s;
}

void copy(char *to, const char *from, int count) {
    int n = (count + 3) / 4;
    switch (count % 4) {
    case 0:
        do {
            *to++ = *from++;
        case 3:
            *to++ = *from++;
        case 2:
            *to++ = *from++;
        case 1:
            *to++ = *from++;
        } while (--n > 0);
    }
}

int kind(unsigned u) {
    switch (u) {
    case 0xffffffff:
        return 1;
    case 7u:
        return 2;
    }
    return 3;
}

int letter(char c) {
    switch (c) {
    case 'a':
    case 'e':
        return 1;
    case 200:
        return 2;
    }
    return 0;
}

int main(void) {
    int ok = 0;
    char dst[8];
    ok += classify(0) == 10 && classify(2) == 20 && classify(-5) == 30;
    ok += classify(100) == 10 && classify(7) == -1;
    ok += fall(1) == 6;
    ok += fall(2) == 5;
    ok += fall(4) == 4 && fall(9) == 0;
    ok += nested(1, 1) == 11;
    ok += nested(1, 2) == 12;
    ok += nested(1, 3) == 19;
    ok += nested(2, 0) == 5;
    ok += nested(3, 0) == 0;
    ok += loop_continue() == 422;
    copy(dst, "abcdefg", 7);
    ok += dst[0] == 'a' && dst[6] == 'g';
    copy(dst, "xyz", 2);
    ok += dst[1] == 'y' && dst[2] == 'c';
    ok += kind(-1) == 1 && kind(7) == 2 && kind(8) == 3;
    ok += letter('e') == 1 && letter((char)200) == 2 && letter('z') == 0;
    switch (ok) {
    default:
        ok++;
    }
    return ok;
}
