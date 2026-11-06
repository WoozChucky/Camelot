/* cc1: diagnostics, as clang gives them and prints them (follows merlin's
 * Error, Severity and name_files, in lib.rs and preprocess.rs). */
#include "cc.h"

static struct diag *diag_new(struct pos pos, const char *msg, int severity, const char *flag) {
    struct diag *d = arena_alloc(sizeof *d);
    d->pos = pos;
    d->msg = (char *)msg;
    d->severity = severity;
    d->flag = flag;
    return d;
}

struct diag *diag_error(struct pos pos, const char *msg) {
    return diag_new(pos, msg, SEV_ERROR, NULL);
}

struct diag *diag_warning(struct pos pos, const char *msg, const char *flag) {
    return diag_new(pos, msg, SEV_WARNING, flag);
}

/* A warning clang gives with no flag (`character constant too long for its
 * type`). */
struct diag *diag_warning_unflagged(struct pos pos, const char *msg) {
    return diag_new(pos, msg, SEV_WARNING, NULL);
}

struct diag *diag_fatal(struct pos pos, const char *msg) {
    return diag_new(pos, msg, SEV_FATAL, NULL);
}

void diag_append(struct diag **list, struct diag *d) {
    while (*list != NULL) {
        list = &(*list)->next;
    }
    *list = d;
}

struct diag *diag_note(struct diag *d, struct pos pos, const char *msg) {
    diag_append(&d->notes, diag_new(pos, msg, SEV_NOTE, NULL));
    return d;
}

int diag_is_warning(struct diag *d) {
    return d->severity == SEV_WARNING;
}

/* A diagnostic in a header names the header (merlin's name_files): the
 * position's file, where it is not 0, the file being compiled, and the
 * diagnostic names none yet. Its notes too. */
/* msg with the file of each place in it (types.c's place, `\1N\1LINE:COL`)
 * named: the number between two FILE_MARKs becomes the file's name, then a
 * `:` (merlin's name_places; a number of no file names none). */
static char *name_places(const char *msg, const char **files, uint nfiles) {
    struct buf b;
    memset(&b, 0, sizeof b);
    const char *p = msg;
    const char *mark = strchr(p, FILE_MARK);
    buf_put(&b, p, (uint)(mark - p));
    p = mark + 1;
    for (;;) {
        /* The file's number, then the rest up to the next mark. */
        const char *end = strchr(p, FILE_MARK);
        if (end == NULL) {
            break;
        }
        uint n = 0;
        int number = end > p;
        for (const char *q = p; q < end; q++) {
            if (*q < '0' || *q > '9') {
                number = 0;
                break;
            }
            n = n * 10 + (uint)(*q - '0');
        }
        if (number && n < nfiles) {
            buf_str(&b, files[n]);
        }
        buf_byte(&b, ':');
        p = end + 1;
        const char *next = strchr(p, FILE_MARK);
        uint len = next != NULL ? (uint)(next - p) : (uint)strlen(p);
        buf_put(&b, p, len);
        if (next == NULL) {
            break;
        }
        p = next + 1;
    }
    char *r = buf_arena(&b);
    buf_free(&b);
    return r;
}

void diag_name_files(struct diag *d, const char **files, uint nfiles) {
    if (d->pos.file != 0 && d->file == NULL && d->pos.file < nfiles) {
        d->file = files[d->pos.file];
    }
    if (strchr(d->msg, FILE_MARK) != NULL) {
        d->msg = name_places(d->msg, files, nfiles);
    }
    for (struct diag *n = d->notes; n != NULL; n = n->next) {
        diag_name_files(n, files, nfiles);
    }
}

/* The diagnostic's own line (merlin's Error::first_line): `file:line:col:
 * error: message`, then ` [-Wflag]` if it has a flag; with no place, the
 * file alone, or nothing at all for a diagnostic of no file. */
static void print_line(struct diag *d, const char *input) {
    const char *file = d->file != NULL ? d->file : input;
    const char *what = "error";
    if (d->severity == SEV_FATAL) {
        what = "fatal error";
    } else if (d->severity == SEV_WARNING) {
        what = "warning";
    } else if (d->severity == SEV_NOTE) {
        what = "note";
    }
    struct buf b;
    memset(&b, 0, sizeof b);
    if (d->pos.line > 0) {
        buf_fmt(&b, "%s:%u:%u: %s: %s", file, d->pos.line, d->pos.col, what, d->msg);
    } else if (file[0] == 0) {
        buf_fmt(&b, "%s: %s", what, d->msg);
    } else {
        buf_fmt(&b, "%s: %s: %s", file, what, d->msg);
    }
    if (d->flag != NULL) {
        buf_fmt(&b, " [-W%s]", d->flag);
    }
    buf_byte(&b, '\n');
    write(2, b.data, b.len);
    buf_free(&b);
}

/* merlin's Error::render, then the newline eprintln! ends it with: the
 * diagnostic's line, then a line for each note. */
void diag_print(struct diag *d, const char *input) {
    print_line(d, input);
    for (struct diag *n = d->notes; n != NULL; n = n->next) {
        print_line(n, input);
    }
}
