// expect: 66
// A frame over 2 KiB: locals beyond a 12-bit offset from s0 are reached
// through t0, at every access width, and an initialized array of more than
// 16 words is cleared with a loop before its elements are stored.
int main() {
    int big[700] = {1, 2, 3};          // 2800 bytes: cleared by a loop
    char buf[3001] = "hello";          // the rest of it zero
    short tail = -300;                 // lh/sh at large offsets
    unsigned short us = 60000;         // lhu
    signed char sc = -5;               // lb/sb
    unsigned char uc = 250;            // lbu
    int r = 0;
    big[699] = 7;
    buf[3000] = 9;
    for (int i = 0; i < 700; i++)
        r += big[i];                   // 13
    for (int i = 0; i < 3001; i++)
        r += buf[i];                   // 104+101+108+108+111 + 9 = 541 -> 554
    r += tail + 300;                   // 554
    r += us - 59990;                   // 564
    r += sc + 5;                       // 564
    r += uc - 240;                     // 574
    tail = tail * 2;
    us++;
    sc = sc * 2;
    uc++;
    r += (tail == -600) + (us == 60001) + (sc == -10) + (uc == 251);   // 578
    return r;                          // 578 & 255 = 66
}
