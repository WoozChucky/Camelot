/* -w and the -W options, as clang applies them to its warnings (merlin's
 * merlin::warnings, which the two follow): what cc1 makes of its warnings
 * and the cc driver of its own. A -W option names a warning flag or a
 * group of them, clang's groups, which the callers' generated tables give
 * (crates/merlin/src/warning_groups.txt, from clang's `diagtool tree`):
 * a line "FLAG GROUP..." for each flag, the groups that hold it.
 * -Wno-everything turns off every warning, an unflagged one too, and
 * -Weverything turns on those off by default (none of the toolchain's);
 * `everything` is no group for -Werror= and -Wno-error=, which take it for
 * an unknown one, as clang's do. The options apply in command-line order,
 * as clang's ProcessWarningOptions applies them, to each warning's state.
 * An option that names no group of clang's is its warning `unknown warning
 * option '-Wfoo' [-Wunknown-warning-option]` (warning_option_message,
 * merlin's option_warnings), a warning like any other. */
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
 * `flag` (NULL: unflagged, which no name names) or a group that holds
 * it. */
static int names(const char *name, const char *flag, const char *const *groups, int ngroups) {
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

/* As clang's DiagnosticsEngine decides it (merlin::warnings::level): the
 * warning's severity, shown at first, and its "no -Werror" mark. -WX shows
 * it unless it is an error; -Wno-X leaves it out; -Werror=X makes it an
 * error; -Wno-error=X makes an error shown again and marks it;
 * -Wno-everything leaves out every warning, -Weverything changes none of
 * these. At the end, under the last of -Werror and -Wno-error, a shown
 * warning not marked is an error; -w anywhere leaves every warning out. */
int warning_level(char *const *options, int noptions, const char *flag, const char *const *groups, int ngroups) {
    int severity = WARNING_SHOWN;
    int no_werror = 0;
    int werror = 0;
    for (int i = 0; i < noptions; i++) {
        const char *o = options[i];
        if (strcmp(o, "-w") == 0) {
            return WARNING_IGNORED;
        }
        if (strncmp(o, "-W", 2) != 0) {
            continue;
        }
        const char *w = o + 2;
        /* clang's other spellings: -Wformat=0 is -Wno-format, and
         * -Werror-implicit-function-declaration GCC's -Werror=... */
        if (strcmp(w, "format=0") == 0) {
            w = "no-format";
        } else if (strcmp(w, "error-implicit-function-declaration") == 0) {
            w = "error=implicit-function-declaration";
        } else if (strcmp(w, "no-error-implicit-function-declaration") == 0) {
            w = "no-error=implicit-function-declaration";
        }
        /* -Werror= with no name is -Werror, as in clang. */
        if (strcmp(w, "error") == 0 || strcmp(w, "error=") == 0) {
            werror = 1;
        } else if (strcmp(w, "no-error") == 0 || strcmp(w, "no-error=") == 0) {
            werror = 0;
        } else if (strcmp(w, "everything") == 0) {
            /* Turns on what is off by default: nothing here. */
        } else if (strcmp(w, "no-everything") == 0) {
            severity = WARNING_IGNORED;
        } else if (strncmp(w, "error=", 6) == 0) {
            if (names(w + 6, flag, groups, ngroups)) {
                severity = WARNING_ERROR;
            }
        } else if (strncmp(w, "no-error=", 9) == 0) {
            if (names(w + 9, flag, groups, ngroups)) {
                if (severity == WARNING_ERROR) {
                    severity = WARNING_SHOWN;
                }
                no_werror = 1;
            }
        } else if (strncmp(w, "no-", 3) == 0) {
            if (names(w + 3, flag, groups, ngroups)) {
                severity = WARNING_IGNORED;
            }
        } else if (names(w, flag, groups, ngroups) && severity != WARNING_ERROR) {
            severity = WARNING_SHOWN;
        }
    }
    if (severity == WARNING_SHOWN && werror && !no_werror) {
        return WARNING_ERROR;
    }
    return severity;
}

int is_warning_option(const char *a) {
    return strcmp(a, "-w") == 0 || (strncmp(a, "-W", 2) == 0 && strchr(a, ',') == NULL);
}

/* Whether `name` is one of clang's warning groups, names[0..n); the empty
 * name, -W alone, is their root. */
static int known(const char *name, const char *const *names, int n) {
    if (name[0] == 0) {
        return 1;
    }
    for (int i = 0; i < n; i++) {
        if (strcmp(names[i], name) == 0) {
            return 1;
        }
    }
    return 0;
}

const char *warning_option_message(const char *o, const char *const *names, int nnames, char *buf, size_t size) {
    if (strncmp(o, "-W", 2) != 0) {
        return NULL; /* -w */
    }
    const char *w = strcmp(o + 2, "format=0") == 0 ? "no-format" : o + 2;
    int positive = strncmp(w, "no-", 3) != 0;
    const char *opt = positive ? w : w + 3;
    if (strcmp(opt, "system-headers") == 0 || strcmp(opt, "everything") == 0) {
        return NULL;
    }
    if (strncmp(opt, "error", 5) == 0) {
        const char *rest = opt + 5;
        if (rest[0] == 0) {
            return NULL;
        }
        if (rest[0] != '=' && strcmp(rest, "-implicit-function-declaration") != 0) {
            snprintf(buf, size, "unknown -Werror warning specifier: '%s'", o);
            return buf;
        }
        if (rest[1] != 0 && !known(rest + 1, names, nnames)) {
            snprintf(buf, size, "unknown warning option '-Werror=%s'", rest + 1);
            return buf;
        }
        return NULL;
    }
    if (strncmp(opt, "fatal-errors", 12) == 0) {
        const char *rest = opt + 12;
        if (rest[0] == 0) {
            return NULL;
        }
        if ((rest[0] != '=' && rest[0] != '-') || rest[1] == 0) {
            snprintf(buf, size, "unknown -Wfatal-errors warning specifier: '%s'", o);
            return buf;
        }
        if (!known(rest + 1, names, nnames)) {
            snprintf(buf, size, "unknown warning option '-Wfatal-errors=%s'", rest + 1);
            return buf;
        }
        return NULL;
    }
    if (!known(opt, names, nnames)) {
        snprintf(buf, size, "unknown warning option '%s%s'", positive ? "-W" : "-Wno-", opt);
        return buf;
    }
    return NULL;
}
