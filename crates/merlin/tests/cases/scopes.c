// expect: 123
// Inner blocks may shadow a name without touching the outer variable.
int main() {
    int x = 1;
    int r = 0;
    {
        int x = 2;
        {
            int x = 3;
            r = r + x;     // 3
            x = 9;
        }
        r = r + x * 10;    // 20
    }
    return r + x * 100;    // 100
}
