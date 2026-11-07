/* id: prints who the process runs as (docs/excalibur-spec.md, "Users").
 *
 *   id
 *
 * `uid=1000(user) gid=1000(user)`, coreutils' format without the
 * supplementary groups (Camelot has none): each id with its name from
 * /etc/passwd or /etc/group, or alone without one, and the effective ids
 * after them (`euid=0(root)`, `egid=...`) when they are not the real ones,
 * in a set-user-ID program. */
#include "user.h"

/* " KEY=ID(NAME)", or without the name if there is none. */
static void show(const char *key, uint id, int user) {
    const char *name = NULL;
    if (user) {
        struct passwd *pw = getpwuid(id);
        name = pw != NULL ? pw->pw_name : NULL;
    } else {
        struct group *gr = getgrgid(id);
        name = gr != NULL ? gr->gr_name : NULL;
    }
    if (name != NULL) {
        printf("%s=%u(%s)", key, id, name);
    } else {
        printf("%s=%u", key, id);
    }
}

int main(int argc, char **argv) {
    (void)argv;
    if (argc != 1) {
        fprintf(STDERR, "usage: id\n");
        return 2;
    }
    show("uid", getuid(), 1);
    printf(" ");
    show("gid", getgid(), 0);
    if (geteuid() != getuid()) {
        printf(" ");
        show("euid", geteuid(), 1);
    }
    if (getegid() != getgid()) {
        printf(" ");
        show("egid", getegid(), 0);
    }
    printf("\n");
    return 0;
}
