/* wc [-l] [-w] [-c] [FILE...]: counts the lines, words and bytes of each
 * file, or of its input if there is none, and prints them: `LINES WORDS
 * BYTES`, then the file's name if it has one. -l, -w and -c print only
 * those counts (in that order), so `wc -l` is the number of lines. Exits 1
 * if a file can't be read or an option is unknown, else 0. */
#include "user.h"

static char buf[512];

/* Which counts to print. */
static int show_lines;
static int show_words;
static int show_bytes;

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
    /* The counts asked for, a space between them: put together first, so
     * the line is printed with one write. */
    char counts[36];
    int len = 0;
    if (show_lines) {
        len += snprintf(counts + len, sizeof counts - (size_t)len, " %u", lines);
    }
    if (show_words) {
        len += snprintf(counts + len, sizeof counts - (size_t)len, " %u", words);
    }
    if (show_bytes) {
        snprintf(counts + len, sizeof counts - (size_t)len, " %u", bytes);
    }
    if (name != NULL) {
        printf("%s %s\n", counts + 1, name);
    } else {
        printf("%s\n", counts + 1);
    }
    return 0;
}

int main(int argc, char **argv) {
    int i = 1;
    for (; i < argc && argv[i][0] == '-' && argv[i][1] != 0; i++) {
        for (const char *f = argv[i] + 1; *f; f++) {
            if (*f == 'l') {
                show_lines = 1;
            } else if (*f == 'w') {
                show_words = 1;
            } else if (*f == 'c') {
                show_bytes = 1;
            } else {
                fprintf(STDERR, "wc: unknown option -%c\nusage: wc [-l] [-w] [-c] [FILE...]\n", *f);
                return 1;
            }
        }
    }
    if (!show_lines && !show_words && !show_bytes) {
        show_lines = 1;
        show_words = 1;
        show_bytes = 1;
    }
    if (i == argc) {
        return count(STDIN, NULL);
    }
    int status = 0;
    for (; i < argc; i++) {
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
