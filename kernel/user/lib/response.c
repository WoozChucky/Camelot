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
 * is left as it is; a FILE that can't be read (a directory, as `@` alone
 * names the working directory) is an error, and so is one that names
 * itself, directly or through others. A file's UTF-8 byte-order mark is
 * left out and UTF-16 text (with its byte-order mark) made UTF-8; a NUL
 * byte ends the argument it is in, and the reading goes on after it. */
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

/* The working directory, from malloc, its buffer grown until the path fits
 * (a page is the kernel's limit); "/" if it can't be found. */
static char *working_dir(void) {
    for (size_t size = 256; size <= 4096; size *= 2) {
        char *buf = malloc(size);
        if (buf == NULL) {
            return NULL;
        }
        long r = getcwd(buf, size);
        if (r > 0) {
            return buf;
        }
        free(buf);
        if (r != -ERANGE) {
            break;
        }
    }
    char *root = malloc(2);
    if (root != NULL) {
        root[0] = '/';
        root[1] = 0;
    }
    return root;
}

/* `path` made absolute from the working directory, `.` and `..` taken out
 * by their names, from malloc (NULL without memory). `@` alone names the
 * working directory itself (an empty `path`). */
static char *absolute_path(const char *path) {
    char *cwd = path[0] == '/' ? NULL : working_dir();
    if (path[0] != '/' && cwd == NULL) {
        return NULL;
    }
    size_t clen = cwd != NULL ? strlen(cwd) : 0;
    size_t plen = strlen(path);
    char *full = malloc(clen + 1 + plen + 2);
    if (full == NULL) {
        free(cwd);
        return NULL;
    }
    memcpy(full, cwd != NULL ? cwd : "", clen);
    free(cwd);
    full[clen] = '/';
    memcpy(full + clen + 1, path, plen + 1);
    /* In place: each name copied down after a '/', `.` dropped, `..`
     * taking the last name off. It never overtakes what it reads, as
     * `full` starts with a '/' (the working directory's or `path`'s). */
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

/* The arguments in text[0..len), each pushed onto `out`, by the rules
 * above; the text is rewritten in place to hold them. An argument ends at a
 * NUL byte in it (a C string), as LLVM's do; the reading goes on after it,
 * to `len`. 0, or -ENOMEM. */
static int tokenize(char *text, size_t len, struct list *out) {
    size_t i = 0;
    for (;;) {
        while (i < len && is_separator(text[i])) {
            i++;
        }
        if (i == len) {
            return 0;
        }
        /* The argument is written over its own text, which it never
         * outgrows: quotes and backslashes only take bytes out. */
        char *arg = text + i;
        size_t w = i;
        while (i < len && !is_separator(text[i])) {
            char c = text[i];
            if (c == '\\' && i + 1 < len) {
                text[w++] = text[i + 1];
                i += 2;
            } else if (c == '"' || c == '\'') {
                i++;
                while (i < len && text[i] != c) {
                    if (text[i] == '\\' && i + 1 < len) {
                        i++;
                    }
                    text[w++] = text[i++];
                }
                if (i < len) {
                    i++;
                }
            } else {
                text[w++] = text[i++];
            }
        }
        /* Past the argument's end, which may be its separator. */
        size_t next = i < len ? i + 1 : i;
        text[w] = 0;
        int r = list_push(out, arg);
        if (r < 0) {
            return r;
        }
        i = next;
    }
}

/* The whole of file descriptor `fd`, its length in *len, NUL-terminated,
 * from malloc; NULL with *err. */
static char *read_all(int fd, size_t *len, int *err) {
    size_t cap = 1024;
    size_t n = 0;
    char *text = malloc(cap);
    if (text == NULL) {
        *err = -ENOMEM;
        return NULL;
    }
    for (;;) {
        if (n + 1 == cap) {
            cap *= 2;
            char *t = realloc(text, cap);
            if (t == NULL) {
                free(text);
                *err = -ENOMEM;
                return NULL;
            }
            text = t;
        }
        long got = read(fd, text + n, cap - 1 - n);
        if (got < 0) {
            free(text);
            *err = (int)got;
            return NULL;
        }
        if (got == 0) {
            break;
        }
        n += (size_t)got;
    }
    text[n] = 0;
    *len = n;
    return text;
}

/* The bytes of a code point `c` in UTF-8 at `out`; how many. */
static size_t utf8(unsigned c, char *out) {
    if (c < 0x80) {
        out[0] = (char)c;
        return 1;
    }
    if (c < 0x800) {
        out[0] = (char)(0xc0 | (c >> 6));
        out[1] = (char)(0x80 | (c & 0x3f));
        return 2;
    }
    if (c < 0x10000) {
        out[0] = (char)(0xe0 | (c >> 12));
        out[1] = (char)(0x80 | ((c >> 6) & 0x3f));
        out[2] = (char)(0x80 | (c & 0x3f));
        return 3;
    }
    out[0] = (char)(0xf0 | (c >> 18));
    out[1] = (char)(0x80 | ((c >> 12) & 0x3f));
    out[2] = (char)(0x80 | ((c >> 6) & 0x3f));
    out[3] = (char)(0x80 | (c & 0x3f));
    return 4;
}

/* A response file's text as LLVM reads it (excalibur::response::decode): a
 * UTF-8 byte-order mark left out, and UTF-16 with one (either byte order)
 * made UTF-8, a lone surrogate U+FFFD. The text in *text and *len, which
 * may be a new one (from malloc, the old freed); 0, or -ENOMEM. */
static int decode(char **text, size_t *len) {
    unsigned char *b = (unsigned char *)*text;
    size_t n = *len;
    if (n >= 3 && b[0] == 0xef && b[1] == 0xbb && b[2] == 0xbf) {
        memmove(b, b + 3, n - 3 + 1);
        *len = n - 3;
        return 0;
    }
    int le = n >= 2 && b[0] == 0xff && b[1] == 0xfe;
    int be = n >= 2 && b[0] == 0xfe && b[1] == 0xff;
    if (!le && !be) {
        return 0;
    }
    /* At most 3 bytes of UTF-8 for each 2 of UTF-16. */
    char *out = malloc(n * 2 + 1);
    if (out == NULL) {
        return -ENOMEM;
    }
    size_t o = 0;
    for (size_t i = 2; i + 1 < n; i += 2) {
        unsigned u = le ? (unsigned)(b[i] | b[i + 1] << 8) : (unsigned)(b[i] << 8 | b[i + 1]);
        unsigned c = u;
        if (u >= 0xd800 && u < 0xdc00 && i + 3 < n) {
            unsigned lo = le ? (unsigned)(b[i + 2] | b[i + 3] << 8) : (unsigned)(b[i + 2] << 8 | b[i + 3]);
            if (lo >= 0xdc00 && lo < 0xe000) {
                c = 0x10000 + ((u - 0xd800) << 10) + (lo - 0xdc00);
                i += 2;
            } else {
                c = 0xfffd;
            }
        } else if (u >= 0xd800 && u < 0xe000) {
            c = 0xfffd;
        }
        o += utf8(c, out + o);
    }
    out[o] = 0;
    free(*text);
    *text = out;
    *len = o;
    return 0;
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
 * in turn; `reading` holds the files being read, by absolute path. `@`
 * alone names the working directory, as LLVM's name for it does (an error:
 * a directory). 0, or -1 with the message in `error`. */
static int expand(char *arg, struct list *out, struct list *reading, char *error, size_t size) {
    int fd = -ENOENT;
    if (arg[0] == '@') {
        fd = arg[1] != 0 ? open(arg + 1, O_RDONLY) : open(".", O_RDONLY);
    }
    if (fd == -ENOENT) {
        /* Not `@FILE`, or no such file: an argument like any other. */
        if (list_push(out, arg) < 0) {
            snprintf(error, size, "out of memory");
            return -1;
        }
        return 0;
    }
    char *path = absolute_path(arg + 1);
    if (path == NULL) {
        if (fd >= 0) {
            close(fd);
        }
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
    size_t len = 0;
    char *text = read_all(fd, &len, &err);
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
    if (decode(&text, &len) < 0 || list_push(reading, path) < 0 || tokenize(text, len, &words) < 0) {
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
