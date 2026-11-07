/* ar: `ar [-]OPERATION[MODIFIERS] ARCHIVE [FILE...]`, the part of llvm-ar's
 * command line that makes, lists and unpacks static libraries:
 *
 *   r   puts the FILEs in ARCHIVE, made if it is missing: a member of a
 *       FILE's name (its last path component) is replaced in its place,
 *       the others go at the end, in order (archive.c's archive_replace)
 *   t   lists the members' names, a line each (only those named, if FILEs
 *       are given)
 *   x   writes the members (only those named, if FILEs are given) to files
 *       of their names in the working directory
 *
 * and the modifiers c (no "creating" message when r makes the archive), s
 * (write the symbol table: the default), S (do not), D (deterministic:
 * times and ids 0, mode 644, the default and the only mode). An argument
 * @FILE is replaced by the arguments of FILE, as llvm-ar reads response
 * files (quotes and backslashes too: lib/response.c): the shell passes a
 * program 16 arguments at most, fewer than a library has members. Errors go to stderr as
 * `ar: error: ...`, and the status is 1. */
#include "ar.h"

static const char USAGE[] = "usage: ar [-]{r,t,x}[c][s][S][D] ARCHIVE [FILE...] [@FILE]";

void die(const char *msg) {
    fprintf(2, "ar: error: %s\n", msg);
    exit(1);
}

void *xmalloc(uint n) {
    void *p = malloc(n ? n : 1);
    if (p == NULL) {
        die("out of memory");
    }
    return p;
}

char *xstrndup(const char *s, uint n) {
    char *r = xmalloc(n + 1);
    memcpy(r, s, n);
    r[n] = 0;
    return r;
}

void *grow(void *p, uint *cap, uint need, uint size) {
    if (need <= *cap) {
        return p;
    }
    uint c = *cap ? *cap : 8;
    while (c < need) {
        c *= 2;
    }
    void *q = realloc(p, (size_t)c * size);
    if (q == NULL) {
        die("out of memory");
    }
    *cap = c;
    return q;
}

void buf_put(struct buf *b, const void *p, uint n) {
    b->data = grow(b->data, &b->cap, b->len + n, 1);
    memcpy(b->data + b->len, p, n);
    b->len += n;
}

void buf_byte(struct buf *b, uint v) {
    uchar c = (uchar)v;
    buf_put(b, &c, 1);
}

/* The whole file, or NULL with *err set (a negative error number). */
static uchar *read_file(const char *path, uint *len, int *err) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        *err = fd;
        return NULL;
    }
    struct stat st;
    int r = fstat(fd, &st);
    if (r < 0 || st.type != T_FILE) {
        close(fd);
        *err = r < 0 ? r : -EISDIR;
        return NULL;
    }
    uint size = (uint)st.size;
    uchar *data = xmalloc(size + 1);
    uint done = 0;
    while (done < size) {
        int n = (int)read(fd, data + done, size - done);
        if (n < 0) {
            close(fd);
            *err = n;
            return NULL;
        }
        if (n == 0) {
            break;
        }
        done += (uint)n;
    }
    close(fd);
    *len = done;
    return data;
}

static int write_file(const char *path, const uchar *data, uint len) {
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        return fd;
    }
    uint done = 0;
    while (done < len) {
        uint n = len - done > 4096 ? 4096 : len - done;
        int r = write(fd, data + done, n);
        if (r <= 0) {
            close(fd);
            return r < 0 ? r : -ENOSPC;
        }
        done += (uint)r;
    }
    return close(fd);
}

/* The arguments after argv[0], each @FILE replaced by the arguments of
 * FILE (the C library's expand_response_files, as llvm-ar reads them: for
 * more members than the shell takes). 0 after a message. */
static int expand(int argc, char **argv, char ***out, uint *n) {
    char error[512];
    int count = 0;
    char **all = expand_response_files(argc, argv, &count, error, sizeof error);
    if (all == NULL) {
        fprintf(2, "ar: error: %s\n", error);
        return 0;
    }
    *out = all + 1;
    *n = (uint)(count - 1);
    return 1;
}

/* A path's last component: the name its file gets in an archive. */
static char *base_name(char *path) {
    char *slash = strrchr(path, '/');
    return slash != NULL ? slash + 1 : path;
}

