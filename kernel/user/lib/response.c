/* Response files: `@FILE` on a command line stands for the arguments
 * written in FILE, read as clang and LLVM's tools read them (LLVM's GNU
 * rules), the one way every tool of the toolchain reads them: `cc`, `cc1`,
 * `as`'s neighbours `ld` and `ar` here, and merlin and merlin-ld on the host
 * (excalibur::response, whose rules these follow, and whose tests and
 * selfhost.rs's hold the two to each other). Camelot's exec passes at most
 * MAXARG arguments, so a long command line is given in one.
 *
 * The rules: arguments are separated by blanks, tabs, carriage returns and
 * line feeds; a backslash takes the character after it as it is; '...'
 * and "..." quote (a backslash still escapes inside them, and "" is an
 * empty argument); an `@FILE` in a response file is expanded too, FILE
 * named from the working directory; an `@FILE` whose FILE does not exist
 * is left as it is; a FILE that can't be read (a directory) is an error,
 * and so is one that names itself, directly or through others. */
#include "../user.h"

/* A growing argument list, NULL after the last. */
struct list {
    char **v;
    int n;
    int cap;
};

static int list_push(struct list *l, char *s) {
    if (l->n + 1 >= l->cap) {
        int cap = l->cap == 0 ? 16 : 2 * l->cap;
        char **v = realloc(l->v, (size_t)cap * sizeof(char *));
        if (v == NULL) {
            return -ENOMEM;
        }
        l->v = v;
        l->cap = cap;
    }
    l->v[l->n++] = s;
    l->v[l->n] = NULL;
    return 0;
}

/* Whether `c` separates two arguments in a response file. */
static int is_separator(char c) {
    return c == ' ' || c == '\t' || c == '\r' || c == '\n';
}

/* `path` made absolute from the working directory, `.` and `..` taken out
 * by their names, from malloc (NULL without memory). */
static char *absolute_path(const char *path) {
    char cwd[MAXPATH + 1];
    size_t clen = 0;
    if (path[0] != '/') {
        long r = getcwd(cwd, sizeof cwd);
        clen = r > 0 ? (size_t)r - 1 : 0;
    }
    size_t plen = strlen(path);
    char *full = malloc(clen + 1 + plen + 2);
    if (full == NULL) {
        return NULL;
    }
    memcpy(full, cwd, clen);
    full[clen] = '/';
    memcpy(full + clen + 1, path, plen + 1);
    /* In place: each name copied down after a '/', `.` dropped, `..`
     * taking the last name off. */
    size_t len = strlen(full);
    size_t o = 0;
    for (size_t i = 0; i < len;) {
        while (i < len && full[i] == '/') {
            i++;
        }
        size_t start = i;
        while (i < len && full[i] != '/') {
            i++;
        }
        size_t n = i - start;
        if (n == 0 || (n == 1 && full[start] == '.')) {
            continue;
        }
        if (n == 2 && full[start] == '.' && full[start + 1] == '.') {
            while (o > 0) {
                o--;
                if (full[o] == '/') {
                    break;
                }
            }
            continue;
        }
        full[o] = '/';
        memmove(full + o + 1, full + start, n);
        o += 1 + n;
    }
    if (o == 0) {
        full[o++] = '/';
    }
    full[o] = 0;
    return full;
}

/* The arguments in `text` (NUL-terminated), each pushed onto `out`, by the
 * rules above; the text is rewritten in place to hold them. 0, or
 * -ENOMEM. */
static int tokenize(char *text, struct list *out) {
    char *p = text;
    for (;;) {
        while (*p != 0 && is_separator(*p)) {
            p++;
        }
        if (*p == 0) {
            return 0;
        }
        /* The argument is written over its own text, which it never
         * outgrows: quotes and backslashes only take bytes out. */
        char *arg = p;
        char *w = p;
        while (*p != 0 && !is_separator(*p)) {
            char c = *p;
            if (c == '\\' && p[1] != 0) {
                *w++ = p[1];
                p += 2;
            } else if (c == '"' || c == '\'') {
                p++;
                while (*p != 0 && *p != c) {
                    if (*p == '\\' && p[1] != 0) {
                        p++;
                    }
                    *w++ = *p++;
                }
                if (*p == c) {
                    p++;
                }
            } else {
                *w++ = *p++;
            }
        }
        /* Past the argument's end, which may be its separator. */
        char *next = *p != 0 ? p + 1 : p;
        *w = 0;
        int r = list_push(out, arg);
        if (r < 0) {
            return r;
        }
        p = next;
    }
}

