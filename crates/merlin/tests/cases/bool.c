// expect: 15
// _Bool: every conversion to it gives 0 or 1.

_Bool flag;

_Bool is_odd(int x) {
    return x & 1;
}

int main(void) {
    int ok = 0;
    _Bool b = 256; /* a char would be 0 */
    _Bool c = 0;
    int *p = &ok;
    _Bool hasp = p;
    _Bool bits[3] = {5, 0, -1};
    ok += b == 1;
    ok += c == 0;
    ok += hasp == 1;
    ok += is_odd(7) == 1 && is_odd(8) == 0;
    ok += sizeof(_Bool) == 1;
    b = -3;
    ok += b == 1;
    c++;
    ok += c == 1;
    c++;
    ok += c == 1;
    c--;
    ok += c == 0;
    c--; /* 0 - 1 is -1: true */
    ok += c == 1;
    ok += (c++ == 1) && c == 1;
    ok += (_Bool)0x100 == 1;
    flag = ok > 3;
    ok += flag;
    b = 2;
    ok += b + b == 2;
    ok += bits[0] + bits[1] + bits[2] == 2;
    return ok;
}
