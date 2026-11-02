/* ln OLD NEW: gives the file OLD a second name, NEW (a hard link). Exits 1
 * if it can't. */
#include "user.h"

int main(int argc, char **argv) {
    if (argc != 3) {
        fprintf(STDERR, "usage: ln OLD NEW\n");
        return 1;
    }
    int r = link(argv[1], argv[2]);
    if (r < 0) {
        fprintf(STDERR, "ln: %s: %s\n", argv[2], strerror(-r));
        return 1;
    }
    return 0;
}
