/* mv SRC DST, mv SRC... DIR: renames file SRC to DST, or moves each SRC
 * into the directory DIR under its own last name. There is no rename
 * system call: mv links the new name (`link`), then removes the old one
 * (`unlink`), removing a file already called DST first. So it is not one
 * transaction: a crash in between can leave both names, or (when DST was
 * replaced) neither the old DST nor the new one. Directories can't be
 * moved, since `link` refuses them. When DST is another name of SRC's
 * file (a hard link), only SRC is removed; SRC and DST naming the same
 * entry is an error. Exits 1 if a file can't be moved (it says why and
 * goes on with the next), else 0. */
#include "user.h"

/* `path`'s last name: what follows its last '/'. */
static const char *last_name(const char *path) {
    const char *slash = strrchr(path, '/');
    return slash != NULL ? slash + 1 : path;
}

/* stat()s the directory that holds `path` (at most MAXPATH bytes): the
 * path up to its last '/', "/" if that is the first character, "." if
 * there is none. */
static int dir_stat(const char *path, struct stat *st) {
    char dir[MAXPATH + 1];
    size_t len = (size_t)(last_name(path) - path);
    if (len == 0) {
        return stat(".", st);
    }
    if (len > 1) {
        len--; /* without the '/' */
    }
    memcpy(dir, path, len);
    dir[len] = 0;
    return stat(dir, st);
}

/* Whether `a` and `b`, two names of one file, are the same directory entry:
 * the same last name in the same directory, however the paths spell it
 * (`d/x` and `d/./x`). */
static int same_entry(const char *a, const char *b) {
    if (strcmp(last_name(a), last_name(b)) != 0) {
        return 0;
    }
    struct stat da;
    struct stat db;
    if (dir_stat(a, &da) != 0 || dir_stat(b, &db) != 0) {
        return 1; /* can't tell: the safe answer, as nothing is removed */
    }
    return da.ino == db.ino;
}

/* Moves `src` to `dst`. Returns 0, or 1 having said why it can't. */
static int move(const char *src, const char *dst) {
    struct stat from;
    int r = stat(src, &from);
    if (r < 0) {
        fprintf(STDERR, "mv: %s: %s\n", src, strerror(-r));
        return 1;
    }
    if (from.type == T_DIR) {
        fprintf(STDERR, "mv: %s: %s\n", src, strerror(EISDIR));
        return 1;
    }
    struct stat to;
    if (stat(dst, &to) == 0) {
        if (to.ino == from.ino) {
            if (same_entry(src, dst)) {
                fprintf(STDERR, "mv: %s and %s are the same file\n", src, dst);
                return 1;
            }
            /* Two names of one file (hard links): dst already is what src
             * would become, so only src goes, as POSIX allows. */
            r = unlink(src);
            if (r < 0) {
                fprintf(STDERR, "mv: %s: %s\n", src, strerror(-r));
                return 1;
            }
            return 0;
        }
        if (to.type == T_DIR) {
            fprintf(STDERR, "mv: %s: %s\n", dst, strerror(EISDIR));
            return 1;
        }
        r = unlink(dst);
        if (r < 0) {
            fprintf(STDERR, "mv: %s: %s\n", dst, strerror(-r));
            return 1;
        }
    }
    r = link(src, dst);
    if (r < 0) {
        fprintf(STDERR, "mv: %s: %s\n", dst, strerror(-r));
        return 1;
    }
    r = unlink(src);
    if (r < 0) {
        fprintf(STDERR, "mv: %s: %s\n", src, strerror(-r));
        return 1;
    }
    return 0;
}

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(STDERR, "usage: mv SRC DST, or mv SRC... DIR\n");
        return 1;
    }
    const char *dst = argv[argc - 1];
    struct stat st;
    if (stat(dst, &st) != 0 || st.type != T_DIR) {
        if (argc > 3) {
            fprintf(STDERR, "mv: %s: %s\n", dst, strerror(ENOTDIR));
            return 1;
        }
        return move(argv[1], dst);
    }
    int status = 0;
    for (int i = 1; i < argc - 1; i++) {
        char path[MAXPATH + 1];
        if (snprintf(path, sizeof path, "%s/%s", dst, last_name(argv[i])) > MAXPATH) {
            fprintf(STDERR, "mv: %s/%s: %s\n", dst, last_name(argv[i]), strerror(ENAMETOOLONG));
            status = 1;
            continue;
        }
        status |= move(argv[i], path);
    }
    return status;
}
