/* chown: changes files' owner (docs/camelot-syscalls.md, "Users and
 * permissions").
 *
 *   chown USER[:GROUP] FILE...
 *
 * USER and GROUP are names (from /etc/passwd and /etc/group) or numbers;
 * without GROUP the group stays as it is, and `USER:`, an empty GROUP, is
 * USER's login group (its /etc/passwd line's). Only root may give a file away.
 * Says which files it could not change and exits 1 if there was one. */
#include "user.h"

/* `s` as a number, or -1 if it isn't one. */
static long number(const char *s) {
    long v = 0;
    if (*s == 0) {
        return -1;
    }
    for (; *s != 0; s++) {
        if (*s < '0' || *s > '9' || v > 0xfffffffeL / 10) {
            return -1;
        }
        v = v * 10 + (*s - '0');
    }
    return v < 0xffffffffL ? v : -1;
}

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(STDERR, "usage: chown USER[:GROUP] FILE...\n");
        return 2;
    }
    char owner[64];
    if (strlen(argv[1]) >= sizeof owner) {
        fprintf(STDERR, "chown: %s: no such user\n", argv[1]);
        return 2;
    }
    snprintf(owner, sizeof owner, "%s", argv[1]);
    char *group = strchr(owner, ':');
    if (group != NULL) {
        *group++ = 0;
    }
    long uid = number(owner);
    /* The login group, for `USER:`: the account's, or -1 for a number with
     * none. */
    long login_gid = -1;
    struct passwd *pw = uid < 0 ? getpwnam(owner) : getpwuid((uid_t)uid);
    if (uid < 0) {
        if (pw == NULL) {
            fprintf(STDERR, "chown: %s: no such user\n", owner);
            return 2;
        }
        uid = pw->pw_uid;
    }
    if (pw != NULL) {
        login_gid = pw->pw_gid;
    }
    /* -1: the group as it is. */
    long gid = -1;
    if (group != NULL && *group == 0) {
        if (login_gid < 0) {
            fprintf(STDERR, "chown: %s: no login group\n", owner);
            return 2;
        }
        gid = login_gid;
    } else if (group != NULL) {
        gid = number(group);
        if (gid < 0) {
            struct group *gr = getgrnam(group);
            if (gr == NULL) {
                fprintf(STDERR, "chown: %s: no such group\n", group);
                return 2;
            }
            gid = gr->gr_gid;
        }
    }
    int status = 0;
    for (int i = 2; i < argc; i++) {
        int err = chown(argv[i], (uid_t)uid, (gid_t)gid);
        if (err < 0) {
            fprintf(STDERR, "chown: %s: %s\n", argv[i], strerror(-err));
            status = 1;
        }
    }
    return status;
}
