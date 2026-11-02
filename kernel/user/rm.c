/* rm PATH...: removes each name: a file loses that link (and goes once it
 * has none and nobody has it open), a directory must be empty. Exits 1 if
 * one can't be removed. */
#include "user.h"

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(STDERR, "usage: rm PATH...\n");
        return 1;
    }
    int status = 0;
    for (int i = 1; i < argc; i++) {
        int r = unlink(argv[i]);
        if (r < 0) {
            fprintf(STDERR, "rm: %s: %s\n", argv[i], strerror(-r));
            status = 1;
        }
    }
    return status;
}
