/* stty: a terminal's size, as Linux's stty shows and sets it (#185), on
 * standard input's terminal:
 *
 *   stty                        rows R; columns C;
 *   stty size                   R C
 *   stty rows N | cols N ...    sets them (TIOCSWINSZ; columns is cols too)
 *
 * The size is what the terminal's emulator said it is (term tells the
 * pseudo-terminal each time its window is resized), which full-screen
 * programs lay their lines out by. Exits 1 when standard input is no
 * terminal, 2 on a usage error. */
#include "user.h"

static int usage(void) {
    fprintf(2, "usage: stty [size | rows N | cols N | columns N]...\n");
    return 2;
}

/* A size's count: a decimal number from 0 to 65535, else -1. */
static long count(const char *s) {
    char *end;
    long n = strtol(s, &end, 10);
    if (end == s || *end != 0 || n < 0 || n > 65535) {
        return -1;
    }
    return n;
}

int main(int argc, char **argv) {
    struct winsize ws;
    if (ioctl(0, TIOCGWINSZ, &ws) < 0) {
        fprintf(2, "stty: standard input: not a terminal\n");
        return 1;
    }
    if (argc == 1) {
        printf("rows %d; columns %d;\n", ws.ws_row, ws.ws_col);
        return 0;
    }
    if (argc == 2 && strcmp(argv[1], "size") == 0) {
        printf("%d %d\n", ws.ws_row, ws.ws_col);
        return 0;
    }
    for (int i = 1; i < argc; i += 2) {
        if (i + 1 >= argc) {
            return usage();
        }
        long n = count(argv[i + 1]);
        if (n < 0) {
            return usage();
        }
        if (strcmp(argv[i], "rows") == 0) {
            ws.ws_row = (ushort)n;
        } else if (strcmp(argv[i], "cols") == 0 || strcmp(argv[i], "columns") == 0) {
            ws.ws_col = (ushort)n;
        } else {
            return usage();
        }
    }
    if (ioctl(0, TIOCSWINSZ, &ws) < 0) {
        fprintf(2, "stty: can't set the size\n");
        return 1;
    }
    return 0;
}
