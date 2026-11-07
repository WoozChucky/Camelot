/* The account files (docs/excalibur-spec.md, "Users"): getpwnam and
 * getpwuid over /etc/passwd, getgrnam and getgrgid over /etc/group, and
 * getspnam over /etc/shadow, as POSIX's and glibc's are, each line a record
 * of fields separated by `:`:
 *
 *   /etc/passwd  name:x:uid:gid:gecos:home:shell
 *   /etc/group   name:x:gid:members
 *   /etc/shadow  name:hash:...
 *
 * Every call reads its file again, whole (at most ACCOUNTS_MAX bytes), into
 * a buffer of its own, and returns a record pointing into it: the record is
 * good until the next call of the same family, as POSIX allows (so they are
 * not for several threads at once). NULL if the file can't be read (shadow
 * is root's alone: anyone else gets NULL), or has no such record. A line
 * with too few fields, or an id that isn't a number, is left out. */
#include "../user.h"

/* The most bytes of an account file read: the rest is left out. */
#define ACCOUNTS_MAX 4096

struct file_buf {
    char data[ACCOUNTS_MAX + 1];
    char *next; /* the next line to look at */
};

static struct file_buf pw_file;
static struct file_buf gr_file;
static struct file_buf sp_file;
static struct passwd pw;
static struct group gr;
static struct spwd shadow;

/* Reads `path` into `f`, its lines from the start next. 0, or the error. */
static int read_file(const char *path, struct file_buf *f) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        return fd;
    }
    int len = 0;
    for (;;) {
        int n = read(fd, f->data + len, ACCOUNTS_MAX - len);
        if (n <= 0) {
            break;
        }
        len += n;
    }
    close(fd);
    f->data[len] = 0;
    f->next = f->data;
    return 0;
}

/* Splits the next line of `f` at its colons, in place, into at most `max`
 * fields; returns how many it had (0 at the end of the file). A line with
 * more fields keeps the rest in its last. */
static int next_line(struct file_buf *f, char **fields, int max) {
    while (*f->next != 0) {
        char *line = f->next;
        char *end = strchr(line, '\n');
        if (end != NULL) {
            *end = 0;
            f->next = end + 1;
        } else {
            f->next = line + strlen(line);
        }
        if (*line == 0 || *line == '#') {
            continue;
        }
        int n = 0;
        char *p = line;
        fields[n++] = p;
        while (n < max && (p = strchr(p, ':')) != NULL) {
            *p++ = 0;
            fields[n++] = p;
        }
        return n;
    }
    return 0;
}

/* An id: a decimal number below 2^32 - 1 (which stands for no id). -1 if
 * `s` is not one. */
static long parse_id(const char *s) {
    if (*s == 0) {
        return -1;
    }
    long v = 0;
    for (; *s != 0; s++) {
        if (*s < '0' || *s > '9') {
            return -1;
        }
        v = v * 10 + (*s - '0');
        if (v >= 0xffffffffL) {
            return -1;
        }
    }
    return v;
}

/* The next record of /etc/passwd into `pw`; 0 at the end. */
static int next_pw(void) {
    char *f[7];
    int n;
    while ((n = next_line(&pw_file, f, 7)) != 0) {
        long uid = n == 7 ? parse_id(f[2]) : -1;
        long gid = n == 7 ? parse_id(f[3]) : -1;
        if (uid < 0 || gid < 0) {
            continue;
        }
        pw.pw_name = f[0];
        pw.pw_passwd = f[1];
        pw.pw_uid = (uid_t)uid;
        pw.pw_gid = (gid_t)gid;
        pw.pw_gecos = f[4];
        pw.pw_dir = f[5];
        pw.pw_shell = f[6];
        return 1;
    }
    return 0;
}

struct passwd *getpwnam(const char *name) {
    if (read_file(PASSWD_FILE, &pw_file) < 0) {
        return NULL;
    }
    while (next_pw()) {
        if (strcmp(pw.pw_name, name) == 0) {
            return &pw;
        }
    }
    return NULL;
}

struct passwd *getpwuid(uid_t uid) {
    if (read_file(PASSWD_FILE, &pw_file) < 0) {
        return NULL;
    }
    while (next_pw()) {
        if (pw.pw_uid == uid) {
            return &pw;
        }
    }
    return NULL;
}

/* The next record of /etc/group into `gr`; 0 at the end. */
static int next_gr(void) {
    char *f[4];
    int n;
    while ((n = next_line(&gr_file, f, 4)) != 0) {
        long gid = n >= 3 ? parse_id(f[2]) : -1;
        if (gid < 0) {
            continue;
        }
        gr.gr_name = f[0];
        gr.gr_passwd = f[1];
        gr.gr_gid = (gid_t)gid;
        return 1;
    }
    return 0;
}

struct group *getgrnam(const char *name) {
    if (read_file(GROUP_FILE, &gr_file) < 0) {
        return NULL;
    }
    while (next_gr()) {
        if (strcmp(gr.gr_name, name) == 0) {
            return &gr;
        }
    }
    return NULL;
}

struct group *getgrgid(gid_t gid) {
    if (read_file(GROUP_FILE, &gr_file) < 0) {
        return NULL;
    }
    while (next_gr()) {
        if (gr.gr_gid == gid) {
            return &gr;
        }
    }
    return NULL;
}

struct spwd *getspnam(const char *name) {
    if (read_file(SHADOW_FILE, &sp_file) < 0) {
        return NULL;
    }
    char *f[3];
    int n;
    while ((n = next_line(&sp_file, f, 3)) != 0) {
        if (n >= 2 && strcmp(f[0], name) == 0) {
            shadow.sp_namp = f[0];
            shadow.sp_pwdp = f[1];
            return &shadow;
        }
    }
    return NULL;
}
