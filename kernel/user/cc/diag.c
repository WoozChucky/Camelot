/* cc1: diagnostics, as clang gives them and prints them (follows merlin's
 * Error, Severity and name_files, in lib.rs and preprocess.rs). A message
 * and a file's name are bytes with a length, as Rust's strings are: one
 * that quotes the source may hold a NUL. */
#include "cc.h"

static struct diag *diag_new(struct pos pos, const char *msg, uint len, int severity, const char *flag) {
    struct diag *d = arena_alloc(sizeof *d);
    d->pos = pos;
    d->msg = msg;
    d->msglen = len;
    d->severity = severity;
    d->flag = flag;
    return d;
}

struct diag *diag_error(struct pos pos, const char *msg) {
    return diag_new(pos, msg, (uint)strlen(msg), SEV_ERROR, NULL);
}

struct diag *diag_error_n(struct pos pos, const char *msg, uint len) {
    return diag_new(pos, msg, len, SEV_ERROR, NULL);
}

struct diag *diag_warning(struct pos pos, const char *msg, const char *flag) {
    return diag_new(pos, msg, (uint)strlen(msg), SEV_WARNING, flag);
}

struct diag *diag_warning_n(struct pos pos, const char *msg, uint len, const char *flag) {
    return diag_new(pos, msg, len, SEV_WARNING, flag);
}

/* A warning clang gives with no flag (`character constant too long for its
 * type`). */
struct diag *diag_warning_unflagged(struct pos pos, const char *msg) {
    return diag_new(pos, msg, (uint)strlen(msg), SEV_WARNING, NULL);
}

struct diag *diag_fatal(struct pos pos, const char *msg) {
    return diag_new(pos, msg, (uint)strlen(msg), SEV_FATAL, NULL);
}

struct diag *diag_fatal_n(struct pos pos, const char *msg, uint len) {
    return diag_new(pos, msg, len, SEV_FATAL, NULL);
}

void diag_append(struct diag **list, struct diag *d) {
    while (*list != NULL) {
        list = &(*list)->next;
    }
    *list = d;
}

struct diag *diag_note(struct diag *d, struct pos pos, const char *msg) {
    diag_append(&d->notes, diag_new(pos, msg, (uint)strlen(msg), SEV_NOTE, NULL));
    return d;
}

int diag_is_warning(struct diag *d) {
    return d->severity == SEV_WARNING;
}

/* The number `n` bytes at s spell, as Rust's usize::from_str reads one (an
 * optional `+`, then decimal digits, nothing else, no overflow); 0 if they
 * spell none. */
static int parse_index(const char *s, uint n, uint64_t *out) {
    uint i = 0;
    if (n > 0 && s[0] == '+') {
        i = 1;
    }
    if (i == n) {
        return 0;
    }
    uint64_t v = 0;
    for (; i < n; i++) {
        uint d = (uchar)s[i] - '0';
        if (d > 9 || v > (0xffffffffffffffffull - d) / 10) {
            return 0;
        }
        v = v * 10 + d;
    }
    *out = v;
    return 1;
}

/* The message with the file of each place in it named (merlin's
 * name_places): it is split at its FILE_MARKs, and each pair of pieces
 * after the first, a file's number and what follows it, becomes the
 * file's name (none for a number no file has), `:` and that; a piece left
 * without its pair is dropped. */
static void name_places(struct diag *d, const char **names, const uint *lens, uint nfiles) {
    /* The pieces: from `start`, up to the next mark or the end. */
    struct buf b;
    memset(&b, 0, sizeof b);
    uint start = 0;
    uint end = 0;
    while (end < d->msglen && d->msg[end] != FILE_MARK) {
        end++;
    }
    buf_put(&b, d->msg, end);
    for (;;) {
        /* A file's number, then the rest, if both are there. */
        if (end >= d->msglen) {
            break;
        }
        start = end + 1;
        uint file_end = start;
        while (file_end < d->msglen && d->msg[file_end] != FILE_MARK) {
            file_end++;
        }
        if (file_end >= d->msglen) {
            break;
        }
        uint rest = file_end + 1;
        end = rest;
        while (end < d->msglen && d->msg[end] != FILE_MARK) {
            end++;
        }
        uint64_t n;
        if (parse_index(d->msg + start, file_end - start, &n) && n < nfiles) {
            buf_put(&b, names[n], lens[n]);
        }
        buf_byte(&b, ':');
        buf_put(&b, d->msg + rest, end - rest);
    }
    d->msglen = b.len;
    d->msg = buf_arena(&b);
    buf_free(&b);
}

/* A diagnostic in a header names the header (merlin's name_files): the
 * position's file, where it is not 0, the file being compiled, and the
 * diagnostic names none yet; and a message with a FILE_MARK has its
 * places named. Its notes too. */
void diag_name_files(struct diag *d, const char **names, const uint *lens, uint nfiles) {
    if (d->pos.file != 0 && d->file == NULL && d->pos.file < nfiles) {
        d->file = names[d->pos.file];
        d->filelen = lens[d->pos.file];
    }
    for (uint i = 0; i < d->msglen; i++) {
        if (d->msg[i] == FILE_MARK) {
            name_places(d, names, lens, nfiles);
            break;
        }
    }
    for (struct diag *n = d->notes; n != NULL; n = n->next) {
        diag_name_files(n, names, lens, nfiles);
    }
}

