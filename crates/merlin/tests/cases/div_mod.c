// expect: 15
// Division truncates toward zero; the remainder takes the dividend's sign.
int main() {
    int r = 0;
    r = r + (-7 / 2 == -3);
    r = r + 2 * (-7 % 2 == -1);
    r = r + 4 * (7 / -2 == -3);
    r = r + 8 * (7 % -2 == 1);
    return r;
}
