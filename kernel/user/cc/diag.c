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
void diag_name_files(struct diag *d, const char **files, uint nfiles) {
    if (d->pos.file != 0 && d->file == NULL && d->pos.file < nfiles) {
        d->file = files[d->pos.file];
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
