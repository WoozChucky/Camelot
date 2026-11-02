/* mkdir DIR...: makes each directory. Exits 1 if one can't be made. */
#include "user.h"

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(STDERR, "usage: mkdir DIR...\n");
        return 1;
    }
    int status = 0;
    for (int i = 1; i < argc; i++) {
        int r = mkdir(argv[i]);
        if (r < 0) {
            fprintf(STDERR, "mkdir: %s: %s\n", argv[i], strerror(-r));
            status = 1;
        }
    }
    return status;
}
