/* cp SRC DST, cp SRC... DIR: copies file SRC to DST (made with SRC's
 * permission bits but the set-ID ones, as coreutils' cp makes it, or
 * emptied first), or each SRC into the directory DIR under its own last
 * name.
 * Directories are not copied. Exits 1 if a file can't be copied (it says
 * why and goes on with the next), else 0. */
#include "user.h"

static char buf[512];

/* `path`'s last name: what follows its last '/'. */
static const char *last_name(const char *path) {
    const char *slash = strrchr(path, '/');
    return slash != NULL ? slash + 1 : path;
}

/* Whether `path` names a directory. */
static int is_dir(const char *path) {
    struct stat st;
    return stat(path, &st) == 0 && st.type == T_DIR;
}

/* Copies `src` to `dst`. Returns 0, or 1 having said why it can't. */
static int copy(const char *src, const char *dst) {
    int in = open(src, O_RDONLY);
    if (in < 0) {
        fprintf(STDERR, "cp: %s: %s\n", src, strerror(-in));
        return 1;
    }
    struct stat from;
    fstat(in, &from);
    if (from.type == T_DIR) {
        fprintf(STDERR, "cp: %s: %s\n", src, strerror(EISDIR));
        close(in);
        return 1;
    }
    /* Opening the file itself with O_TRUNC would empty it before it is
     * read. */
    struct stat to;
    if (stat(dst, &to) == 0 && to.ino == from.ino && from.ino != 0) {
        fprintf(STDERR, "cp: %s and %s are the same file\n", src, dst);
        close(in);
        return 1;
    }
    int out = open(dst, O_WRONLY | O_CREAT | O_TRUNC, from.mode & 0777);
    if (out < 0) {
        fprintf(STDERR, "cp: %s: %s\n", dst, strerror(-out));
        close(in);
        return 1;
    }
    int status = 0;
    for (;;) {
        int n = read(in, buf, sizeof buf);
        if (n == 0) {
            break;
        }
        if (n < 0) {
            fprintf(STDERR, "cp: %s: %s\n", src, strerror(-n));
            status = 1;
            break;
        }
        int w = write(out, buf, (uint)n);
        if (w != n) {
            /* A short write: the disk filled up. */
            fprintf(STDERR, "cp: %s: %s\n", dst, strerror(w < 0 ? -w : ENOSPC));
            status = 1;
            break;
        }
    }
    close(in);
    close(out);
    return status;
}

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(STDERR, "usage: cp SRC DST, or cp SRC... DIR\n");
        return 1;
    }
    const char *dst = argv[argc - 1];
    if (!is_dir(dst)) {
        if (argc > 3) {
            fprintf(STDERR, "cp: %s: %s\n", dst, strerror(ENOTDIR));
            return 1;
        }
        return copy(argv[1], dst);
    }
    int status = 0;
    for (int i = 1; i < argc - 1; i++) {
        char path[MAXPATH + 1];
        if (snprintf(path, sizeof path, "%s/%s", dst, last_name(argv[i])) > MAXPATH) {
            fprintf(STDERR, "cp: %s/%s: %s\n", dst, last_name(argv[i]), strerror(ENAMETOOLONG));
            status = 1;
            continue;
        }
        status |= copy(argv[i], path);
    }
    return status;
}
