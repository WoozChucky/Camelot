/* env [-i] [-u NAME]... [NAME=VALUE]... [PROG [ARG...]]: runs PROG with
 * the environment changed, as POSIX's env (and coreutils') does, or
 * without PROG prints the environment, an entry a line.
 *
 * It starts from its own environment (environ): -i (or `-` alone, POSIX's
 * older spelling) empties it, -u NAME takes NAME out, and each NAME=VALUE
 * sets NAME (setenv), in the order given. Then PROG replaces env in its
 * process (exec, as coreutils' env execvp()s it), with the ARGs and that
 * environment; PROG is found as the shell finds a program: /bin/PROG, or
 * the path itself if it has a '/'. So `env -i A=1 env` prints `A=1`.
 *
 * Exits 0 after printing, 125 if env itself fails (a bad option, a NAME
 * that can't be set), 127 if there is no PROG and 126 if it is there but
 * can't be run, as coreutils' env does; PROG's status is its own. */
#include "user.h"

/* An environment with nothing in it, for -i. */
static char *empty[] = {NULL};

static int usage(void) {
    fprintf(STDERR, "usage: env [-i] [-u NAME]... [NAME=VALUE]... [PROG [ARG...]]\n");
    return 125;
}

int main(int argc, char **argv) {
    int i = 1;
    for (; i < argc && argv[i][0] == '-'; i++) {
        if (strcmp(argv[i], "-i") == 0 || strcmp(argv[i], "-") == 0) {
            environ = empty;
        } else if (strcmp(argv[i], "-u") == 0 && i + 1 < argc) {
            i++;
            int r = unsetenv(argv[i]);
            if (r < 0) {
                fprintf(STDERR, "env: cannot unset %s: %s\n", argv[i], strerror(-r));
                return 125;
            }
        } else if (strcmp(argv[i], "--") == 0) {
            i++;
            break;
        } else {
            return usage();
        }
    }
    for (; i < argc && strchr(argv[i], '=') != NULL; i++) {
        /* NAME=VALUE: the name ends at the first '=' (a NAME that is empty
         * is refused by setenv). */
        char *eq = strchr(argv[i], '=');
        *eq = 0;
        int r = setenv(argv[i], eq + 1, 1);
        *eq = '=';
        if (r < 0) {
            fprintf(STDERR, "env: cannot set %s: %s\n", argv[i], strerror(-r));
            return 125;
        }
    }
    if (i == argc) {
        for (char **e = environ; *e != NULL; e++) {
            int r = printf("%s\n", *e);
            if (r < 0) {
                fprintf(STDERR, "env: %s\n", strerror(-r));
                return 125;
            }
        }
        return 0;
    }
    const char *prog = argv[i];
    char path[MAXPATH + 2];
    const char *run = prog;
    if (strchr(prog, '/') == NULL) {
        /* /bin/PROG, cut to one byte over MAXPATH if it is longer: a path
         * still too long for exec, which says so. */
        snprintf(path, sizeof path, "/bin/%s", prog);
        run = path;
    }
    int err = exec(run, &argv[i], environ);
    /* Still here: PROG couldn't be started. A PROG looked up in /bin that
     * isn't there is missing whatever the lookup tripped on, as in the
     * shell. */
    fprintf(STDERR, "env: %s: %s\n", prog, strerror(-err));
    int missing = err == -ENOENT || (run == path && (err == -ENOTDIR || err == -ENAMETOOLONG));
    return missing ? 127 : 126;
}