/* Whether member `name` is one of the n FILEs (all of them when n is 0),
 * marking it found. As llvm-ar, each FILE stands for one member, the first
 * of its name not yet taken: `ar t lib.a p.o` with two p.o lists one, and
 * `ar x lib.a p.o` writes the first. */
static int wanted(const char *name, char **files, uint n, uchar *found) {
    if (n == 0) {
        return 1;
    }
    for (uint i = 0; i < n; i++) {
        if (!found[i] && strcmp(base_name(files[i]), name) == 0) {
            found[i] = 1;
            return 1;
        }
    }
    return 0;
}

int main(int argc, char **argv) {
    char **args;
    uint nargs;
    if (!expand(argc, argv, &args, &nargs)) {
        return 1;
    }
    if (nargs < 2) {
        fprintf(2, "%s\n", USAGE);
        return 1;
    }
    char *ops = args[0][0] == '-' ? args[0] + 1 : args[0];
    int op = 0, create_quietly = 0, symtab = 1;
    for (char *p = ops; *p; p++) {
        if (*p == 'r' || *p == 't' || *p == 'x') {
            if (op != 0) {
                die("only one operation may be given");
            }
            op = *p;
        } else if (*p == 'c') {
            create_quietly = 1;
        } else if (*p == 's') {
            symtab = 1;
        } else if (*p == 'S') {
            symtab = 0;
        } else if (*p != 'D') {
            fprintf(2, "ar: error: unknown modifier '%c'\n%s\n", *p, USAGE);
            return 1;
        }
    }
    if (op == 0) {
        fprintf(2, "ar: error: no operation given\n%s\n", USAGE);
        return 1;
    }
    char *path = args[1];
    char **files = args + 2;
    uint nfiles = nargs - 2;

    /* The archive as it is, if there is one. */
    uint len = 0;
    int err = 0;
    uchar *data = read_file(path, &len, &err);
    struct member *members = NULL;
    uint n = 0;
    if (data == NULL && (op != 'r' || err != -ENOENT)) {
        fprintf(2, "ar: error: '%s': %s\n", path, strerror(-err));
        return 1;
    }
    if (data != NULL) {
        char *msg;
        if (!archive_read(data, len, &members, &n, &msg)) {
            fprintf(2, "ar: error: '%s': %s\n", path, msg);
            return 1;
        }
    }

    if (op == 'r') {
        if (data == NULL && !create_quietly) {
            fprintf(2, "ar: creating %s\n", path);
        }
        struct member *given = xmalloc((nfiles + 1) * sizeof(struct member));
        for (uint i = 0; i < nfiles; i++) {
            given[i].name = base_name(files[i]);
            given[i].data = read_file(files[i], &given[i].len, &err);
            if (given[i].data == NULL) {
                fprintf(2, "ar: error: '%s': %s\n", files[i], strerror(-err));
                return 1;
            }
        }
        uint count;
        struct member *all = archive_replace(members, n, given, nfiles, &count);
        struct buf out;
        memset(&out, 0, sizeof out);
        char *msg;
        if (!archive_write(all, count, symtab, &out, &msg)) {
            fprintf(2, "ar: error: %s\n", msg);
            return 1;
        }
        int r = write_file(path, out.data, out.len);
        if (r < 0) {
            fprintf(2, "ar: error: '%s': %s\n", path, strerror(-r));
            return 1;
        }
        return 0;
    }

    uchar *found = xmalloc(nfiles + 1);
    memset(found, 0, nfiles + 1);
    for (uint i = 0; i < n; i++) {
        if (!wanted(members[i].name, files, nfiles, found)) {
            continue;
        }
        if (op == 't') {
            printf("%s\n", members[i].name);
        } else {
            int r = write_file(members[i].name, members[i].data, members[i].len);
            if (r < 0) {
                fprintf(2, "ar: error: '%s': %s\n", members[i].name, strerror(-r));
                return 1;
            }
        }
    }
    int status = 0;
    for (uint i = 0; i < nfiles; i++) {
        if (!found[i]) {
            fprintf(2, "ar: error: '%s' was not found\n", files[i]);
            status = 1;
        }
    }
    return status;
}