/* The diagnostic's own line (merlin's Error::first_line), as of
 * `severity`: `file:line:col: error: message`, then ` [-Wflag]` if it has
 * a flag; with no place, the file alone, or nothing at all for a
 * diagnostic of no file. A warning -Werror made an error (`promoted`) says
 * `[-Werror,-Wflag]` (`[-Werror]` with no flag); one made fatal its flag
 * alone, as clang marks them. */
static void print_line(struct diag *d, const char *input, int severity, int promoted) {
    const char *file = input;
    uint filelen = (uint)strlen(input);
    if (d->file != NULL) {
        file = d->file;
        filelen = d->filelen;
    }
    const char *what = "error";
    if (severity == SEV_FATAL) {
        what = "fatal error";
    } else if (severity == SEV_WARNING) {
        what = "warning";
    } else if (severity == SEV_NOTE) {
        what = "note";
    }
    struct buf b;
    memset(&b, 0, sizeof b);
    if (d->pos.line > 0) {
        buf_fmt(&b, "%b:%u:%u: %s: %b", file, filelen, d->pos.line, d->pos.col, what, d->msg, d->msglen);
    } else if (filelen == 0) {
        buf_fmt(&b, "%s: %b", what, d->msg, d->msglen);
    } else {
        buf_fmt(&b, "%b: %s: %b", file, filelen, what, d->msg, d->msglen);
    }
    if (d->flag != NULL) {
        buf_fmt(&b, promoted ? " [-Werror,-W%s]" : " [-W%s]", d->flag);
    } else if (promoted) {
        /* An unflagged warning made an error, as clang marks it. */
        buf_str(&b, " [-Werror]");
    }
    buf_byte(&b, '\n');
    write(2, b.data, b.len);
    buf_free(&b);
}

/* -w and the -W options, in command-line order (merlin's), which
 * warning_level (lib/warnings.c) applies to each warning with clang's
 * warning groups (tables.c's WARNING_GROUPS). */
static char **w_options;
static int nw_options;
int diag_failed;
int diag_fatal_seen;

void diag_take_w(char *opt) {
    w_options = realloc(w_options, (size_t)(nw_options + 1) * sizeof(char *));
    if (w_options == NULL) {
        die("out of memory");
    }
    w_options[nw_options++] = opt;
}

/* merlin's warnings::option_warnings, printed as merlin's main prints
 * them: the warning of each -W option that names no group of clang's
 * (warning_option_message, tables.c's WARNING_OPTIONS), in order, a
 * diagnostic of no file under -w and -W as any warning. Whether one was
 * made an error, which fails every input. */
int diag_option_warnings(void) {
    struct pos none;
    memset(&none, 0, sizeof none);
    int failed = 0;
    for (int i = 0; i < nw_options; i++) {
        char buf[256];
        const char *msg = warning_option_message(w_options[i], WARNING_OPTIONS, NWARNING_OPTIONS, buf, sizeof buf);
        if (msg == NULL) {
            continue;
        }
        struct diag *d = diag_warning(none, fmt("%s", msg), "unknown-warning-option");
        d->file = "";
        d->filelen = 0;
        diag_failed = 0;
        diag_print(d, "");
        failed |= diag_failed;
    }
    diag_failed = 0;
    return failed || diag_fatal_seen;
}

/* merlin's Error::render as it is, -w and -W aside: the command line's own
 * diagnostics (-O's, -std='s), which clang gives before it applies them. */
void diag_print_as_is(struct diag *d, const char *input) {
    print_line(d, input, d->severity, 0);
    for (struct diag *n = d->notes; n != NULL; n = n->next) {
        print_line(n, input, n->severity, 0);
    }
}

/* merlin's Error::render, then the newline eprintln! ends it with: the
 * diagnostic's line, then a line for each note; under -w and -W
 * (merlin's warnings::apply), a warning is left out, notes and all, or
 * made an error or a fatal one, and an error made fatal under
 * -Wfatal-errors (an error clang makes of a warning by default stays as it
 * is otherwise). After a fatal error (its notes said) nothing more is
 * said, as in clang: diag_fatal_seen. */
void diag_print(struct diag *d, const char *input) {
    if (diag_fatal_seen) {
        return;
    }
    int severity = d->severity;
    int promoted = 0;
    if (severity == SEV_WARNING) {
        int level = warning_level(w_options, nw_options, d->flag, WARNING_GROUPS, NWARNING_GROUPS);
        if (level == WARNING_IGNORED) {
            return;
        }
        if (level == WARNING_ERROR) {
            severity = SEV_ERROR;
            promoted = 1;
        } else if (level == WARNING_FATAL) {
            severity = SEV_FATAL;
        }
        diag_failed |= level == WARNING_ERROR || level == WARNING_FATAL;
    } else if (severity == SEV_ERROR &&
               warning_error_is_fatal(w_options, nw_options, d->flag, WARNING_GROUPS, NWARNING_GROUPS)) {
        severity = SEV_FATAL;
    }
    print_line(d, input, severity, promoted);
    for (struct diag *n = d->notes; n != NULL; n = n->next) {
        print_line(n, input, n->severity, 0);
    }
    diag_fatal_seen = severity == SEV_FATAL;
}
