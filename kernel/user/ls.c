/* ls [PATH...]: lists each directory, or describes each file, named (the
 * working directory without one). A line per entry, as the kernel's boot
 * listing and `camelot-fs ls` print them: the type (d directory, f file, c
 * device), the inode, the links, the size and the name. Exits 1 if a path,
 * or an entry of a directory, can't be opened (it says why), else 0. */
#include "user.h"

static char type_letter(int type) {
    return type == T_DIR ? 'd' : type == T_FILE ? 'f' : type == T_DEVICE ? 'c' : '?';
}

static void line(const struct stat *st, const char *name) {
    printf("%c %4u %2u %7ld %s\n", type_letter(st->type), st->ino, st->nlink, st->size, name);
}

/* Lists `path`. Returns 0, or 1 if it can't. */
static int ls(const char *path) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        fprintf(STDERR, "ls: %s: %s\n", path, strerror(-fd));
        return 1;
    }
    struct stat st;
    fstat(fd, &st);
    if (st.type != T_DIR) {
        line(&st, path);
        close(fd);
        return 0;
    }
    /* "path/name" for each entry, to look it up. */
    char entry[MAXPATH + 1];
    uint len = strlen(path);
    if (len + 1 + DIRSIZ > MAXPATH) {
        fprintf(STDERR, "ls: %s: %s\n", path, strerror(ENAMETOOLONG));
        close(fd);
        return 1;
    }
    memcpy(entry, path, len);
    entry[len] = '/';
    int status = 0;
    struct dirent de;
    while (read(fd, &de, sizeof de) == sizeof de) {
        if (de.inum == 0) {
            continue;
        }
        memcpy(entry + len + 1, de.name, DIRSIZ);
        entry[len + 1 + DIRSIZ] = 0;
        /* stat() opens the entry: a device node with no driver, or no open
         * file to spare, is an error to report, not an entry to leave out. */
        int r = stat(entry, &st);
        if (r == -ENOENT) {
            /* Gone since it was read. */
            continue;
        }
        if (r < 0) {
            fprintf(STDERR, "ls: %s: %s\n", entry, strerror(-r));
            status = 1;
            continue;
        }
        line(&st, entry + len + 1);
    }
    close(fd);
    return status;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        return ls(".");
    }
    int status = 0;
    for (int i = 1; i < argc; i++) {
        status |= ls(argv[i]);
    }
    return status;
}
