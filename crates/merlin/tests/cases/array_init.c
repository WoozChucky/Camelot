// expect: 106
// Initializer lists: elements left out are zero, the size can come from the
// initializer, nested braces for nested arrays, and strings for char arrays.
int g[5] = {10, 20};
int inferred[] = {1, 2, 3, 4};
int grid[2][3] = {{1, 2, 3}, {4}};
char word[8] = "hey";

int main() {
    int a[6] = {5, 6, 7};
    char s[] = "abc";
    char exact[3] = "xyz";                     // no room for the NUL: allowed
    short h[4] = {-1, 300};
    int total = 0;
    for (int k = 0; k < 6; k++)
        total += a[k];                         // 18
    total += g[0] + g[1] + g[2] + g[4];        // 30 -> 48
    total += sizeof inferred / sizeof inferred[0] + inferred[3];   // 4 + 4 -> 56
    total += grid[1][0] + grid[1][2] + grid[0][2];                 // 4 + 0 + 3 -> 63
    total += sizeof s + (s[3] == 0) + (s[0] == 'a');               // 4 + 1 + 1 -> 69
    total += (word[2] == 'y') + (word[3] == 0) + (word[7] == 0);   // 3 -> 72
    total += (exact[2] == 'z') + sizeof exact;                     // 1 + 3 -> 76
    total += h[0] + h[1] / 10 + h[3];                              // -1 + 30 + 0 -> 105
    return total + (h[2] == 0);                                    // 106
}
