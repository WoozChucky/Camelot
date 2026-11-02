/* cat [FILE...]: copies each file, or its input if there is none, to its
 * output. Exits 1 if a file can't be opened or read (a directory is
 * refused) or the output written (a pipe whose reader went away), else 0. */
#include "user.h"

static char buf[512];

/* Copies `fd` to the output. Returns 0, or 1 having said why. */
static int copy(int fd, const char *name) {
    for (;;) {
        int n = read(fd, buf, sizeof buf);
        if (n == 0) {
            return 0;
        }
        if (n < 0) {
            fprintf(STDERR, "cat: %s: %s\n", name, strerror(-n));
            return 1;
        }
        int w = write(STDOUT, buf, (uint)n);
        if (w != n) {
            fprintf(STDERR, "cat: %s\n", strerror(w < 0 ? -w : ENOSPC));
            return 1;
        }
    }
}

int main(int argc, char **argv) {
    if (argc < 2) {
        return copy(STDIN, "input");
    }
    for (int i = 1; i < argc; i++) {
        int fd = open(argv[i], O_RDONLY);
        if (fd < 0) {
            fprintf(STDERR, "cat: %s: %s\n", argv[i], strerror(-fd));
            return 1;
        }
        /* A directory's entries are not text to copy. */
        struct stat st;
        if (fstat(fd, &st) == 0 && st.type == T_DIR) {
            fprintf(STDERR, "cat: %s: %s\n", argv[i], strerror(EISDIR));
            close(fd);
            return 1;
        }
        int r = copy(fd, argv[i]);
        close(fd);
        if (r != 0) {
            return r;
        }
    }
    return 0;
}
