/* ls [-l] [PATH...]: lists each directory, or describes each file, named
 * (the working directory without one). A line per entry, as the kernel's
 * boot listing and `camelot-fs ls` print them: the type (d directory, f
 * file, c device, s socket), the inode, the links, the size and the name.
 *
 * With -l, the line is coreutils' long one: the type and the mode
 * (`drwxr-xr-x`: - file, d directory, c device, s socket, then read,
 * write and run for the owner, the group and everyone else, `s`/`S` for
 * the set-user-ID and set-group-ID bits and `t`/`T` for the sticky bit,
 * the small letter when the x under it is set too), the links, the owner's
 * and the group's names (/etc/passwd's and /etc/group's, the number when
 * there is none), the size and the name.
 *
 * Exits 1 if a path, or an entry of a directory, can't be looked at (it
 * says why), else 0. */
#include "user.h"

static int long_form;

static char type_letter(int type) {
    return type == T_DIR ? 'd' : type == T_FILE ? 'f' : type == T_DEVICE ? 'c' : type == T_SOCKET ? 's' : '?';
}

/* `st`'s type and mode as `ls -l` shows them, in `out` (11 bytes). */
static void mode_string(const struct stat *st, char *out) {
    out[0] = st->type == T_DIR ? 'd' : st->type == T_DEVICE ? 'c' : st->type == T_SOCKET ? 's' : '-';
    const char *rwx = "rwxrwxrwx";
    for (int i = 0; i < 9; i++) {
        out[1 + i] = (st->mode & (0400u >> i)) != 0 ? rwx[i] : '-';
    }
    /* The special bits over the x's: the owner's, the group's, everyone's. */
    uint special[3] = {S_ISUID, S_ISGID, S_ISVTX};
    char letter[3] = {'s', 's', 't'};
    for (int i = 0; i < 3; i++) {
        if ((st->mode & special[i]) != 0) {
            char *x = &out[3 + 3 * i];
            *x = *x == 'x' ? letter[i] : (char)(letter[i] - 'a' + 'A');
        }
    }
    out[10] = 0;
}

/* The user's or the group's name of `id` in `buf`: its name, or the
 * number if it has none. */
static const char *user_name(uint id, char *buf) {
    struct passwd *pw = getpwuid(id);
    if (pw != NULL) {
        return pw->pw_name;
    }
    snprintf(buf, 12, "%u", id);
    return buf;
}

static const char *group_name(uint id, char *buf) {
    struct group *gr = getgrgid(id);
    if (gr != NULL) {
        return gr->gr_name;
    }
    snprintf(buf, 12, "%u", id);
    return buf;
}

static void line(const struct stat *st, const char *name) {
    if (!long_form) {
        printf("%c %4u %2u %7ld %s\n", type_letter(st->type), st->ino, st->nlink, st->size, name);
        return;
    }
    char mode[11];
    char ubuf[12];
    char gbuf[12];
    mode_string(st, mode);
    /* getpwuid's record is good until the next call: print it first. */
    printf("%s %2u %-8s ", mode, st->nlink, user_name(st->uid, ubuf));
    printf("%-8s %7ld %s\n", group_name(st->gid, gbuf), st->size, name);
}

/* Lists `path`. Returns 0, or 1 if it can't. */
static int ls(const char *path) {
    /* stat() first: a socket's name can't be opened, only looked at. */
    struct stat st;
    int r = stat(path, &st);
    if (r < 0) {
        fprintf(STDERR, "ls: %s: %s\n", path, strerror(-r));
        return 1;
    }
    if (st.type != T_DIR) {
        line(&st, path);
        return 0;
    }
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        fprintf(STDERR, "ls: %s: %s\n", path, strerror(-fd));
        return 1;
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
        r = stat(entry, &st);
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
    int first = 1;
    if (argc > 1 && strcmp(argv[1], "-l") == 0) {
        long_form = 1;
        first = 2;
    }
    if (argc <= first) {
        return ls(".");
    }
    int status = 0;
    for (int i = first; i < argc; i++) {
        status |= ls(argv[i]);
    }
    return status;
}