/* The whole of file descriptor `fd`, NUL-terminated, from malloc; NULL
 * with *err. */
static char *read_all(int fd, int *err) {
    size_t cap = 1024;
    size_t len = 0;
    char *text = malloc(cap);
    if (text == NULL) {
        *err = -ENOMEM;
        return NULL;
    }
    for (;;) {
        if (len + 1 == cap) {
            cap *= 2;
            char *t = realloc(text, cap);
            if (t == NULL) {
                free(text);
                *err = -ENOMEM;
                return NULL;
            }
            text = t;
        }
        long n = read(fd, text + len, cap - 1 - len);
        if (n < 0) {
            free(text);
            *err = (int)n;
            return NULL;
        }
        if (n == 0) {
            break;
        }
        len += (size_t)n;
    }
    text[len] = 0;
    return text;
}

/* The message of a failed expansion, formatted into `error`. */
static void say(char *error, size_t size, const char *what, const char *path, const char *why) {
    if (why != NULL) {
        snprintf(error, size, "%s '%s': %s", what, path, why);
    } else {
        snprintf(error, size, "%s '%s'", what, path);
    }
}

/* Pushes `arg` onto `out`, or for `@FILE` FILE's arguments, each expanded
 * in turn; `reading` holds the files being read, by absolute path. 0, or -1
 * with the message in `error`. */
static int expand(char *arg, struct list *out, struct list *reading, char *error, size_t size) {
    int fd = arg[0] == '@' && arg[1] != 0 ? open(arg + 1, O_RDONLY) : -ENOENT;
    if (fd == -ENOENT) {
        /* Not `@FILE`, or no such file: an argument like any other. */
        if (list_push(out, arg) < 0) {
            snprintf(error, size, "out of memory");
            return -1;
        }
        return 0;
    }
    const char *file = arg + 1;
    char *path = absolute_path(file);
    if (path == NULL) {
        snprintf(error, size, "out of memory");
        return -1;
    }
    if (fd < 0) {
        say(error, size, "cannot not open file", path, strerror(-fd));
        return -1;
    }
    struct stat st;
    if (fstat(fd, &st) == 0 && st.type == T_DIR) {
        close(fd);
        say(error, size, "cannot not open file", path, "is a directory");
        return -1;
    }
    int err = 0;
    char *text = read_all(fd, &err);
    close(fd);
    if (text == NULL) {
        say(error, size, "cannot not open file", path, strerror(-err));
        return -1;
    }
    for (int i = 0; i < reading->n; i++) {
        if (strcmp(reading->v[i], path) == 0) {
            say(error, size, "recursive expansion of:", path, NULL);
            return -1;
        }
    }
    struct list words;
    memset(&words, 0, sizeof words);
    if (list_push(reading, path) < 0 || tokenize(text, &words) < 0) {
        snprintf(error, size, "out of memory");
        return -1;
    }
    for (int i = 0; i < words.n; i++) {
        if (expand(words.v[i], out, reading, error, size) < 0) {
            return -1;
        }
    }
    free(words.v);
    reading->n--;
    return 0;
}

char **expand_response_files(int argc, char **argv, int *n, char *error, size_t size) {
    struct list out;
    struct list reading;
    memset(&out, 0, sizeof out);
    memset(&reading, 0, sizeof reading);
    /* argv[0] is the program's name, never a response file. */
    if (argc > 0 && list_push(&out, argv[0]) < 0) {
        snprintf(error, size, "out of memory");
        return NULL;
    }
    for (int i = 1; i < argc; i++) {
        if (expand(argv[i], &out, &reading, error, size) < 0) {
            return NULL;
        }
    }
    *n = out.n;
    return out.v;
}

char *response_file_text(char **args, int n) {
    size_t len = 1;
    for (int i = 0; i < n; i++) {
        len += 2 * strlen(args[i]) + 3;
    }
    char *text = malloc(len);
    if (text == NULL) {
        return NULL;
    }
    size_t at = 0;
    for (int i = 0; i < n; i++) {
        text[at++] = '"';
        for (const char *s = args[i]; *s != 0; s++) {
            if (*s == '"' || *s == '\\') {
                text[at++] = '\\';
            }
            text[at++] = *s;
        }
        text[at++] = '"';
        text[at++] = ' ';
    }
    text[at] = 0;
    return text;
}
