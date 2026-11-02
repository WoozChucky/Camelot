// expect: 5
// A shift by a negative amount or by 32 or more is undefined in C; merlin
// uses the low 5 bits of the amount, as the hardware does.
int id(int x) { return x; }

int main() {
    int r = 0;
    r += (1 << id(32)) == 1;
    r += (1 << id(33)) == 2;
    r += (0x100 >> id(36)) == 0x10;
    unsigned u = 0x80000000;
    r += (u >> id(63)) == 1;
    r += (1 << id(-1)) == -2147483647 - 1;
    return r;
}
