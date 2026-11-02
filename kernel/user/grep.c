/* grep [-v] [-c] [-n] PATTERN [FILE...]: prints the lines of each file, or
 * of its input if there is none, that contain PATTERN, a fixed string
 * (no regular expressions). With more than one file, each line starts with
 * its file's name and a ':'.
 *
 *   -v  prints the lines that do not contain it instead
 *   -c  prints how many lines would be printed instead of the lines
 *       (`NAME:COUNT` with more than one file)
 *   -n  puts the line's number and a ':' before each line
 *
 * Exits 0 if a line was selected, 1 if none was, 2 on an error (a bad
 * option, a file that can't be read), as Unix grep does. Lines can be of
 * any length: the line buffer grows with malloc. */
#include "user.h"

struct options {
    int invert;
    int count;
    int number;
    int names; /* more than one file: say which */
};

/* The line being looked at: `len` bytes in `buf` (`cap` bytes, from malloc),
 * its newline included if it has one. */
struct line {
    char *buf;
    int cap;
    int len;
};

/* Reads the next whole line into `l`, growing its buffer as needed.
 * Returns its length, 0 at the end of the input, or a negative error
 * number (-ENOMEM when the buffer can't grow). */
static int next_line(struct reader *r, struct line *l) {
    l->len = 0;
    for (;;) {
        if (l->cap - l->len < 2) {
            int cap = l->cap == 0 ? 128 : 2 * l->cap;
            char *bigger = realloc(l->buf, (size_t)cap);
            if (bigger == NULL) {
                return -ENOMEM;
            }
            l->buf = bigger;
            l->cap = cap;
        }
        int n = reader_getline(r, l->buf + l->len, l->cap - l->len);
        if (n < 0) {
            return n;
        }
        l->len += n;
        if (n == 0 || l->buf[l->len - 1] == '\n') {
            return l->len;
        }
        /* A piece of a longer line: the buffer was full. */
    }
}

/* Whether the `len` bytes at `s` contain `pat` (`plen` bytes). */
static int contains(const char *s, int len, const char *pat, int plen) {
    for (int i = 0; i + plen <= len; i++) {
        if (memcmp(s + i, pat, (size_t)plen) == 0) {
            return 1;
        }
    }
    return 0;
}

/* Greps `fd` (called `name`). Returns the lines selected, or -1 having
 * said why it couldn't read on. */
static int grep(int fd, const char *name, const char *pat, const struct options *o, struct line *l) {
    struct reader r;
    reader_init(&r, fd);
    int plen = (int)strlen(pat);
    int selected = 0;
    for (int number = 1;; number++) {
        int n = next_line(&r, l);
        if (n == 0) {
            break;
        }
        if (n < 0) {
            fprintf(STDERR, "grep: %s: %s\n", name, strerror(-n));
            return -1;
        }
        int body = l->buf[n - 1] == '\n' ? n - 1 : n;
        if (contains(l->buf, body, pat, plen) == o->invert) {
            continue;
        }
        selected++;
        if (o->count) {
            continue;
        }
        if (o->names) {
            printf("%s:", name);
        }
        if (o->number) {
            printf("%d:", number);
        }
        /* The line as it is (it may hold any byte), with a newline. */
        if (write(STDOUT, l->buf, (uint)body) != body || write(STDOUT, "\n", 1) != 1) {
            fprintf(STDERR, "grep: cannot write the output\n");
            return -1;
        }
    }
    if (o->count) {
        if (o->names) {
            printf("%s:", name);
        }
        printf("%d\n", selected);
    }
    return selected;
}

static int usage(void) {
    fprintf(STDERR, "usage: grep [-v] [-c] [-n] PATTERN [FILE...]\n");
    return 2;
}

int main(int argc, char **argv) {
    struct options o;
    memset(&o, 0, sizeof o);
    int i = 1;
    for (; i < argc && argv[i][0] == '-' && argv[i][1] != 0; i++) {
        for (const char *f = argv[i] + 1; *f; f++) {
            if (*f == 'v') {
                o.invert = 1;
            } else if (*f == 'c') {
                o.count = 1;
            } else if (*f == 'n') {
                o.number = 1;
            } else {
                fprintf(STDERR, "grep: unknown option -%c\n", *f);
                return usage();
            }
        }
    }
    if (i == argc) {
        return usage();
    }
    const char *pat = argv[i];
    i++;
    o.names = argc - i > 1;
    struct line l;
    l.buf = NULL;
    l.cap = 0;
    l.len = 0;
    int found = 0;
    int error = 0;
    if (i == argc) {
        int n = grep(STDIN, "input", pat, &o, &l);
        found = n > 0;
        error = n < 0;
    }
    for (; i < argc; i++) {
        int fd = open(argv[i], O_RDONLY);
        if (fd < 0) {
            fprintf(STDERR, "grep: %s: %s\n", argv[i], strerror(-fd));
            error = 1;
            continue;
        }
        struct stat st;
        if (fstat(fd, &st) == 0 && st.type == T_DIR) {
            fprintf(STDERR, "grep: %s: %s\n", argv[i], strerror(EISDIR));
            error = 1;
            close(fd);
            continue;
        }
        int n = grep(fd, argv[i], pat, &o, &l);
        close(fd);
        if (n > 0) {
            found = 1;
        }
        if (n < 0) {
            error = 1;
        }
    }
    free(l.buf);
    if (error) {
        return 2;
    }
    return found ? 0 : 1;
}
