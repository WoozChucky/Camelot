/* -w and the -W options, as clang applies them to its warnings (merlin's
 * merlin::warnings, which the two follow): what cc1 makes of its warnings
 * and the cc driver of its own. A -W option names a warning flag or a
 * group of them, clang's groups, which the callers' generated tables give
 * (crates/merlin/src/warning_groups.txt, from clang's `diagtool tree`):
 * a line "FLAG GROUP..." for each flag, the groups that hold it;
 * `everything` holds every warning, an unflagged one too. The options apply
 * in command-line order, as clang's ProcessWarningOptions applies them. */
#include "../user.h"

/* Whether `word`, `n` bytes, is one of the blank-separated words of
 * `list`. */
static int has_word(const char *list, const char *word, size_t n) {
    const char *p = list;
    while (*p != 0) {
        while (*p == ' ') {
            p++;
        }
        const char *w = p;
        while (*p != 0 && *p != ' ') {
            p++;
        }
        if ((size_t)(p - w) == n && n > 0 && strncmp(w, word, n) == 0) {
            return 1;
        }
    }
    return 0;
}

/* Whether option name `name` (what follows -W, -Wno-, -Werror=...) names
 * `flag` (NULL: unflagged) or a group that holds it. */
static int names(const char *name, const char *flag, const char *const *groups, int ngroups) {
    if (strcmp(name, "everything") == 0) {
        return 1;
    }
    if (flag == NULL) {
        return 0;
    }
    if (strcmp(name, flag) == 0) {
        return 1;
    }
    size_t flen = strlen(flag);
    for (int i = 0; i < ngroups; i++) {
        /* "FLAG GROUP...": this flag's line. */
        if (strncmp(groups[i], flag, flen) == 0 && groups[i][flen] == ' ') {
            return has_word(groups[i] + flen, name, strlen(name));
        }
    }
    return 0;
}

int warning_level(char *const *options, int noptions, const char *flag, const char *const *groups, int ngroups) {
    int on = 1;
    int error_of = -1; /* -W[no-]error=X's word, -1 for none */
    int all_errors = 0;
    for (int i = 0; i < noptions; i++) {
        const char *o = options[i];
        if (strcmp(o, "-w") == 0) {
            return WARNING_IGNORED;
        }
        if (strncmp(o, "-W", 2) != 0) {
            continue;
        }
        const char *w = o + 2;
        if (strcmp(w, "error") == 0) {
            all_errors = 1;
        } else if (strcmp(w, "no-error") == 0) {
            all_errors = 0;
        } else if (strncmp(w, "error=", 6) == 0) {
            if (names(w + 6, flag, groups, ngroups)) {
                on = 1;
                error_of = 1;
            }
        } else if (strncmp(w, "no-error=", 9) == 0) {
            if (names(w + 9, flag, groups, ngroups)) {
                error_of = 0;
            }
        } else if (strncmp(w, "no-", 3) == 0) {
            if (names(w + 3, flag, groups, ngroups)) {
                on = 0;
            }
        } else if (names(w, flag, groups, ngroups)) {
            on = 1;
        }
    }
    if (!on) {
        return WARNING_IGNORED;
    }
    return (error_of >= 0 ? error_of : all_errors) ? WARNING_ERROR : WARNING_SHOWN;
}

int is_warning_option(const char *a) {
    return strcmp(a, "-w") == 0 || (strncmp(a, "-W", 2) == 0 && a[2] != 0 && strchr(a, ',') == NULL);
}
