// expect: 15
// A shift by the width or more is undefined in C. In a constant clang's
// front end gives a value of (a static initializer, a case label, an
// enumerator, an array's size), its evaluator shifts by one less than the
// width, and by a negative count the other way; so does merlin.
int g = 1 << 33;
long long h = 1LL << 70;
enum { E = 8 >> -2 };
char a[(1 << 40) < 0 ? 4 : 1];

int pick(int x) {
    switch (x) {
    case 1 << 40:
        return 1;
    case 2:
        return 2;
    }
    return 0;
}

int main(void) {
    int r = 0;
    r += g == -2147483647 - 1;        // 1
    r += h == -9223372036854775807LL - 1; // 1
    r += E == 32;                     // 1
    r += sizeof a;                    // 4
    r += pick(-2147483647 - 1) * 4;   // 4
    r += pick(2) * 2;                 // 4
    return r;
}
