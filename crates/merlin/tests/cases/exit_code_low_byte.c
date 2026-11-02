// expect: 44
// The exit code is the low 8 bits of main's return value: 300 & 0xff = 44.
int main() {
    return 300;
}
