/* wc [FILE...]: counts the lines, words and bytes of each file, or of its
 * input if there is none, and prints them: `LINES WORDS BYTES`, then the
 * file's name if it has one. Exits 1 if a file can't be read, else 0. */
#include "user.h"

static char buf[512];

static int count(int fd, const char *name) {
    uint lines = 0;
    uint words = 0;
    uint bytes = 0;
    int in_word = 0;
    for (;;) {
        int n = read(fd, buf, sizeof buf);
        if (n == 0) {
            break;
        }
        if (n < 0) {
            fprintf(STDERR, "wc: %s: %s\n", name != NULL ? name : "input", strerror(-n));
            return 1;
        }
        for (int i = 0; i < n; i++) {
            char c = buf[i];
            lines += c == '\n';
            if (c == ' ' || c == '\n' || c == '\t' || c == '\r') {
                in_word = 0;
            } else if (!in_word) {
                in_word = 1;
                words++;
            }
        }
        bytes += (uint)n;
    }
    if (name != NULL) {
        printf("%u %u %u %s\n", lines, words, bytes, name);
    } else {
        printf("%u %u %u\n", lines, words, bytes);
    }
    return 0;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        return count(STDIN, NULL);
    }
    int status = 0;
    for (int i = 1; i < argc; i++) {
        int fd = open(argv[i], O_RDONLY);
        if (fd < 0) {
            fprintf(STDERR, "wc: %s: %s\n", argv[i], strerror(-fd));
            status = 1;
            continue;
        }
        status |= count(fd, argv[i]);
        close(fd);
    }
    return status;
}
