/* echo [-n] [ARG...]: prints its arguments, separated by spaces, and a
 * newline (not with -n). Exits 1 if the output can't be written (a pipe
 * nobody reads), else 0. */
#include "user.h"

int main(int argc, char **argv) {
    int newline = 1;
    int i = 1;
    if (i < argc && strcmp(argv[i], "-n") == 0) {
        newline = 0;
        i++;
    }
    int first = i;
    int r = 0;
    for (; i < argc && r >= 0; i++) {
        r = printf(i > first ? " %s" : "%s", argv[i]);
    }
    if (newline && r >= 0) {
        r = printf("\n");
    }
    if (r < 0) {
        fprintf(STDERR, "echo: %s\n", strerror(-r));
        return 1;
    }
    return 0;
}
