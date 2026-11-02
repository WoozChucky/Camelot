// expect: 51
// String literals and character constants, with escapes; strings are char
// arrays ending in a NUL.
int length(char *s) {
    int n = 0;
    while (*s++)
        n++;
    return n;
}

int same(char *a, char *b) {
    while (*a && *a == *b) {
        a++;
        b++;
    }
    return *a == *b;
}

int main() {
    char *greeting = "hello, " "world\n";               // adjacent literals join
    int r = length(greeting);                           // 13
    r += same("abc", "abc") + !same("abc", "abd");      // 2 -> 15
    r += '\n' + '\t' + '\0' + '\\' + '\'' - 160;        // 10 + 9 + 0 + 92 + 39 - 160 -> 5
    r += "\x41\102C"[2] - 'C' + "tab\there"[3];         // 0 + 9 -> 14
    r += sizeof "four" + sizeof("");                    // 5 + 1 -> 20
    r += (greeting[12] == '\n') + ("q"[1] == 0);        // 2 -> 22
    char quote = '"';
    r += quote + "\"\\"[1] - 97;                        // 34 + 92 - 97 -> 51
    return r;
}
