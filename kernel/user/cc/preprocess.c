/* cc1: the preprocessor, C99 6.10 (follows merlin's preprocess.rs, whose
 * module comment describes it in full).
 *
 * It works on the lexer's preprocessing tokens and hands out the tokens of
 * the whole translation unit one at a time (pp_next), every directive
 * carried out and every macro replaced, with clang's diagnostics, in its
 * words, at its positions: it goes on after most of them, as clang does
 * (a directive with an error is dropped, a macro call with the wrong
 * number of arguments leaves the macro's name as it is). Only a header
 * that is not found stops it (clang's fatal error), and an unterminated
 * comment in the file being compiled.
 *
 * Macro replacement follows Dave Prosser's algorithm, as C99's rules were
 * written down from it: every token carries a hide set, the names of the
 * macros whose expansion produced it, and a name is replaced only if it is
 * not in its own hide set; the expansion goes back in front of the input
 * to be rescanned with the rest of the source.
 *
 * Assembly (`.S`, -x assembler-with-cpp) goes through the same
 * preprocessor, with clang's differences for it: a `#` followed by no
 * directive's name starts no directive (it is an assembly comment), `#`
 * in a function-like macro may stand before anything, and pasting two
 * tokens that make no token is no error.
 *
 * -E writes the tokens out (emit): C as merlin's spell writes them, a line
 * break where the source had one, a space where it had whitespace or
 * where two tokens would otherwise read as others; assembly as clang's -E
 * writes it (print_asm), each token on the line it came from, with line
 * markers. Both are written as the tokens come, which is what merlin's
 * printers, reading the finished list in order, do too.
 *
 * To compile, the tokens are collected instead (pp->collect,
 * preprocess_unit): the translation unit's every token, where the
 * expansions of macros made some, and each diagnostic with where the last
 * token before it came out, for the front end to put them in clang's
 * order (merlin's Output). */
#include "cc.h"

/* How deeply #includes may nest (clang's limit too). */
#define MAX_INCLUDE_DEPTH 200

/* ---- Tokens with their history ---- */

/* The names of the macros whose expansion produced a token: NULL is the
 * empty set. A set is never changed once made, so tokens share them. */
struct hideset {
    uint n;
    const char **names;
};

/* A token and its hide set, and where it comes from, for print_asm
 * (merlin's PTok). */
struct ptok {
    struct pptok t;
    struct hideset *hide;
    /* For a token a macro's expansion produced: where the outermost macro
     * was used (clang's expansion location), the line and column it is
     * shown at. */
    int has_site;
    struct pos site;
    /* Where its characters are written, in a file or in a macro's body;
     * none for one the preprocessor made (`#x`, `a ## b`, __LINE__). Two
     * tokens written next to each other need no space between them. */
    int has_spelled;
    struct pos spelled_from;
    struct pos spelled_to;
    /* clang's leading space: whitespace (or a comment) before it on its own
     * line, which pptok's space also says for a newline. */
    int lead;
    /* It is a token of a function-like macro's body (where clang does not
     * warn of a `defined` in #if). */
    int function_body;
};

/* A growable list of tokens (on the heap). */
struct toks {
    struct ptok *v;
    uint n;
    uint cap;
};

static void toks_push(struct toks *l, struct ptok *t) {
    l->v = grow(l->v, &l->cap, l->n + 1, sizeof(struct ptok));
    l->v[l->n++] = *t;
}

static void toks_free(struct toks *l) {
    heap_free(l->v);
    memset(l, 0, sizeof *l);
}

/* Tokens waiting to be read before the files: a double-ended queue (a ring
 * of cap slots, len of them used from head). */
struct deque {
    struct ptok *buf;
    uint head;
    uint len;
    uint cap;
};

static void deque_make_room(struct deque *d) {
    if (d->len < d->cap) {
        return;
    }
    uint cap = d->cap ? d->cap * 2 : 16;
    struct ptok *buf = heap_alloc((size_t)cap * sizeof(struct ptok));
    for (uint i = 0; i < d->len; i++) {
        buf[i] = d->buf[(d->head + i) % d->cap];
    }
    heap_free(d->buf);
    d->buf = buf;
    d->head = 0;
    d->cap = cap;
}

static void push_front(struct deque *d, struct ptok *t) {
    deque_make_room(d);
    d->head = (d->head + d->cap - 1) % d->cap;
    d->buf[d->head] = *t;
    d->len++;
}

static void push_back(struct deque *d, struct ptok *t) {
    deque_make_room(d);
    d->buf[(d->head + d->len) % d->cap] = *t;
    d->len++;
}

static int pop_front(struct deque *d, struct ptok *t) {
    if (d->len == 0) {
        return 0;
    }
    *t = d->buf[d->head];
    d->head = (d->head + 1) % d->cap;
    d->len--;
    return 1;
}

static struct ptok *front(struct deque *d) {
    return d->len ? &d->buf[d->head] : NULL;
}

/* A queue holding the tokens of l, in order. */
static struct deque deque_of(struct toks *l) {
    struct deque d;
    memset(&d, 0, sizeof d);
    for (uint i = 0; i < l->n; i++) {
        push_back(&d, &l->v[i]);
    }
    return d;
}

static int hidden(struct hideset *hs, const char *name) {
    if (hs == NULL) {
        return 0;
    }
    for (uint i = 0; i < hs->n; i++) {
        if (strcmp(hs->names[i], name) == 0) {
            return 1;
        }
    }
    return 0;
}

/* a's names, then b's that a has not. Shared where it is one of them. */
static struct hideset *hide_union(struct hideset *a, struct hideset *b) {
    if (b == NULL || b->n == 0) {
        return a;
    }
    if (a == NULL || a->n == 0) {
        return b;
    }
    uint extra = 0;
    for (uint i = 0; i < b->n; i++) {
        if (!hidden(a, b->names[i])) {
            extra++;
        }
    }
    if (extra == 0) {
        return a;
    }
    struct hideset *u = arena_alloc(sizeof *u);
    u->names = arena_alloc((a->n + extra) * sizeof(char *));
    for (uint i = 0; i < a->n; i++) {
        u->names[u->n++] = a->names[i];
    }
    for (uint i = 0; i < b->n; i++) {
        if (!hidden(a, b->names[i])) {
            u->names[u->n++] = b->names[i];
        }
    }
    return u;
}

/* a's names that b has too. */
static struct hideset *hide_intersection(struct hideset *a, struct hideset *b) {
    if (a == NULL || b == NULL) {
        return NULL;
    }
    struct hideset *s = arena_alloc(sizeof *s);
    s->names = arena_alloc((a->n + 1) * sizeof(char *));
    for (uint i = 0; i < a->n; i++) {
        if (hidden(b, a->names[i])) {
            s->names[s->n++] = a->names[i];
        }
    }
    return s;
}

static struct hideset *hide_one(const char *name) {
    struct hideset *s = arena_alloc(sizeof *s);
    s->names = arena_alloc(sizeof(char *));
    s->names[0] = name;
    s->n = 1;
    return s;
}

/* A token as read from a file (merlin's PTok::read). */
static void read_tok(struct pptok *t, struct ptok *out) {
    memset(out, 0, sizeof *out);
    out->t = *t;
    out->has_spelled = 1;
    out->spelled_from = t->pos;
    out->spelled_to = t->end;
    /* What comes before the first token of a line is on that line only if
     * it is not in column 1. */
    out->lead = t->space && (!t->bol || t->pos.col > 1);
}

/* Tokens of a directive's line, to be replaced on their own. */
static void read_all(struct pptok *t, uint n, struct toks *out) {
    memset(out, 0, sizeof *out);
    for (uint i = 0; i < n; i++) {
        struct ptok p;
        read_tok(&t[i], &p);
        toks_push(out, &p);
    }
}

/* Sets a token's text to the n bytes at s: interned for an identifier,
 * the table's for a punctuator. */
static void set_text(struct pptok *t, const char *s, uint n) {
    if (t->kind == PP_IDENT) {
        t->text = intern(s, n);
    } else if (t->kind == PP_PUNCT) {
        t->text = PUNCTS[t->punct];
    } else {
        t->text = arena_strndup(s, n);
    }
    t->len = n;
}

/* ---- Macros ---- */

enum {
    MACRO_OBJECT,
    /* params, and whether the last one takes the variadic arguments:
     * __VA_ARGS__ for `...` (C99), `args` for GNU's `args...`. */
    MACRO_FUNCTION,
    /* __FILE__: the current file's name, as a string literal. */
    MACRO_FILE,
    /* __LINE__: the current line, as a number. */
    MACRO_LINE
};

struct macro {
    int kind;
    const char **params;
    uint nparams;
    int variadic;
    struct pptok *body;
    uint nbody;
    /* Where it was defined. */
    struct pos pos;
};

static int same_kind(struct pptok *a, struct pptok *b) {
    if (a->kind != b->kind) {
        return 0;
    }
    if (a->kind == PP_PUNCT) {
        return a->punct == b->punct;
    }
    if (a->kind == PP_INVALID) {
        if (a->msg == NULL || b->msg == NULL) {
            return a->msg == b->msg;
        }
        return strcmp(a->msg, b->msg) == 0;
    }
    return 1;
}

/* Whether two definitions are the same (C99 6.10.3p2): the same parameters
 * and the same body, token for token, with whitespace between the same
 * tokens. */
static int macro_same(struct macro *a, struct macro *b) {
    if (a->kind != b->kind || a->nbody != b->nbody) {
        return 0;
    }
    if (a->kind == MACRO_FUNCTION) {
        if (a->nparams != b->nparams || a->variadic != b->variadic) {
            return 0;
        }
        for (uint i = 0; i < a->nparams; i++) {
            if (strcmp(a->params[i], b->params[i]) != 0) {
                return 0;
            }
        }
    }
    for (uint i = 0; i < a->nbody; i++) {
        struct pptok *x = &a->body[i];
        struct pptok *y = &b->body[i];
        if (!same_kind(x, y) || x->len != y->len || memcmp(x->text, y->text, x->len) != 0 ||
            (i > 0 && x->space != y->space)) {
            return 0;
        }
    }
    return 1;
}

/* The index of the parameter a body token names, or -1. */
static int macro_param(struct macro *m, struct pptok *t) {
    if (m->kind != MACRO_FUNCTION || t->kind != PP_IDENT) {
        return -1;
    }
    for (uint i = 0; i < m->nparams; i++) {
        if (strcmp(m->params[i], t->text) == 0) {
            return (int)i;
        }
    }
    return -1;
}

/* ---- Files and paths ---- */

struct file {
    /* For __FILE__ and errors: namelen bytes (a name #line gives may hold
     * a NUL). */
    const char *name;
    uint namelen;
    /* For #pragma once and for finding the files it includes (never a
     * NUL: a header's name with one is no file). */
    const char *path;
    /* Its contents' MD5, for -g (merlin's File::md5): none for a name
     * #line gives, or when options.checksums does not ask for it. */
    struct checksum sum;
};

/* A file being read. */
struct cursor {
    struct pptok *toks;
    uint n;
    uint i;
    /* Its text (textlen bytes), for the lines #error and #warning print as
     * written; owned (freed when the file is left) for a header. */
    const char *text;
    uint textlen;
    int owned;
    /* Its entry in files (#line "name" makes another). */
    uint file;
    /* Added to every line number (#line). */
    int64_t line_delta;
    /* How many conditionals were open when it was entered: the ones it
     * opens must be closed in it. */
    uint conds;
    /* For an #included file: the line the file that includes it goes on
     * at, after the directive. */
    int has_back;
    uint back;
    /* How many line markers say a file was entered (flag 1) and not left
     * yet (flag 2): what flag 2 may leave. */
    uint entered_by_markers;
};

/* An open #if (or #ifdef, #ifndef). */
struct cond {
    struct pos pos;
    /* One of its groups has been taken: the rest are skipped. */
    int taken;
    /* Its #else has been seen. */
    int seen_else;
};

/* A change of file in the output: a line marker (merlin's Mark). */
enum { MARK_ENTER, MARK_EXIT, MARK_RENAME };
struct pmark {
    int kind;
    uint line;
    uint file;
};

/* The state of print_asm (merlin's Printer), and of spell. */
struct printer {
    struct buf *out;
    /* The line of the file being written that the output is on, and that
     * file. */
    uint line;
    uint file;
    /* Tokens have been written on the current line. */
    int started;
    /* No token has been written since a file was entered: the next one, if
     * it is on the file's first line, is indented, even on the line of the
     * marker. */
    int entered;
    /* Line markers are written (no -P). */
    int markers;
    /* The last two tokens written, and how they were laid out. */
    uint count;
    struct ptok prev;
    struct pos prev_at;
    struct ptok prev_prev;
};

struct pp {
    struct options *opts;
    struct file *files;
    uint nfiles;
    uint capfiles;
    /* Name -> struct macro. */
    struct map macros;
    /* The paths of the files marked #pragma once. */
    struct map once;
    /* The files being read: the one compiled, and the headers it is
     * including, innermost last. */
    struct cursor *stack;
    uint nstack;
    uint capstack;
    /* Macro expansions waiting to be rescanned. */
    struct deque pending;
    /* Expanding a list of tokens on its own (a macro argument, an #if
     * line): nothing past pending is read. */
    int isolated;
    struct cond *conds;
    uint nconds;
    uint capconds;
    /* The file of the predefined macros, <built-in>. */
    uint builtin_file;
    /* Preprocessing assembly. */
    int asm;
    /* The include guard of each file read that has one: path -> name. */
    struct map guards;
    /* How many tokens have come out so far, and where the last one is. */
    uint emitted;
    struct pos last_out;
    /* The diagnostics so far, and where the last token out was when each
     * came (for the parser's order of them, merlin's Output::merge). */
    struct diag **diags;
    struct pos *after;
    uint ndiags;
    uint capdiags;
    uint capafter;
    /* Line markers waiting for the next token out. */
    struct pmark *marks;
    uint nmarks;
    uint capmarks;
    /* In a group #if skips. */
    int skipping;
    /* A macro that expanded to nothing started a line, or had a space before
     * it: the next token takes that over (its bol, space and lead). */
    int carry_bol;
    int carry_space;
    int carry_lead;
    struct printer print;
    /* Compiling, not -E: the tokens are collected (merlin's Output) rather
     * than written out. */
    int collect;
    struct pptok *out;
    uint nout;
    uint capout;
    /* The positions of the tokens a macro's expansion made. */
    struct posmap macro_sites;
    /* The end of the file being compiled. */
    struct pos end;
};

/* The components of a path, as Rust's Path::components gives them on
 * Unix: the root (`/`), a `.` that starts the path, `..`, and names; empty
 * names (`a//b`) and other `.`s are none. */
enum { COMP_ROOT, COMP_CUR, COMP_PARENT, COMP_NORMAL };
struct comp {
    int kind;
    uint start;
    uint end;
};

/* The components of p (len bytes: a file's name may hold a NUL), on the heap;
 * their count in *n. */
static struct comp *components(const char *p, uint len, uint *n) {
    struct comp *c = heap_alloc((len + 1) * sizeof(struct comp));
    uint k = 0;
    uint i = 0;
    if (len > 0 && p[0] == '/') {
        c[k].kind = COMP_ROOT;
        c[k].start = 0;
        c[k].end = 1;
        k++;
        i = 1;
    } else if (len > 0 && p[0] == '.' && (len == 1 || p[1] == '/')) {
        c[k].kind = COMP_CUR;
        c[k].start = 0;
        c[k].end = 1;
        k++;
        i = 1;
    }
    while (i < len) {
        while (i < len && p[i] == '/') {
            i++;
        }
        uint start = i;
        while (i < len && p[i] != '/') {
            i++;
        }
        uint n2 = i - start;
        if (n2 == 0 || (n2 == 1 && p[start] == '.')) {
            continue;
        }
        c[k].kind = n2 == 2 && p[start] == '.' && p[start + 1] == '.' ? COMP_PARENT : COMP_NORMAL;
        c[k].start = start;
        c[k].end = i;
        k++;
    }
    *n = k;
    return c;
}

/* A lexical form of a path for telling files apart: `.` dropped, `..`
 * applied where it can be, separators made uniform (merlin's normalize). */
static const char *normalize(const char *p) {
    uint n;
    struct comp *c = components(p, (uint)strlen(p), &n);
    /* The components kept, as indexes into c. */
    uint *kept = heap_alloc((n + 1) * sizeof(uint));
    uint k = 0;
    for (uint i = 0; i < n; i++) {
        if (c[i].kind == COMP_CUR) {
            continue;
        }
        if (c[i].kind == COMP_PARENT && k > 0 && c[kept[k - 1]].kind == COMP_NORMAL) {
            k--;
            continue;
        }
        kept[k++] = i;
    }
    struct buf b;
    memset(&b, 0, sizeof b);
    for (uint i = 0; i < k; i++) {
        struct comp *x = &c[kept[i]];
        if (x->kind == COMP_ROOT) {
            buf_byte(&b, '/');
            continue;
        }
        if (i > 0 && c[kept[i - 1]].kind != COMP_ROOT) {
            buf_byte(&b, '/');
        }
        buf_put(&b, p + x->start, x->end - x->start);
    }
    char *r = buf_arena(&b);
    buf_free(&b);
    heap_free(kept);
    heap_free(c);
    return r;
}

/* The parent of p (len bytes) as Rust's Path::parent gives it, the path as
 * written up to the end of its next-to-last component, its length in
 * *out; NULL for none (the root, or an empty path). */
static const char *parent(const char *p, uint len, uint *out) {
    uint n;
    struct comp *c = components(p, len, &n);
    const char *r = NULL;
    if (n > 0 && c[n - 1].kind != COMP_ROOT) {
        *out = n > 1 ? c[n - 2].end : 0;
        r = arena_strndup(p, *out);
    }
    heap_free(c);
    return r;
}

/* `dir` joined with `name` (Rust's Path::join): name if it is absolute or
 * dir is empty, else the two with a `/` between them unless dir ends with
 * one. */
static const char *join(const char *dir, const char *name) {
    uint n = (uint)strlen(dir);
    if (name[0] == '/' || n == 0) {
        return name;
    }
    return fmt("%s%s%s", dir, dir[n - 1] == '/' ? "" : "/", name);
}

/* The file at path, NULL-terminated, if it is one that reads as UTF-8 (a
 * Rust read_to_string that succeeds); *len its size. */
static char *load(const char *path, uint *len) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        return NULL;
    }
    struct stat st;
    if (fstat(fd, &st) < 0 || st.type == T_DIR) {
        close(fd);
        return NULL;
    }
    struct buf b;
    memset(&b, 0, sizeof b);
    char chunk[4096];
    for (;;) {
        int r = read(fd, chunk, sizeof chunk);
        if (r < 0) {
            close(fd);
            buf_free(&b);
            return NULL;
        }
        if (r == 0) {
            break;
        }
        buf_put(&b, chunk, (uint)r);
    }
    close(fd);
    buf_put(&b, "", 0);
    if (!valid_utf8((uchar *)b.data, b.len)) {
        buf_free(&b);
        return NULL;
    }
    *len = b.len;
    return b.data;
}

/* `\` and `"` escaped, to go between double quotes. */
static void escape_into(struct buf *b, const char *s, uint n) {
    for (uint i = 0; i < n; i++) {
        if (s[i] == '\\' || s[i] == '"') {
            buf_byte(b, '\\');
        }
        buf_byte(b, (uchar)s[i]);
    }
}

/* ---- Reporting ---- */

/* Records a diagnostic and goes on. */
static void report(struct pp *pp, struct diag *d) {
    pp->diags = grow(pp->diags, &pp->capdiags, pp->ndiags + 1, sizeof(struct diag *));
    pp->after = grow(pp->after, &pp->capafter, pp->ndiags + 1, sizeof(struct pos));
    pp->diags[pp->ndiags] = d;
    pp->after[pp->ndiags] = pp->last_out;
    pp->ndiags++;
}

/* Records the diagnostics of a list (linked by next) the lexer collected. */
static void report_all(struct pp *pp, struct diag *list) {
    while (list != NULL) {
        struct diag *next = list->next;
        list->next = NULL;
        report(pp, list);
        list = next;
    }
}

/* A new file, its number: text (len bytes) is what was read of it, whose
 * MD5 -g's line table gives (NULL for a name #line gives, which has
 * none). */
static uint new_file(struct pp *pp, const char *name, uint namelen, const char *path, const char *text, uint len) {
    pp->files = grow(pp->files, &pp->capfiles, pp->nfiles + 1, sizeof(struct file));
    struct file *f = &pp->files[pp->nfiles];
    f->name = name;
    f->namelen = namelen;
    f->path = path;
    memset(&f->sum, 0, sizeof f->sum);
    if (text != NULL && pp->opts->checksums) {
        f->sum.has = 1;
        md5((const uchar *)text, len, f->sum.sum);
    }
    return pp->nfiles++;
}

static void add_mark(struct pp *pp, int kind, uint line, uint file) {
    pp->marks = grow(pp->marks, &pp->capmarks, pp->nmarks + 1, sizeof(struct pmark));
    pp->marks[pp->nmarks].kind = kind;
    pp->marks[pp->nmarks].line = line;
    pp->marks[pp->nmarks].file = file;
    pp->nmarks++;
}

static void enter(struct pp *pp, struct lexed *lx, const char *text, uint textlen, int owned, uint file) {
    pp->stack = grow(pp->stack, &pp->capstack, pp->nstack + 1, sizeof(struct cursor));
    struct cursor *c = &pp->stack[pp->nstack++];
    memset(c, 0, sizeof *c);
    c->toks = lx->toks;
    c->n = lx->n;
    c->text = text;
    c->textlen = textlen;
    c->owned = owned;
    c->file = file;
    c->conds = pp->nconds;
}

static struct cursor *top(struct pp *pp) {
    return &pp->stack[pp->nstack - 1];
}

/* ---- Reading tokens ---- */

/* The next token of the innermost file, if it has one, without taking it. */
static struct pptok *peek_file(struct pp *pp) {
    if (pp->nstack == 0) {
        return NULL;
    }
    struct cursor *c = top(pp);
    return c->i < c->n ? &c->toks[c->i] : NULL;
}

static void renumber_pos(struct cursor *c, struct pos *p) {
    p->file = c->file;
    p->line = (uint)((int64_t)p->line + c->line_delta);
}

/* Takes the next token of the innermost file (there must be one), with its
 * position as #line has it, and gives the lexer's warnings of what came
 * before it, but in a group #if skips, which clang reads raw. */
static struct pptok take_file(struct pp *pp) {
    struct cursor *c = top(pp);
    struct pptok t = c->toks[c->i++];
    struct diag *lexed = t.lexed;
    t.lexed = NULL;
    renumber_pos(c, &t.pos);
    renumber_pos(c, &t.end);
    for (struct diag *w = lexed; w != NULL; w = w->next) {
        renumber_pos(c, &w->pos);
    }
    if (!pp->skipping) {
        report_all(pp, lexed);
    }
    return t;
}

static int is_directive_name(const char *name) {
    for (uint i = 0; i < NDIRECTIVES; i++) {
        if (strcmp(DIRECTIVES[i], name) == 0) {
            return 1;
        }
    }
    return 0;
}

/* Whether the innermost file's next token starts a directive: a `#` that
 * starts a line, and in assembly only one followed by a directive's name or
 * by nothing (`# a comment` is assembly). */
static int at_directive(struct pp *pp) {
    if (pp->nstack == 0) {
        return 0;
    }
    struct cursor *c = top(pp);
    if (c->i >= c->n || !c->toks[c->i].bol || !tok_is_punct(&c->toks[c->i], "#")) {
        return 0;
    }
    if (!pp->asm || c->i + 1 >= c->n) {
        return 1;
    }
    struct pptok *next = &c->toks[c->i + 1];
    return next->bol || (next->kind == PP_IDENT && is_directive_name(next->text));
}

/* The tokens of the innermost file up to the end of the line (an array on
 * the heap, *n of them). */
static struct pptok *rest_of_line(struct pp *pp, uint *n) {
    struct pptok *line = NULL;
    uint cap = 0;
    *n = 0;
    while (peek_file(pp) != NULL && !peek_file(pp)->bol) {
        line = grow(line, &cap, *n + 1, sizeof(struct pptok));
        line[(*n)++] = take_file(pp);
    }
    return line;
}

/* The rest of the line after the token `name` (a directive's name in the
 * innermost file), as written: what clang's #error and #warning print.
 * Spaces after the name are left out; comments stay, and a backslash at the
 * end of a line joins the next one to it. */
/* The end of the line of the cursor's text that starts at offset `at`: the
 * offset of its newline, or of the text's end. */
static uint line_end(struct cursor *c, uint at) {
    while (at < c->textlen && c->text[at] != '\n') {
        at++;
    }
    return at;
}

/* The rest of the line after the token `name` (a directive's name in the
 * innermost file), as written: what clang's #error and #warning print, its
 * length in *len (it may hold a NUL). Spaces after the name are left out;
 * comments stay, and a backslash at the end of a line joins the next one
 * to it. */
static char *raw_rest(struct pp *pp, struct pptok *name, uint *len) {
    struct cursor *c = top(pp);
    /* The line as the lexer numbered it, before #line, and where it starts. */
    uint line = (uint)((int64_t)name->end.line - c->line_delta);
    uint at = 0;
    int found = 1;
    for (uint k = 1; k < line && found; k++) {
        at = line_end(c, at);
        if (at < c->textlen) {
            at++;
        } else {
            found = 0;
        }
    }
    struct buf b;
    memset(&b, 0, sizeof b);
    if (found) {
        uint eol = line_end(c, at);
        /* Columns count bytes. */
        uint start = name->end.col - 1 < eol - at ? at + name->end.col - 1 : eol;
        for (;;) {
            uint n = eol - start;
            if (n > 0 && c->text[start + n - 1] == '\r') {
                n--;
            }
            /* A backslash at the end joins the next line, if there is one. */
            if (n > 0 && c->text[start + n - 1] == '\\' && eol < c->textlen) {
                buf_put(&b, c->text + start, n - 1);
                start = eol + 1;
                eol = line_end(c, start);
                continue;
            }
            buf_put(&b, c->text + start, n);
            break;
        }
    }
    uint skip = 0;
    while (skip < b.len && b.data[skip] == ' ') {
        skip++;
    }
    char *text = arena_strndup(b.len ? b.data + skip : "", b.len - skip);
    *len = b.len - skip;
    buf_free(&b);
    return text;
}

/* Leaves the innermost file. clang reports every conditional it left open,
 * the innermost first, and closes them. */
static void leave(struct pp *pp) {
    struct cursor c = *top(pp);
    pp->nstack--;
    while (pp->nconds > c.conds) {
        pp->nconds--;
        report(pp, diag_error(pp->conds[pp->nconds].pos, "unterminated conditional directive"));
    }
    if (c.has_back && pp->nstack > 0) {
        add_mark(pp, MARK_EXIT, c.back, top(pp)->file);
    }
    /* Its tokens are copied wherever they went: the file is done with. */
    heap_free(c.toks);
    if (c.owned) {
        heap_free((char *)c.text);
    }
}

static struct diag *directive(struct pp *pp);
static struct diag *pp_next(struct pp *pp, struct ptok *out, int *got);

/* Inside the arguments of a call of the macro `name`, a directive that
 * would bring in other text (#include, #pragma...) is an error, and
 * dropped, as clang has it (C leaves any directive there undefined; clang
 * carries out the others). Whether the next line was one. */
static int embedded_include(struct pp *pp, struct pptok *name) {
    struct cursor *c = top(pp);
    const char *d = "";
    if (c->i + 1 < c->n && !c->toks[c->i + 1].bol && c->toks[c->i + 1].kind == PP_IDENT) {
        d = c->toks[c->i + 1].text;
    }
    if (strcmp(d, "include") != 0 && strcmp(d, "include_next") != 0 && strcmp(d, "import") != 0 &&
        strcmp(d, "__include_macros") != 0 && strcmp(d, "pragma") != 0 && strcmp(d, "embed") != 0) {
        return 0;
    }
    take_file(pp); /* # */
    uint n;
    struct pptok *line = rest_of_line(pp, &n);
    struct diag *e = diag_error(line[0].pos,
                                fmt("embedding a #%s directive within macro arguments is not supported", d));
    diag_note(e, name->pos, fmt("expansion of macro '%s' requested here", name->text));
    report(pp, e);
    heap_free(line);
    return 1;
}

/* The next token before macro replacement: from an expansion waiting to be
 * rescanned, or else from the files, carrying out directives on the way.
 * Inside the arguments of a call of the macro `call`, the arguments go on
 * past the end of a header, but not past the end of the file being
 * compiled. *got: whether there was one. An error is a fatal one. */
static struct diag *next_raw(struct pp *pp, struct pptok *call, struct ptok *out, int *got) {
    *got = 0;
    for (;;) {
        if (pop_front(&pp->pending, out)) {
            *got = 1;
            return NULL;
        }
        if (pp->isolated || pp->nstack == 0) {
            return NULL;
        }
        if (peek_file(pp) == NULL) {
            if (call != NULL && pp->nstack == 1) {
                return NULL;
            }
            leave(pp);
            continue;
        }
        if (at_directive(pp)) {
            if (call != NULL && embedded_include(pp, call)) {
                continue;
            }
            /* An error drops the directive, and preprocessing goes on; only a
             * fatal one stops it. */
            struct diag *e = directive(pp);
            if (e != NULL) {
                if (e->severity == SEV_FATAL) {
                    return e;
                }
                report(pp, e);
            }
            continue;
        }
        struct pptok t = take_file(pp);
        read_tok(&t, out);
        /* A `#` that starts no directive (in assembly) is written with a
         * space before it, even in column 1, so that it reads as none again
         * (clang's output does this). */
        if (out->t.bol && tok_is_punct(&out->t, "#")) {
            out->lead = 1;
        }
        *got = 1;
        return NULL;
    }
}

/* Whether the next token is `(`, without reading past the innermost file. */
static int lparen_follows(struct pp *pp) {
    struct ptok *f = front(&pp->pending);
    if (f != NULL) {
        return tok_is_punct(&f->t, "(");
    }
    if (pp->isolated) {
        return 0;
    }
    /* A `(` on a later line counts too (a `#` there would start a
     * directive, and is no `(`). */
    struct pptok *t = peek_file(pp);
    return t != NULL && tok_is_punct(t, "(");
}

/* A token made by the preprocessor where the token `at` was, its text len
 * bytes. */
static void made(struct ptok *at, int kind, const char *text, uint len, struct ptok *out) {
    memset(out, 0, sizeof *out);
    out->t.kind = (uchar)kind;
    out->t.text = text;
    out->t.len = len;
    out->t.pos = at->t.pos;
    out->t.end = at->t.end;
    out->t.bol = at->t.bol;
    out->t.space = at->t.space;
    out->hide = at->hide;
    out->has_site = 1;
    out->site = at->has_site ? at->site : at->t.pos;
    out->lead = at->lead;
}

/* Replaces a list of tokens on its own, as if it were the rest of the file:
 * a macro argument, or a directive's line. Nothing stops it: no directive
 * is read, and the diagnostics of the expansion are reported. */
static void expand_alone(struct pp *pp, struct toks *in, struct toks *out) {
    struct deque saved = pp->pending;
    pp->pending = deque_of(in);
    int was_isolated = pp->isolated;
    pp->isolated = 1;
    int cb = pp->carry_bol;
    int cs = pp->carry_space;
    int cl = pp->carry_lead;
    pp->carry_bol = pp->carry_space = pp->carry_lead = 0;
    memset(out, 0, sizeof *out);
    for (;;) {
        struct ptok t;
        int got;
        /* Only a directive can stop the preprocessor, and none is read. */
        pp_next(pp, &t, &got);
        if (!got) {
            break;
        }
        toks_push(out, &t);
    }
    heap_free(pp->pending.buf);
    pp->pending = saved;
    pp->isolated = was_isolated;
    pp->carry_bol = cb;
    pp->carry_space = cs;
    pp->carry_lead = cl;
}

/* What reading the arguments of a macro call gives (merlin's Call). */
enum {
    /* The arguments, and the `)` that ends them. */
    CALL_ARGS,
    /* Too many or too few: the macro's name stands for itself, as in clang,
     * and what was read is gone. */
    CALL_WRONG,
    /* The input ended before the `)`: the call is gone. */
    CALL_UNTERMINATED
};

static struct diag *defined_here(struct diag *e, struct macro *m, struct pptok *name) {
    return diag_note(e, m->pos, fmt("macro '%s' defined here", name->text));
}

/* After the name of the function-like macro m, reads `( arguments )` as
 * clang's ReadMacroCallArgumentList does: each argument's tokens (split at
 * commas outside parentheses; the commas of the variadic part stay in it)
 * and the `)`. A wrong number of arguments is an error at the first extra
 * one, or at the `)` for too few; the end of the input before the `)` is
 * one at the name. *args is m->nparams lists (on the heap) for CALL_ARGS. */
static struct diag *arguments(struct pp *pp, struct pptok *name, struct macro *m, int *call,
                              struct toks **args_out, struct ptok *rparen) {
    struct ptok open;
    int got;
    struct diag *e = next_raw(pp, name, &open, &got);
    if (e != NULL) {
        return e;
    }
    /* The lists: nargs of them, the last the argument being read. */
    struct toks *args = NULL;
    uint capargs = 0;
    args = grow(args, &capargs, 1, sizeof(struct toks));
    memset(&args[0], 0, sizeof(struct toks));
    uint nargs = 1;
    /* Where each argument starts: its first token, or the `(` or `,` before
     * it if it has none. */
    struct pos *starts = NULL;
    uint nstarts = 0;
    uint capstarts = 0;
    struct pos separator = open.t.pos;
    uint depth = 0;
    for (;;) {
        struct ptok t;
        e = next_raw(pp, name, &t, &got);
        if (e != NULL) {
            for (uint i = 0; i < nargs; i++) {
                toks_free(&args[i]);
            }
            heap_free(args);
            heap_free(starts);
            return e;
        }
        if (!got) {
            report(pp, defined_here(diag_error(name->pos, "unterminated function-like macro invocation"), m,
                                    name));
            for (uint i = 0; i < nargs; i++) {
                toks_free(&args[i]);
            }
            heap_free(args);
            heap_free(starts);
            *call = CALL_UNTERMINATED;
            return NULL;
        }
        uint arg = nargs - 1;
        if (tok_is_punct(&t.t, "(")) {
            depth++;
        } else if (tok_is_punct(&t.t, ")")) {
            if (depth == 0) {
                if (nstarts == arg) {
                    starts = grow(starts, &capstarts, nstarts + 1, sizeof(struct pos));
                    starts[nstarts++] = separator;
                }
                *rparen = t;
                break;
            }
            depth--;
        } else if (tok_is_punct(&t.t, ",") && depth == 0 && !(m->variadic && nargs == m->nparams)) {
            if (nstarts == arg) {
                starts = grow(starts, &capstarts, nstarts + 1, sizeof(struct pos));
                starts[nstarts++] = separator;
            }
            separator = t.t.pos;
            args = grow(args, &capargs, nargs + 1, sizeof(struct toks));
            memset(&args[nargs], 0, sizeof(struct toks));
            nargs++;
            continue;
        }
        if (nstarts == arg) {
            starts = grow(starts, &capstarts, nstarts + 1, sizeof(struct pos));
            starts[nstarts++] = t.t.pos;
        }
        toks_push(&args[arg], &t);
    }
    /* `F()` passes no argument; one empty argument where one is wanted. */
    uint nlists = nargs;
    if (nargs == 1 && args[0].n == 0) {
        nargs = 0;
    }
    uint wanted = m->nparams;
    struct diag *wrong = NULL;
    if (!m->variadic && nargs > wanted) {
        wrong = diag_error(starts[wanted], "too many arguments provided to function-like macro invocation");
    } else {
        /* Too few is fine for `F()` where one argument is wanted, and where
         * only the variadic one is missing (`F(x)` for `F(x, ...)`). */
        int fine = (nargs + 1 == wanted && (nargs == 0 || m->variadic)) ||
                   (m->variadic && nargs == 0 && wanted == 2);
        if (nargs < wanted && !fine) {
            wrong = diag_error(rparen->t.pos, "too few arguments provided to function-like macro invocation");
        }
    }
    heap_free(starts);
    if (wrong != NULL) {
        report(pp, defined_here(wrong, m, name));
        for (uint i = 0; i < nlists; i++) {
            toks_free(&args[i]);
        }
        heap_free(args);
        *call = CALL_WRONG;
        return NULL;
    }
    /* Exactly the parameters' number of lists, the missing ones empty. */
    struct toks *out = heap_calloc(wanted + 1, sizeof(struct toks));
    for (uint i = 0; i < nlists; i++) {
        if (i < nargs && i < wanted) {
            out[i] = args[i];
        } else {
            toks_free(&args[i]);
        }
    }
    heap_free(args);
    *args_out = out;
    *call = CALL_ARGS;
    return NULL;
}

/* `#arg`: the argument's spelling as a string literal, with the `\` and `"`
 * of string literals and character constants in it escaped. Other
 * backslashes stay as they are, so an argument that ends in one (`s(\)`)
 * would have no valid literal (C leaves it undefined): clang leaves that
 * backslash out, and warns. *lone: whether it did; *len, its length. */
static const char *stringize(struct toks *arg, int *lone, uint *len) {
    struct buf b;
    memset(&b, 0, sizeof b);
    buf_byte(&b, '"');
    for (uint k = 0; k < arg->n; k++) {
        struct pptok *t = &arg->v[k].t;
        if (k > 0 && t->space) {
            buf_byte(&b, ' ');
        }
        if (t->kind == PP_STR || t->kind == PP_CHAR) {
            escape_into(&b, t->text, t->len);
        } else {
            buf_put(&b, t->text, t->len);
        }
    }
    /* An odd number of backslashes at the end would escape the closing
     * quote. */
    uint trailing = 0;
    while (trailing < b.len && b.data[b.len - 1 - trailing] == '\\') {
        trailing++;
    }
    *lone = trailing % 2 == 1;
    if (*lone) {
        b.len--;
    }
    buf_byte(&b, '"');
    const char *s = buf_arena(&b);
    *len = b.len;
    buf_free(&b);
    return s;
}

/* `left ## right` in the expansion of a macro used at `at`: the two
 * spellings joined, which must read as one token. When they do not, C has
 * an error and clang goes on with the two tokens, the left one where the
 * macro was used, the right one where it was. In assembly that is no
 * error: `right` has a space before it if it is a token of the macro's
 * body written with one (`space`: `call f ## @plt` is `call foo @plt`),
 * else none (`. ## name` is `.name`), as clang leaves them. */
static void paste(struct pp *pp, struct ptok *left, struct ptok *right, int space, struct pos at,
                  struct toks *out) {
    if (left->t.kind == PP_PLACEMARKER) {
        toks_push(out, right);
        return;
    }
    if (right->t.kind == PP_PLACEMARKER) {
        toks_push(out, left);
        return;
    }
    struct buf text;
    memset(&text, 0, sizeof text);
    buf_put(&text, left->t.text, left->t.len);
    buf_put(&text, right->t.text, right->t.len);
    struct split s;
    lex_split(text.data, text.len, pp->asm, 2, &s);
    int one = s.ok && s.count == 1 && split_is(&s, 0, text.data, text.len) && s.kind[0] != PP_INVALID;
    if (!one) {
        lex_split_free(&s);
        if (pp->asm) {
            right->t.space = (uchar)space;
            right->lead = space;
        } else {
            uint mlen;
            const char *msg =
                fmtn(&mlen, "pasting formed '%b', an invalid preprocessing token", text.data, text.len);
            report(pp, diag_error_n(at, msg, mlen));
            left->t.pos = at;
        }
        buf_free(&text);
        toks_push(out, left);
        toks_push(out, right);
        return;
    }
    struct ptok p = *left;
    p.t.kind = s.kind[0];
    p.t.punct = s.punct[0];
    p.t.msg = NULL;
    set_text(&p.t, text.data, text.len);
    p.has_spelled = 0;
    lex_split_free(&s);
    buf_free(&text);
    toks_push(out, &p);
}

/* The expansion of macro m used at at_tok (ending at end), with these
 * arguments, every token's hide set extended by hide (merlin's
 * substitute).
 *
 * The spaces between the tokens are clang's (which -E output shows): a
 * body token keeps the one before it in the definition, except right after
 * `##`; what an argument gives starts with the space before the parameter
 * in the body, not with the one it had in the call (`[x]` with `( a )` is
 * `[a]`); and an empty argument hands its space on to what comes next. The
 * first token takes the spacing of the macro's name, and whether it
 * started a line. */
static void substitute(struct pp *pp, struct macro *m, struct toks *args, struct hideset *hide,
                       struct ptok *at_tok, struct pos end, struct toks *out) {
    struct pos site = at_tok->has_site ? at_tok->site : at_tok->t.pos;
    struct pptok *at = &at_tok->t;
    int function_body = m->kind == MACRO_FUNCTION;
    memset(out, 0, sizeof *out);
    /* A `##` was just seen: the next tokens' first is pasted to the last one
     * out. */
    int pasting = 0;
    /* The next token out gets a space before it (clang's NextTokGetsSpace). */
    int space = 0;
    struct pptok *body = m->body;
    uint i = 0;
    while (i < m->nbody) {
        struct pptok *t = &body[i];
        if (i > 0 && t->space && !tok_is_punct(&body[i - 1], "##")) {
            space = 1;
        }
        int next_is_paste = i + 1 < m->nbody && tok_is_punct(&body[i + 1], "##");
        /* After a `##` that forms no token (assembly), a token of the body
         * keeps the space it had before it; an argument's first token has
         * none. */
        int own_space = t->space && macro_param(m, t) < 0;
        /* In a function-like macro of C, `#` is always before a parameter
         * (define checks); in assembly it may be a token. */
        int stringized = -1;
        if (tok_is_punct(t, "#") && i + 1 < m->nbody) {
            stringized = macro_param(m, &body[i + 1]);
        }
        if (tok_is_punct(t, "##")) {
            pasting = 1;
            space = 0;
            i++;
            continue;
        }
        /* What this body token gives. */
        struct toks piece;
        memset(&piece, 0, sizeof piece);
        /* A body token, where the macro was used. */
        struct ptok here;
        memset(&here, 0, sizeof here);
        here.t = *t;
        here.t.pos = at->pos;
        here.t.end = end;
        here.t.bol = 0;
        here.t.lexed = NULL;
        here.has_site = 1;
        here.site = site;
        here.has_spelled = 1;
        here.spelled_from = t->pos;
        here.spelled_to = t->end;
        here.lead = t->space;
        here.function_body = function_body;
        int p = macro_param(m, t);
        if (stringized >= 0) {
            /* `#param`: the argument's spelling, as a string literal. */
            struct toks *arg = &args[stringized];
            i++;
            here.t.kind = PP_STR;
            here.has_spelled = 0;
            int lone;
            here.t.text = stringize(arg, &lone, &here.t.len);
            here.t.msg = NULL;
            if (lone) {
                report(pp, diag_warning_unflagged(arg->v[arg->n - 1].t.pos,
                                                  "invalid string literal, ignoring final '\\'"));
            }
            toks_push(&piece, &here);
        } else if (p >= 0) {
            struct toks *arg = &args[p];
            if (pasting || next_is_paste) {
                /* Next to `##`: as written, or a placemarker if empty. */
                if (arg->n == 0) {
                    struct ptok pm;
                    memset(&pm, 0, sizeof pm);
                    pm.t.kind = PP_PLACEMARKER;
                    pm.t.text = "";
                    pm.t.pos = at->pos;
                    pm.t.end = end;
                    pm.has_site = 1;
                    pm.site = site;
                    pm.function_body = function_body;
                    toks_push(&piece, &pm);
                } else {
                    for (uint k = 0; k < arg->n; k++) {
                        toks_push(&piece, &arg->v[k]);
                    }
                }
            } else {
                expand_alone(pp, arg, &piece);
            }
        } else {
            toks_push(&piece, &here);
        }
        i++;
        /* What comes out for this body token starts with the space it gets,
         * and (for an argument) no longer starts a line; the rest of an
         * argument keeps its own, as in clang, line starts too. */
        if (piece.n > 0 && piece.v[0].t.kind != PP_PLACEMARKER) {
            piece.v[0].t.space = (uchar)space;
            space = 0;
            piece.v[0].lead = piece.v[0].t.space;
            piece.v[0].t.bol = 0;
        }
        uint k = 0;
        if (pasting) {
            pasting = 0;
            /* `##` is never first in a body. */
            struct ptok left = out->v[--out->n];
            if (piece.n > 0) {
                paste(pp, &left, &piece.v[0], own_space, at->pos, out);
                k = 1;
            } else {
                /* An argument that expanded to nothing. */
                toks_push(out, &left);
            }
        }
        for (; k < piece.n; k++) {
            toks_push(out, &piece.v[k]);
        }
        toks_free(&piece);
    }
    /* Placemarkers have done their job; the first token takes the spacing of
     * the macro's name. */
    uint kept = 0;
    for (uint k = 0; k < out->n; k++) {
        if (out->v[k].t.kind != PP_PLACEMARKER) {
            out->v[kept++] = out->v[k];
        }
    }
    out->n = kept;
    for (uint k = 0; k < out->n; k++) {
        out->v[k].hide = hide_union(out->v[k].hide, hide);
        out->v[k].has_site = 1;
        out->v[k].site = site;
    }
    if (out->n > 0) {
        out->v[0].t.space = at->space;
        out->v[0].lead = at_tok->lead;
        out->v[0].t.bol = at_tok->t.bol;
    }
}

/* The next token after macro replacement (merlin's Pp::next); *got: whether
 * there was one. An error is a fatal one. */
static struct diag *pp_next(struct pp *pp, struct ptok *out, int *got) {
    for (;;) {
        struct ptok t;
        struct diag *e = next_raw(pp, NULL, &t, got);
        if (e != NULL || !*got) {
            return e;
        }
        if (t.t.kind == PP_DIRECTIVE) {
            /* A line of its own: what an empty expansion leaves is for the
             * token after it. */
            *out = t;
            return NULL;
        }
        t.t.bol |= (uchar)pp->carry_bol;
        t.t.space |= (uchar)pp->carry_space;
        t.lead |= pp->carry_lead;
        pp->carry_bol = pp->carry_space = pp->carry_lead = 0;
        struct macro *m = NULL;
        if (t.t.kind == PP_IDENT && !hidden(t.hide, t.t.text)) {
            m = map_get(&pp->macros, t.t.text);
        }
        if (m == NULL) {
            *out = t;
            return NULL;
        }
        const char *name = t.t.text;
        struct toks expansion;
        memset(&expansion, 0, sizeof expansion);
        if (m->kind == MACRO_FILE) {
            struct buf b;
            memset(&b, 0, sizeof b);
            struct file *f = &pp->files[t.t.pos.file];
            buf_byte(&b, '"');
            escape_into(&b, f->name, f->namelen);
            buf_byte(&b, '"');
            struct ptok s;
            made(&t, PP_STR, buf_arena(&b), b.len, &s);
            buf_free(&b);
            toks_push(&expansion, &s);
        } else if (m->kind == MACRO_LINE) {
            struct ptok s;
            const char *line = fmt("%u", t.t.pos.line);
            made(&t, PP_NUMBER, line, (uint)strlen(line), &s);
            toks_push(&expansion, &s);
        } else if (m->kind == MACRO_OBJECT) {
            struct hideset *hide = hide_union(t.hide, hide_one(name));
            substitute(pp, m, NULL, hide, &t, t.t.end, &expansion);
        } else {
            if (!lparen_follows(pp)) {
                *out = t;
                return NULL;
            }
            int call;
            struct toks *args = NULL;
            struct ptok rparen;
            e = arguments(pp, &t.t, m, &call, &args, &rparen);
            if (e != NULL) {
                return e;
            }
            if (call == CALL_WRONG) {
                *out = t;
                return NULL;
            }
            if (call == CALL_UNTERMINATED) {
                continue;
            }
            struct hideset *hide = hide_union(hide_intersection(t.hide, rparen.hide), hide_one(name));
            substitute(pp, m, args, hide, &t, rparen.t.end, &expansion);
            for (uint i = 0; i < m->nparams; i++) {
                toks_free(&args[i]);
            }
            heap_free(args);
        }
        if (expansion.n == 0) {
            pp->carry_bol = t.t.bol;
            pp->carry_space = t.t.space;
            pp->carry_lead = t.lead;
        }
        for (uint i = expansion.n; i > 0; i--) {
            push_front(&pp->pending, &expansion.v[i - 1]);
        }
        toks_free(&expansion);
    }
}

/* ---- #if expressions (merlin's Eval) ----
 * An #if expression (C99 6.10.1) evaluated as clang's PPExpressions.cpp
 * does, with its diagnostics: an integer constant expression over 64-bit
 * values, every operator but assignments, `++`, `--` and `sizeof`, read by
 * precedence climbing. The tokens come one at a time from the line, macros
 * replaced as they come, but the name after `defined`. `&&`, `||` and `?:`
 * evaluate only what they need (`live` 0 below them): division by zero
 * there is no error, an overflow no warning. */

/* A value: 64 bits, signed (intmax_t) or not (uintmax_t); `ident` the
 * identifier it is, if it is one alone (one followed by `(` is no macro:
 * `function-like macro 'f' is not defined`). */
struct val {
    uint64_t bits;
    int is_unsigned;
    const char *ident;
    struct pos ident_pos;
};

static struct val val_new(uint64_t bits, int is_unsigned) {
    struct val v;
    memset(&v, 0, sizeof v);
    v.bits = bits;
    v.is_unsigned = is_unsigned;
    return v;
}

static struct val truth(int b) {
    return val_new(b ? 1 : 0, 0);
}

static int negative(struct val *v) {
    return !v->is_unsigned && (int64_t)v->bits < 0;
}

/* The precedence of `?`, where the top of a condition starts, and of `,`,
 * which only parentheses or the middle of `?:` may hold (clang's). */
#define QUESTION 4
#define COMMA 3

struct eval {
    struct pp *pp;
    /* The next token; has_tok 0 at the end of the line. */
    int has_tok;
    struct ptok tok;
    /* Where the line ends, for what is missing there. */
    struct pos end;
};

/* The precedence of the binary operator that is the next token, as clang
 * has them: the higher binds tighter; `)`, `:` and the end of the line end
 * what is being read. -1 for a token that is no binary operator. */
struct binop {
    const char *op;
    int prec;
};

static const struct binop BINOPS[] = {
    {"*", 14},  {"/", 14},  {"%", 14},  {"+", 13},        {"-", 13},     {"<<", 12}, {">>", 12},
    {"<", 11},  {"<=", 11}, {">", 11},  {">=", 11},       {"==", 10},    {"!=", 10}, {"&", 9},
    {"^", 8},   {"|", 7},   {"&&", 6},  {"||", 5},        {"?", QUESTION}, {",", COMMA}, {":", 2},
    {")", 0},   {0, 0},
};

static int precedence(struct eval *ev) {
    if (!ev->has_tok) {
        return 0;
    }
    if (ev->tok.t.kind != PP_PUNCT) {
        return -1;
    }
    for (uint i = 0; BINOPS[i].op != NULL; i++) {
        if (strcmp(BINOPS[i].op, ev->tok.t.text) == 0) {
            return BINOPS[i].prec;
        }
    }
    return -1;
}

/* Reads the next token, macros replaced. */
static void advance(struct eval *ev) {
    /* The line is read on its own: no directive, no fatal error. */
    pp_next(ev->pp, &ev->tok, &ev->has_tok);
}

static struct pos ev_here(struct eval *ev) {
    return ev->has_tok ? ev->tok.t.pos : ev->end;
}

static int ev_at(struct eval *ev, const char *p) {
    return ev->has_tok && tok_is_punct(&ev->tok.t, p);
}

static struct diag *value(struct eval *ev, int live, struct val *out);
static struct diag *binary(struct eval *ev, struct val *lhs, int min, int live);

/* clang's warning for a signed result that does not fit. */
static void overflow(struct eval *ev, struct pos at) {
    report(ev->pp, diag_warning_unflagged(at, "integer overflow in preprocessor expression"));
}

/* Whether a preprocessing number is a floating constant: it has a `.` or an
 * exponent (`e` in decimal, `p` in hex). */
static int floating(const char *text) {
    int hex = text[0] == '0' && (text[1] == 'x' || text[1] == 'X');
    for (const char *s = text; *s; s++) {
        if (*s == '.' || (!hex && (*s == 'e' || *s == 'E')) || (hex && (*s == 'p' || *s == 'P'))) {
            return 1;
        }
    }
    return 0;
}

/* The low 64 bits of an integer literal too large for them, whose digits
 * lex_integer has checked. */
static uint64_t wrapped(const char *text) {
    uint radix = 10;
    const char *s = text;
    if (s[0] == '0' && (s[1] == 'x' || s[1] == 'X')) {
        radix = 16;
        s += 2;
    } else if (s[0] == '0') {
        radix = 8;
    }
    uint64_t v = 0;
    for (; *s; s++) {
        uint c = (uchar)*s;
        uint d;
        if (c >= '0' && c <= '9') {
            d = c - '0';
        } else if (c >= 'a' && c <= 'z') {
            d = c - 'a' + 10;
        } else if (c >= 'A' && c <= 'Z') {
            d = c - 'A' + 10;
        } else {
            break;
        }
        if (d >= radix) {
            break;
        }
        v = v * radix + d;
    }
    return v;
}

/* An integer literal: clang's errors for a floating one and for wrong
 * digits or suffixes. One too large for 64 bits is an error too, and its
 * low 64 bits, unsigned, go on; one too large for a signed value is
 * unsigned, which clang warns of in decimal. */
static struct diag *number(struct eval *ev, struct pptok *t, int live, struct val *out) {
    if (floating(t->text)) {
        return diag_error(t->pos, "floating point literal in preprocessor expression");
    }
    struct intlit lit;
    struct diag *e = lex_integer(t->text, t->len, t->pos, &lit);
    if (e != NULL) {
        return e;
    }
    if (!lit.fits) {
        const char *msg = "integer literal is too large to be represented in any integer type";
        report(ev->pp, diag_error(t->pos, msg));
        *out = val_new(wrapped(t->text), 1);
    } else if (!lit.is_unsigned && lit.value > 0x7fffffffffffffffull) {
        if (live && lit.decimal) {
            report(ev->pp, diag_warning(t->pos,
                                        "integer literal is too large to be represented in a signed integer "
                                        "type, interpreting as unsigned",
                                        "implicitly-unsigned-literal"));
        }
        *out = val_new(lit.value, 1);
    } else {
        *out = val_new(lit.value, lit.is_unsigned);
    }
    return NULL;
}

/* `defined NAME` or `defined ( NAME )`, the name not replaced (clang's
 * EvaluateDefined): 1 if it is a macro. A `defined` that a macro's
 * expansion produced is undefined behavior (C99 6.10.1p4), which clang
 * evaluates all the same, and warns of, but in a function-like macro's
 * body. */
static struct diag *defined(struct eval *ev, struct ptok *def, struct val *out) {
    struct ptok next;
    int got;
    next_raw(ev->pp, NULL, &next, &got);
    int has_open = 0;
    struct pos open;
    if (got && tok_is_punct(&next.t, "(")) {
        has_open = 1;
        open = next.t.pos;
        next_raw(ev->pp, NULL, &next, &got);
    }
    if (!got) {
        return diag_error(ev->end, "macro name missing");
    }
    if (next.t.kind != PP_IDENT) {
        return diag_error(next.t.pos, "macro name must be an identifier");
    }
    const char *name = next.t.text;
    if (has_open) {
        struct ptok close;
        next_raw(ev->pp, NULL, &close, &got);
        if (!got || !tok_is_punct(&close.t, ")")) {
            struct diag *e = diag_error(got ? close.t.pos : ev->end, "missing ')' after 'defined'");
            return diag_note(e, open, "to match this '('");
        }
    }
    int is_macro = map_get(&ev->pp->macros, name) != NULL;
    advance(ev);
    if (def->has_site && !def->function_body) {
        report(ev->pp, diag_warning(def->t.pos, "macro expansion producing 'defined' has undefined behavior",
                                    "expansion-to-defined"));
    }
    *out = truth(is_macro);
    return NULL;
}

/* A value (clang's EvaluateValue): a number, a character constant, an
 * identifier (0), `defined`, a unary operator and its operand, or an
 * expression in parentheses. */
static struct diag *value(struct eval *ev, int live, struct val *out) {
    if (!ev->has_tok) {
        return diag_error(ev->end, "expected value in expression");
    }
    struct ptok t = ev->tok;
    struct pos pos = t.t.pos;
    struct diag *e;
    if (t.t.kind == PP_IDENT && strcmp(t.t.text, "defined") == 0) {
        return defined(ev, &t, out);
    }
    if (t.t.kind == PP_IDENT) {
        advance(ev);
        *out = val_new(0, 0);
        out->ident = t.t.text;
        out->ident_pos = pos;
        return NULL;
    }
    if (t.t.kind == PP_NUMBER) {
        e = number(ev, &t.t, live, out);
        if (e != NULL) {
            return e;
        }
        advance(ev);
        return NULL;
    }
    if (t.t.kind == PP_CHAR) {
        struct diag *warnings = NULL;
        int64_t v;
        e = lex_char_constant(t.t.text, t.t.len, pos, &warnings, &v);
        if (e != NULL) {
            return e;
        }
        report_all(ev->pp, warnings);
        advance(ev);
        *out = val_new((uint64_t)v, 0);
        return NULL;
    }
    if (tok_is_punct(&t.t, "(")) {
        advance(ev);
        e = value(ev, live, out);
        if (e != NULL) {
            return e;
        }
        if (!ev_at(ev, ")")) {
            e = binary(ev, out, 1, live);
            if (e != NULL) {
                return e;
            }
            if (!ev_at(ev, ")")) {
                e = diag_error(ev_here(ev), "expected ')' in preprocessor expression");
                return diag_note(e, pos, "to match this '('");
            }
        }
        advance(ev);
        out->ident = NULL;
        return NULL;
    }
    if (tok_is_punct(&t.t, "+") || tok_is_punct(&t.t, "-") || tok_is_punct(&t.t, "~") ||
        tok_is_punct(&t.t, "!")) {
        const char *op = t.t.text;
        advance(ev);
        struct val v;
        e = value(ev, live, &v);
        if (e != NULL) {
            return e;
        }
        if (op[0] == '+') {
            *out = val_new(v.bits, v.is_unsigned);
        } else if (op[0] == '-') {
            /* Only the most negative value overflows. */
            if (live && !v.is_unsigned && v.bits == 0x8000000000000000ull) {
                overflow(ev, pos);
            }
            *out = val_new(0 - v.bits, v.is_unsigned);
        } else if (op[0] == '~') {
            *out = val_new(~v.bits, v.is_unsigned);
        } else {
            *out = truth(v.bits == 0);
        }
        return NULL;
    }
    if (tok_is_punct(&t.t, ")")) {
        return diag_error(pos, "expected value in expression");
    }
    return diag_error(pos, "invalid token at start of a preprocessor expression");
}

/* The precedence of the operator after the value v; an error if the token
 * there is none. */
static struct diag *precedence_after(struct eval *ev, struct val *v, int *prec) {
    *prec = precedence(ev);
    if (*prec >= 0) {
        return NULL;
    }
    if (tok_is_punct(&ev->tok.t, "(") && v->ident != NULL) {
        return diag_error(v->ident_pos, fmt("function-like macro '%s' is not defined", v->ident));
    }
    return diag_error(ev->tok.t.pos, "token is not a valid binary operator in a preprocessor subexpression");
}

/* The number of leading zeros, and of leading ones, of a 64-bit value. */
static uint leading_zeros(uint64_t a) {
    uint n = 0;
    while (n < 64 && !(a & (0x8000000000000000ull >> n))) {
        n++;
    }
    return n;
}

static uint leading_ones(uint64_t a) {
    return leading_zeros(~a);
}

/* An arithmetic right shift of a by n (less than 64): the sign copied in. */
static uint64_t shift_right_signed(uint64_t a, uint n) {
    if ((int64_t)a < 0) {
        return ~(~a >> n);
    }
    return a >> n;
}

/* `l op r`, the operator at `at`, with C's usual arithmetic conversions
 * (unsigned if either side is; not for shifts, which take the left side's
 * type, nor for `&&`, `||` and `,`), wrapping, with clang's warnings where
 * it is evaluated (`live`): for a negative value made unsigned, and for a
 * signed result that overflows. */
static struct diag *apply(struct eval *ev, const char *op, struct val *l, struct val *r, int live,
                          struct pos at, struct val *out) {
    int logical = strcmp(op, "<<") == 0 || strcmp(op, ">>") == 0 || strcmp(op, "&&") == 0 ||
                  strcmp(op, "||") == 0 || strcmp(op, ",") == 0;
    int is_unsigned = !logical && (l->is_unsigned || r->is_unsigned);
    if (live && is_unsigned) {
        for (int side = 0; side < 2; side++) {
            struct val *v = side == 0 ? l : r;
            if (negative(v)) {
                report(ev->pp, diag_warning_unflagged(
                                   at, fmt("%s side of operator converted from negative value to unsigned: "
                                           "%lld to %llu",
                                           side == 0 ? "left" : "right", (int64_t)v->bits, v->bits)));
            }
        }
    }
    uint64_t a = l->bits;
    uint64_t b = r->bits;
    int64_t sa = (int64_t)a;
    int64_t sb = (int64_t)b;
    int64_t min = (int64_t)0x8000000000000000ull;
    int over = 0;
    if (strcmp(op, "||") == 0) {
        *out = truth(a != 0 || b != 0);
    } else if (strcmp(op, "&&") == 0) {
        *out = truth(a != 0 && b != 0);
    } else if (strcmp(op, ",") == 0) {
        *out = val_new(b, r->is_unsigned);
    } else if (strcmp(op, "|") == 0) {
        *out = val_new(a | b, is_unsigned);
    } else if (strcmp(op, "^") == 0) {
        *out = val_new(a ^ b, is_unsigned);
    } else if (strcmp(op, "&") == 0) {
        *out = val_new(a & b, is_unsigned);
    } else if (strcmp(op, "==") == 0) {
        *out = truth(a == b);
    } else if (strcmp(op, "!=") == 0) {
        *out = truth(a != b);
    } else if (strcmp(op, "<") == 0) {
        *out = truth(is_unsigned ? a < b : sa < sb);
    } else if (strcmp(op, ">") == 0) {
        *out = truth(is_unsigned ? a > b : sa > sb);
    } else if (strcmp(op, "<=") == 0) {
        *out = truth(is_unsigned ? a <= b : sa <= sb);
    } else if (strcmp(op, ">=") == 0) {
        *out = truth(is_unsigned ? a >= b : sa >= sb);
    } else if (strcmp(op, "<<") == 0) {
        /* As APInt's ushl_ov and sshl_ov: a shift by 64 or more is 0, and
         * overflows; a shift that drops a set bit overflows (for a signed
         * value, one that changes its sign too). */
        uint64_t bits = 0;
        if (b >= 64) {
            over = 1;
        } else {
            uint n = (uint)b;
            if (l->is_unsigned) {
                over = n > leading_zeros(a);
            } else if (sa >= 0) {
                over = n >= leading_zeros(a);
            } else {
                over = n >= leading_ones(a);
            }
            bits = a << n;
        }
        *out = val_new(bits, l->is_unsigned);
    } else if (strcmp(op, ">>") == 0) {
        /* A shift by the width or more shifts by one less. */
        over = b >= 64;
        uint n = b > 63 ? 63 : (uint)b;
        *out = val_new(l->is_unsigned ? a >> n : shift_right_signed(a, n), l->is_unsigned);
    } else if (strcmp(op, "+") == 0) {
        uint64_t s = a + b;
        over = !is_unsigned && (sa < 0) == (sb < 0) && ((int64_t)s < 0) != (sa < 0);
        *out = val_new(s, is_unsigned);
    } else if (strcmp(op, "-") == 0) {
        uint64_t s = a - b;
        over = !is_unsigned && (sa < 0) != (sb < 0) && ((int64_t)s < 0) != (sa < 0);
        *out = val_new(s, is_unsigned);
    } else if (strcmp(op, "*") == 0) {
        uint64_t p = a * b;
        if (!is_unsigned && sa != 0 && sb != 0) {
            if ((sa == -1 && sb == min) || (sb == -1 && sa == min)) {
                over = 1;
            } else if (sb != -1 && (int64_t)p / sb != sa) {
                over = 1;
            } else if (sb == -1 && (int64_t)p != 0 - sa) {
                over = 1;
            }
        }
        *out = val_new(p, is_unsigned);
    } else if (b == 0) {
        /* `/` or `%` by zero. */
        if (live) {
            return diag_error(at, fmt("%s by zero in preprocessor expression",
                                      strcmp(op, "/") == 0 ? "division" : "remainder"));
        }
        *out = val_new(0, is_unsigned);
    } else if (is_unsigned) {
        *out = val_new(strcmp(op, "/") == 0 ? a / b : a % b, is_unsigned);
    } else if (strcmp(op, "/") == 0) {
        over = sa == min && sb == -1;
        *out = val_new(sb == -1 ? 0 - a : (uint64_t)(sa / sb), is_unsigned);
    } else {
        *out = val_new(sb == -1 ? 0 : (uint64_t)(sa % sb), is_unsigned);
    }
    if (over && live) {
        overflow(ev, at);
    }
    return NULL;
}

/* The binary operators after lhs whose precedence is at least min (clang's
 * EvaluateDirectiveSubExpr), applied to it: each takes as its right side a
 * value and the operators after it that bind tighter, but `?`, whose middle
 * runs to its `:` and whose end groups to the right. */
static struct diag *binary(struct eval *ev, struct val *lhs, int min, int live) {
    int prec;
    struct diag *e = precedence_after(ev, lhs, &prec);
    if (e != NULL) {
        return e;
    }
    while (prec >= min) {
        /* Only operators bind. */
        const char *op = ev->tok.t.text;
        struct pos at = ev_here(ev);
        /* The right side of `0 && x`, `1 || x` and `0 ? x : y` is not
         * evaluated. */
        int rhs_live = live;
        if (strcmp(op, "&&") == 0 || strcmp(op, "?") == 0) {
            rhs_live = live && lhs->bits != 0;
        } else if (strcmp(op, "||") == 0) {
            rhs_live = live && lhs->bits == 0;
        }
        advance(ev);
        struct val rhs;
        e = value(ev, rhs_live, &rhs);
        if (e != NULL) {
            return e;
        }
        int this = prec;
        e = precedence_after(ev, &rhs, &prec);
        if (e != NULL) {
            return e;
        }
        int rhs_min = strcmp(op, "?") == 0 ? COMMA : this + 1;
        if (prec >= rhs_min) {
            e = binary(ev, &rhs, rhs_min, rhs_live);
            if (e != NULL) {
                return e;
            }
            e = precedence_after(ev, &rhs, &prec);
            if (e != NULL) {
                return e;
            }
        }
        if (strcmp(op, "?") == 0) {
            if (!ev_at(ev, ":")) {
                e = diag_error(ev_here(ev), "expected ':'");
                return diag_note(e, at, "to match this '?'");
            }
            advance(ev);
            int after_live = live && lhs->bits == 0;
            struct val after;
            e = value(ev, after_live, &after);
            if (e != NULL) {
                return e;
            }
            e = binary(ev, &after, this, after_live);
            if (e != NULL) {
                return e;
            }
            e = precedence_after(ev, &after, &prec);
            if (e != NULL) {
                return e;
            }
            int is_unsigned = rhs.is_unsigned || after.is_unsigned;
            *lhs = val_new(lhs->bits != 0 ? rhs.bits : after.bits, is_unsigned);
        } else if (strcmp(op, ":") == 0) {
            return diag_error(at, "':' without preceding '?'");
        } else {
            struct val v;
            e = apply(ev, op, lhs, &rhs, live, at, &v);
            if (e != NULL) {
                return e;
            }
            *lhs = v;
        }
    }
    return NULL;
}

/* The whole line (clang's EvaluateDirectiveExpression): an expression,
 * then the end of the line. Anything else there is an error, but the value
 * stands (`#if 1 )` is true). */
static struct diag *directive_expression(struct eval *ev, int *value_out) {
    advance(ev);
    struct val v;
    struct diag *e = value(ev, 1, &v);
    if (e != NULL) {
        return e;
    }
    if (ev->has_tok) {
        e = binary(ev, &v, QUESTION, 1);
        if (e != NULL) {
            return e;
        }
        if (ev->has_tok) {
            report(ev->pp, diag_error(ev->tok.t.pos, "expected end of line in preprocessor expression"));
        }
    }
    *value_out = v.bits != 0;
    return NULL;
}

/* The value of an #if or #elif condition, which the evaluator reads as the
 * rest of the line is replaced: false if it has an error, as in clang. */
static int condition(struct pp *pp, struct pptok *dir, struct pptok *args, uint nargs) {
    struct toks line;
    read_all(args, nargs, &line);
    struct deque saved = pp->pending;
    pp->pending = deque_of(&line);
    toks_free(&line);
    int was_isolated = pp->isolated;
    pp->isolated = 1;
    int cb = pp->carry_bol;
    int cs = pp->carry_space;
    int cl = pp->carry_lead;
    pp->carry_bol = pp->carry_space = pp->carry_lead = 0;
    struct eval ev;
    memset(&ev, 0, sizeof ev);
    ev.pp = pp;
    ev.end = nargs > 0 ? args[nargs - 1].end : dir->end;
    int v = 0;
    struct diag *e = directive_expression(&ev, &v);
    heap_free(pp->pending.buf);
    pp->pending = saved;
    pp->isolated = was_isolated;
    pp->carry_bol = cb;
    pp->carry_space = cs;
    pp->carry_lead = cl;
    if (e != NULL) {
        report(pp, e);
        return 0;
    }
    return v;
}

/* ---- Directives ---- */

/* clang's warning for tokens after what a directive takes, macros replaced
 * first for `expand` (`#include "a.h" EMPTY` has none). */
static void extra_tokens(struct pp *pp, struct pptok *rest, uint n, const char *dir, int expand) {
    int has = 0;
    struct pos pos;
    if (expand) {
        struct toks in;
        struct toks out;
        read_all(rest, n, &in);
        expand_alone(pp, &in, &out);
        if (out.n > 0) {
            has = 1;
            pos = out.v[0].t.pos;
        }
        toks_free(&in);
        toks_free(&out);
    } else if (n > 0) {
        has = 1;
        pos = rest[0].pos;
    }
    if (has) {
        report(pp, diag_warning(pos, fmt("extra tokens at end of #%s directive", dir), "extra-tokens"));
    }
}

/* The name a #define, #undef, #ifdef or #ifndef is about; one may not
 * define or undefine (define_undef) `defined`. */
static struct diag *macro_name(struct pptok *dir, struct pptok *args, uint nargs, int define_undef,
                               const char **name) {
    if (nargs == 0) {
        return diag_error(dir->end, "macro name missing");
    }
    struct pptok *t = &args[0];
    if (t->kind != PP_IDENT) {
        return diag_error(t->pos, "macro name must be an identifier");
    }
    if (define_undef && strcmp(t->text, "defined") == 0) {
        return diag_error(t->pos, "'defined' cannot be used as a macro name");
    }
    *name = t->text;
    return NULL;
}

/* Whether a macro is one C itself defines: __FILE__, __LINE__, or a
 * predefined one whose name starts with __STDC. C99 6.10.8p4 says not to
 * define them again or undefine them; clang warns, and lets it be done. */
static int language_builtin(struct pp *pp, struct macro *m, const char *name) {
    return m->kind == MACRO_FILE || m->kind == MACRO_LINE ||
           (m->pos.file == pp->builtin_file && strncmp(name, "__STDC", 6) == 0);
}

static struct diag *undef(struct pp *pp, struct pptok *dir, struct pptok *args, uint nargs) {
    const char *name;
    struct diag *e = macro_name(dir, args, nargs, 1, &name);
    if (e != NULL) {
        return e;
    }
    extra_tokens(pp, args + 1, nargs - 1, "undef", 0);
    struct macro *m = map_get(&pp->macros, name);
    if (m != NULL && language_builtin(pp, m, name)) {
        report(pp, diag_warning(args[0].pos, "undefining builtin macro", "builtin-macro-redefined"));
    }
    map_remove(&pp->macros, name);
    return NULL;
}

/* The parameters of a function-like macro, as clang's
 * ReadMacroParameterList reads them, from rest, which starts at the `(`,
 * on a line that ends at eod: their names, whether the last one is
 * variadic, and where the body starts (*body). C99's `...` is the
 * parameter __VA_ARGS__; GNU's `args...` makes args the variadic one. */
static struct diag *parameters(struct pptok *rest, uint n, struct pos eod, struct macro *m, uint *body) {
    uint cap = 0;
    uint i = 1;
    for (;;) {
        if (i >= n) {
            return diag_error(eod, "missing ')' in macro parameter list");
        }
        struct pptok *t = &rest[i++];
        if (tok_is_punct(t, ")")) {
            if (m->nparams == 0) {
                *body = i;
                return NULL;
            }
            return diag_error(t->pos, "expected identifier in macro parameter list");
        }
        if (tok_is_punct(t, "...")) {
            if (i >= n || !tok_is_punct(&rest[i], ")")) {
                return diag_error(i < n ? rest[i].pos : eod, "missing ')' in macro parameter list");
            }
            m->params = grow(m->params, &cap, m->nparams + 1, sizeof(char *));
            m->params[m->nparams++] = intern("__VA_ARGS__", 11);
            m->variadic = 1;
            *body = i + 1;
            return NULL;
        }
        if (t->kind != PP_IDENT) {
            return diag_error(t->pos, "invalid token in macro parameter list");
        }
        for (uint k = 0; k < m->nparams; k++) {
            if (strcmp(m->params[k], t->text) == 0) {
                return diag_error(t->pos, fmt("duplicate macro parameter name '%s'", t->text));
            }
        }
        m->params = grow(m->params, &cap, m->nparams + 1, sizeof(char *));
        m->params[m->nparams++] = t->text;
        if (i < n && tok_is_punct(&rest[i], ")")) {
            *body = i + 1;
            return NULL;
        }
        if (i < n && tok_is_punct(&rest[i], ",")) {
            i++;
            continue;
        }
        if (i < n && tok_is_punct(&rest[i], "...")) {
            if (i + 1 >= n || !tok_is_punct(&rest[i + 1], ")")) {
                return diag_error(i + 1 < n ? rest[i + 1].pos : eod, "missing ')' in macro parameter list");
            }
            m->variadic = 1;
            *body = i + 2;
            return NULL;
        }
        return diag_error(i < n ? rest[i].pos : eod, "expected comma in macro parameter list");
    }
}

static struct diag *define(struct pp *pp, struct pptok *dir, struct pptok *args, uint nargs) {
    const char *name;
    struct diag *e = macro_name(dir, args, nargs, 1, &name);
    if (e != NULL) {
        return e;
    }
    struct pos name_pos = args[0].pos;
    struct pptok *rest = args + 1;
    uint nrest = nargs - 1;
    /* Where the line ends, for what is missing at its end. */
    struct pos eod = args[nargs - 1].end;
    struct macro *m = arena_alloc(sizeof *m);
    m->pos = name_pos;
    uint body = 0;
    /* `F(` with no space before the `(` makes a function-like macro. */
    if (nrest > 0 && tok_is_punct(&rest[0], "(") && !rest[0].space) {
        m->kind = MACRO_FUNCTION;
        e = parameters(rest, nrest, eod, m, &body);
        if (e != NULL) {
            heap_free(m->params);
            return e;
        }
        /* The parameters, kept with the macro. */
        const char **params = arena_alloc((m->nparams + 1) * sizeof(char *));
        memcpy(params, m->params, m->nparams * sizeof(char *));
        heap_free(m->params);
        m->params = params;
    } else {
        m->kind = MACRO_OBJECT;
    }
    m->nbody = nrest - body;
    m->body = arena_alloc((m->nbody + 1) * sizeof(struct pptok));
    memcpy(m->body, rest + body, m->nbody * sizeof(struct pptok));
    /* What the body may hold, checked in clang's order: in a function-like
     * macro `#` before a parameter (in assembly before anything but the end
     * of the line: then it is no operator, a token of no kind, as clang
     * makes it), then `##` between two tokens. */
    if (m->kind == MACRO_FUNCTION) {
        for (uint i = 0; i < m->nbody; i++) {
            struct pptok *next = i + 1 < m->nbody ? &m->body[i + 1] : NULL;
            if (tok_is_punct(&m->body[i], "#") && !(next != NULL && macro_param(m, next) >= 0)) {
                if (!pp->asm || next == NULL) {
                    struct pos at = next != NULL ? next->pos : eod;
                    return diag_error(at, "'#' is not followed by a macro parameter");
                }
                m->body[i].kind = PP_INVALID;
                m->body[i].msg = NULL;
            }
        }
    }
    if (m->nbody > 0 && tok_is_punct(&m->body[0], "##")) {
        return diag_error(m->body[0].pos, "'##' cannot appear at start of macro expansion");
    }
    if (m->nbody > 0 && tok_is_punct(&m->body[m->nbody - 1], "##")) {
        return diag_error(m->body[m->nbody - 1].pos, "'##' cannot appear at end of macro expansion");
    }
    /* Defining a macro again: the same definition is fine (C99 6.10.3p2),
     * another one clang's warning; the new one holds. */
    struct macro *prev = map_get(&pp->macros, name);
    if (prev != NULL) {
        if (language_builtin(pp, prev, name)) {
            report(pp, diag_warning(name_pos, "redefining builtin macro", "builtin-macro-redefined"));
        } else if (!macro_same(prev, m)) {
            struct diag *w = diag_warning(name_pos, fmt("'%s' macro redefined", name), "macro-redefined");
            report(pp, diag_note(w, prev->pos, "previous definition is here"));
        }
    }
    map_put(&pp->macros, name, m);
    return NULL;
}

/* The include guard of a file, from its tokens: X when its first line is
 * `#ifndef X` (or `#if !defined X`, `#if !defined(X)`) and the #endif that
 * closes it is its last, with no #elif or #else of that #if between them,
 * as clang's multiple-include optimization asks. NULL if none. */
static const char *line_directive(struct pptok *t, uint n) {
    if (n >= 2 && tok_is_punct(&t[0], "#") && t[1].kind == PP_IDENT) {
        return t[1].text;
    }
    return NULL;
}

static const char *guard(struct lexed *lx) {
    struct pptok *t = lx->toks;
    uint n = lx->n;
    if (n == 0) {
        return NULL;
    }
    /* The first line. */
    uint len = 1;
    while (len < n && !t[len].bol) {
        len++;
    }
    const char *d = line_directive(t, len);
    const char *name = NULL;
    if (d != NULL && strcmp(d, "ifndef") == 0 && len == 3) {
        name = tok_ident(&t[2]);
    } else if (d != NULL && strcmp(d, "if") == 0 && (len == 5 || len == 7) && tok_is_punct(&t[2], "!") &&
               t[3].kind == PP_IDENT && strcmp(t[3].text, "defined") == 0 &&
               (len == 5 || (tok_is_punct(&t[4], "(") && tok_is_punct(&t[6], ")")))) {
        name = tok_ident(&t[len == 5 ? 4 : 5]);
    }
    if (name == NULL) {
        return NULL;
    }
    int depth = 0;
    uint start = 0;
    while (start < n) {
        uint end = start + 1;
        while (end < n && !t[end].bol) {
            end++;
        }
        d = line_directive(t + start, end - start);
        if (d != NULL) {
            if (strcmp(d, "if") == 0 || strcmp(d, "ifdef") == 0 || strcmp(d, "ifndef") == 0) {
                depth++;
            } else if ((strcmp(d, "elif") == 0 || strcmp(d, "elifdef") == 0 || strcmp(d, "elifndef") == 0 ||
                        strcmp(d, "else") == 0) &&
                       depth == 1) {
                return NULL;
            } else if (strcmp(d, "endif") == 0) {
                depth--;
                if (depth == 0) {
                    return end == n ? name : NULL;
                }
            }
        }
        start = end;
    }
    return NULL;
}

/* `"file"` or `<file>` at the start of an #include's tokens, on a line that
 * ends at eod: the name (*len bytes), whether it was `<file>`, where it is
 * and how many tokens it took (*used, 0 if neither form is there). */
static struct diag *header_name(struct pptok *t, uint n, struct pos eod, const char **name, uint *len,
                                int *angled, struct pos *pos, uint *used) {
    *used = 0;
    if (n == 0) {
        return NULL;
    }
    if (t[0].kind == PP_STR) {
        /* Not a string literal: the characters between the quotes, as they
         * are. */
        *len = t[0].len - 2;
        *name = arena_strndup(t[0].text + 1, *len);
        *angled = 0;
        *pos = t[0].pos;
        *used = 1;
        return NULL;
    }
    if (!tok_is_punct(&t[0], "<")) {
        return NULL;
    }
    uint close = 0;
    while (close < n && !tok_is_punct(&t[close], ">")) {
        close++;
    }
    if (close == n) {
        return diag_note(diag_error(eod, "expected '>'"), t[0].pos, "to match this '<'");
    }
    struct buf b;
    memset(&b, 0, sizeof b);
    for (uint k = 1; k < close; k++) {
        if (k > 1 && t[k].space) {
            buf_byte(&b, ' ');
        }
        buf_put(&b, t[k].text, t[k].len);
    }
    *name = buf_arena(&b);
    *len = b.len;
    buf_free(&b);
    *angled = 1;
    *pos = t[0].pos;
    *used = close + 1;
    return NULL;
}

/* The file an #include names: "file" next to the including file first,
 * then in each -I directory in order; <file> only in those. Its name (by
 * the directory it was found in, *namelen bytes), path and text; 0 if
 * none. The header's name holds no NUL (one that does is no file). */
static int find(struct pp *pp, const char *header, int angled, const char **name, uint *namelen,
                const char **path, char **text, uint *len) {
    uint cur = top(pp)->file;
    uint ndirs = pp->opts->ninclude_dirs + 1;
    for (uint k = 0; k < ndirs; k++) {
        const char *shown;
        uint shownlen = 0;
        const char *dir;
        if (k == 0) {
            if (angled) {
                continue;
            }
            const char *p = pp->files[cur].path;
            uint plen;
            dir = parent(p, (uint)strlen(p), &plen);
            if (dir == NULL) {
                dir = "";
            }
            /* Named by the including file's directory, `.` for one named
             * without (clang: `./sub/s.h` next to `b.S`, the __FILE__ in it
             * too). */
            shown = parent(pp->files[cur].name, pp->files[cur].namelen, &shownlen);
            if (shown == NULL || shownlen == 0) {
                shown = ".";
                shownlen = 1;
            }
        } else {
            shown = pp->opts->include_dirs[k - 1];
            shownlen = (uint)strlen(shown);
            dir = shown;
        }
        const char *p = normalize(join(dir, header));
        char *t = load(p, len);
        if (t == NULL) {
            continue;
        }
        if (shownlen == 0) {
            *name = header;
            *namelen = (uint)strlen(header);
        } else {
            uint n = shownlen;
            while (n > 0 && (shown[n - 1] == '/' || shown[n - 1] == '\\')) {
                n--;
            }
            *name = fmtn(namelen, "%b/%s", shown, n, header);
        }
        *path = p;
        *text = t;
        return 1;
    }
    return 0;
}

static struct diag *include(struct pp *pp, struct pptok *dir, struct pptok *args, uint nargs) {
    struct pos eod = nargs > 0 ? args[nargs - 1].end : dir->end;
    const char *header;
    uint hlen = 0;
    int angled;
    struct pos pos;
    uint used;
    struct diag *e = header_name(args, nargs, eod, &header, &hlen, &angled, &pos, &used);
    if (e != NULL) {
        return e;
    }
    if (used > 0) {
        extra_tokens(pp, args + used, nargs - used, "include", 1);
    } else {
        /* `#include MACRO`: replace, then it must be one of the two forms. */
        struct toks in;
        struct toks out;
        read_all(args, nargs, &in);
        expand_alone(pp, &in, &out);
        toks_free(&in);
        struct pptok *expanded = heap_alloc((out.n + 1) * sizeof(struct pptok));
        for (uint i = 0; i < out.n; i++) {
            expanded[i] = out.v[i].t;
        }
        uint n = out.n;
        toks_free(&out);
        e = header_name(expanded, n, eod, &header, &hlen, &angled, &pos, &used);
        if (e == NULL && used == 0) {
            e = diag_error(nargs > 0 ? args[0].pos : dir->end, "expected \"FILENAME\" or <FILENAME>");
        }
        if (e != NULL) {
            heap_free(expanded);
            return e;
        }
        extra_tokens(pp, expanded + used, n - used, "include", 0);
        heap_free(expanded);
    }
    if (hlen == 0) {
        return diag_error(pos, "empty filename");
    }
    /* A name with a NUL names no file (merlin's paths cannot hold one). */
    int nul = strlen(header) != hlen;
    const char *name;
    uint namelen;
    const char *path;
    char *text;
    uint len;
    if (nul || !find(pp, header, angled, &name, &namelen, &path, &text, &len)) {
        /* clang looks for an <a.h> it does not find as "a.h" too, and
         * includes what it finds, with an error. */
        uint mlen;
        const char *msg;
        if (nul || !angled || !find(pp, header, 0, &name, &namelen, &path, &text, &len)) {
            msg = fmtn(&mlen, "'%b' file not found", header, hlen);
            return diag_fatal_n(pos, msg, mlen);
        }
        msg = fmtn(&mlen, "'%b' file not found with <angled> include; use \"quotes\" instead", header, hlen);
        report(pp, diag_error_n(pos, msg, mlen));
    }
    if (map_get(&pp->once, path) != NULL) {
        heap_free(text);
        return NULL;
    }
    /* A file wholly inside `#ifndef X` ... `#endif` is not read again once X
     * is defined (clang's and GCC's multiple-include optimization): nothing
     * of it would come out, and no line marker says it was entered. */
    const char *g = map_get(&pp->guards, path);
    if (g != NULL && map_get(&pp->macros, g) != NULL) {
        heap_free(text);
        return NULL;
    }
    if (pp->nstack >= MAX_INCLUDE_DEPTH) {
        heap_free(text);
        return diag_error(pos, "#include nested too deeply");
    }
    uint id = new_file(pp, name, namelen, path, text, len);
    struct lexed lx;
    lex_pp(text, len, id, pp->asm, &lx);
    if (lx.error != NULL) {
        heap_free(lx.toks);
        heap_free(text);
        return lx.error;
    }
    g = guard(&lx);
    if (g != NULL) {
        map_put(&pp->guards, path, (void *)g);
    }
    /* The file that includes it goes on after the directive's line. */
    uint back = (nargs > 0 ? args[nargs - 1].end.line : dir->end.line) + 1;
    add_mark(pp, MARK_ENTER, dir->pos.line, id);
    enter(pp, &lx, text, len, 1, id);
    top(pp)->has_back = 1;
    top(pp)->back = back;
    return NULL;
}

/* The number of a #line or a line marker, or of a flag, as clang's
 * GetLineValue reads it: digits, in decimal even with a leading 0 (which
 * clang warns of), that fit in 32 bits; `missing` is the error for anything
 * else, `what` names the directive in the other messages. tok may be NULL. */
static struct diag *line_value(struct pp *pp, struct pptok *tok, struct pos eod, const char *missing,
                               const char *what, uint *out) {
    if (tok == NULL || tok->kind != PP_NUMBER) {
        return diag_error(tok != NULL ? tok->pos : eod, missing);
    }
    uint v = 0;
    for (uint k = 0; k < tok->len; k++) {
        uint c = (uchar)tok->text[k];
        if (c < '0' || c > '9') {
            return diag_error(pos_at(tok->pos.file, tok->pos.line, tok->pos.col + k),
                              fmt("%s directive requires a simple digit sequence", what));
        }
        uint d = c - '0';
        if (v > (0xffffffffu - d) / 10) {
            return diag_error(tok->pos, missing);
        }
        v = v * 10 + d;
    }
    if (tok->text[0] == '0' && v != 0) {
        const char *msg = fmt("%s directive interprets number as decimal, not octal", what);
        report(pp, diag_warning_unflagged(tok->pos, msg));
    }
    *out = v;
    return NULL;
}

/* The file name a #line or a line marker gives, a string literal: *len
 * bytes, which may hold a NUL. */
static struct diag *file_name(struct pp *pp, struct pptok *t, const char **name, uint *len) {
    struct diag *warnings = NULL;
    struct buf bytes;
    memset(&bytes, 0, sizeof bytes);
    struct diag *e = lex_decode(t->text, t->len, t->pos, &warnings, &bytes);
    if (e != NULL) {
        buf_free(&bytes);
        return e;
    }
    report_all(pp, warnings);
    /* The name is a string nothing evaluates, which has only the simple
     * escapes (`\\`, `\n`...) and universal character names: an octal or
     * hex one, or one C has not, is clang's error, at its backslash. */
    const char *body = t->text + 1;
    uint n = t->len - 2;
    uint i = 0;
    while (i < n) {
        uint c = (uchar)body[i];
        if (c != '\\') {
            i += utf8_len(c);
            continue;
        }
        uint at = i;
        if (i + 1 >= n) {
            break;
        }
        uint e2 = (uchar)body[i + 1];
        uint elen = utf8_len(e2);
        i += 1 + elen;
        if (strchr("'\"?\\abfnrtvuU", (int)e2) != NULL && e2 != 0) {
            continue;
        }
        uint seq_end = i;
        if (e2 >= '0' && e2 <= '7') {
            for (int k = 0; k < 2 && seq_end < n && body[seq_end] >= '0' && body[seq_end] <= '7'; k++) {
                seq_end++;
            }
        } else if (e2 == 'x') {
            while (seq_end < n && ((body[seq_end] >= '0' && body[seq_end] <= '9') ||
                                   (body[seq_end] >= 'a' && body[seq_end] <= 'f') ||
                                   (body[seq_end] >= 'A' && body[seq_end] <= 'F'))) {
                seq_end++;
            }
        }
        buf_free(&bytes);
        uint mlen;
        const char *msg = fmtn(&mlen, "invalid escape sequence '%b' in an unevaluated string literal",
                               body + at, seq_end - at);
        return diag_error_n(pos_at(t->pos.file, t->pos.line, t->pos.col + 1 + at), msg, mlen);
    }
    *name = buf_arena(&bytes);
    *len = bytes.len;
    buf_free(&bytes);
    return NULL;
}

/* After the directive at dir, the next line is line n (of a file called
 * rename, renamelen bytes, if not NULL). */
static void renumber(struct pp *pp, struct pptok *dir, uint n, const char *rename, uint renamelen) {
    struct cursor *c = top(pp);
    /* The directive's own line, as the lexer numbered it. */
    int64_t written = (int64_t)dir->pos.line - c->line_delta;
    if (rename != NULL) {
        /* A name #line gives has no checksum, as clang's: its contents are
         * not what is read. */
        uint id = new_file(pp, rename, renamelen, pp->files[c->file].path, NULL, 0);
        c = top(pp);
        c->file = id;
    }
    c->line_delta = (int64_t)n - (written + 1);
    add_mark(pp, MARK_RENAME, n, c->file);
}

/* The tokens of a directive's line, macros replaced: an array on the heap of
 * *n. */
static struct pptok *expand_line(struct pp *pp, struct pptok *args, uint nargs, uint *n) {
    struct toks in;
    struct toks out;
    read_all(args, nargs, &in);
    expand_alone(pp, &in, &out);
    toks_free(&in);
    struct pptok *t = heap_alloc((out.n + 1) * sizeof(struct pptok));
    for (uint i = 0; i < out.n; i++) {
        t[i] = out.v[i].t;
    }
    *n = out.n;
    toks_free(&out);
    return t;
}

/* `#line N` or `#line N "name"` (after macro replacement): the next line is
 * line N (of a file called name). */
static struct diag *hash_line(struct pp *pp, struct pptok *dir, struct pptok *args, uint nargs) {
    struct pos eod = nargs > 0 ? args[nargs - 1].end : dir->end;
    uint n;
    struct pptok *t = expand_line(pp, args, nargs, &n);
    uint v;
    const char *missing = "#line directive requires a positive integer argument";
    struct diag *e = line_value(pp, n > 0 ? &t[0] : NULL, eod, missing, "#line", &v);
    const char *rename = NULL;
    uint renamelen = 0;
    if (e == NULL && n > 1) {
        if (t[1].kind == PP_STR && t[1].text[0] == '"') {
            e = file_name(pp, &t[1], &rename, &renamelen);
            if (e == NULL) {
                extra_tokens(pp, t + 2, n - 2, "line", 0);
            }
        } else {
            e = diag_error(t[1].pos, "invalid filename for #line directive");
        }
    }
    heap_free(t);
    if (e != NULL) {
        return e;
    }
    renumber(pp, dir, v, rename, renamelen);
    return NULL;
}

/* The flags of a line marker, after its file name, checked as clang's
 * ReadLineMarkerFlags does: 1 (a file starts) or 2 (one that a marker
 * started ends), then 3, then 4, each at most once and in that order. How
 * many files markers have started and not ended after them, in *entered. */
static struct diag *marker_flags(struct pp *pp, struct pptok *flags, uint n, struct pos eod, uint *entered) {
    const char *invalid = "invalid flag line marker directive";
    *entered = top(pp)->entered_by_markers;
    if (n == 0) {
        return NULL;
    }
    uint i = 0;
    uint value;
    struct diag *e = line_value(pp, &flags[0], eod, invalid, "#line", &value);
    if (e != NULL) {
        return e;
    }
    if (value == 1 || value == 2) {
        if (value == 1) {
            *entered += 1;
        } else if (*entered == 0) {
            return diag_error(flags[0].pos, "invalid line marker flag '2': cannot pop empty include stack");
        } else {
            *entered -= 1;
        }
        i++;
        if (i >= n) {
            return NULL;
        }
        e = line_value(pp, &flags[i], eod, invalid, "#line", &value);
        if (e != NULL) {
            return e;
        }
    }
    /* Then 3, then 4, and nothing after. */
    for (uint want = 3; want <= 4; want++) {
        if (value != want) {
            return diag_error(flags[i].pos, invalid);
        }
        i++;
        if (i >= n) {
            return NULL;
        }
        if (want == 4) {
            return diag_error(flags[i].pos, invalid);
        }
        e = line_value(pp, &flags[i], eod, invalid, "#line", &value);
        if (e != NULL) {
            return e;
        }
    }
    return NULL;
}

/* `# N "name" flags`, GNU's line marker, which -E writes: what
 * `#line N "name"` does. Its flags are checked as clang checks them, and
 * nothing else is done with them. */
static struct diag *line_marker(struct pp *pp, struct pptok *digits, struct pptok *args, uint nargs) {
    struct pos eod = nargs > 0 ? args[nargs - 1].end : digits->end;
    uint v;
    struct diag *e = line_value(pp, digits, eod, "line marker directive requires a positive integer argument",
                                "GNU line marker", &v);
    if (e != NULL) {
        return e;
    }
    uint n;
    struct pptok *t = expand_line(pp, args, nargs, &n);
    const char *rename = NULL;
    uint renamelen = 0;
    if (n > 0) {
        if (t[0].kind == PP_STR && t[0].text[0] == '"') {
            e = file_name(pp, &t[0], &rename, &renamelen);
            uint entered = 0;
            if (e == NULL) {
                e = marker_flags(pp, t + 1, n - 1, eod, &entered);
            }
            if (e == NULL) {
                top(pp)->entered_by_markers = entered;
            }
        } else {
            e = diag_error(t[0].pos, "invalid filename for line marker directive");
        }
    }
    heap_free(t);
    if (e != NULL) {
        return e;
    }
    renumber(pp, digits, v, rename, renamelen);
    return NULL;
}

/* ---- Conditionals ---- */

/* The innermost #if of the current file, for #elif, #else and #endif. */
static struct diag *innermost_cond(struct pp *pp, struct pptok *dir, struct cond **c) {
    if (pp->nconds <= top(pp)->conds) {
        return diag_error(dir->pos, fmt("#%s without #if", dir->text));
    }
    *c = &pp->conds[pp->nconds - 1];
    return NULL;
}

/* Skips a group of the innermost conditional, and any that follow it, until
 * one is taken (an #elif whose condition holds, or #else, if none was taken
 * before) or its #endif closes it. Only directives are looked at, to follow
 * the nesting of conditionals. */
static void skip_lines(struct pp *pp) {
    /* Where the conditionals that open in the skipped lines are. */
    struct pos *nested = NULL;
    uint nnested = 0;
    uint cap = 0;
    for (;;) {
        struct pptok *t = peek_file(pp);
        if (t == NULL) {
            /* clang reports every conditional left open at the end of the
             * file, the innermost first (leave() those being skipped). */
            while (nnested > 0) {
                report(pp, diag_error(nested[--nnested], "unterminated conditional directive"));
            }
            heap_free(nested);
            return;
        }
        if (!(t->bol && tok_is_punct(t, "#"))) {
            take_file(pp);
            continue;
        }
        take_file(pp); /* # */
        uint n;
        struct pptok *line = rest_of_line(pp, &n);
        if (n == 0) {
            continue;
        }
        struct pptok *first = &line[0];
        struct pptok *args = line + 1;
        const char *d = tok_ident(first);
        if (d == NULL) {
            heap_free(line);
            continue;
        }
        if (strcmp(d, "if") == 0 || strcmp(d, "ifdef") == 0 || strcmp(d, "ifndef") == 0) {
            nested = grow(nested, &cap, nnested + 1, sizeof(struct pos));
            nested[nnested++] = first->pos;
        } else if (strcmp(d, "endif") == 0 && nnested > 0) {
            nnested--;
        } else if (strcmp(d, "endif") == 0) {
            extra_tokens(pp, args, n - 1, "endif", 0);
            pp->nconds--;
            heap_free(line);
            heap_free(nested);
            return;
        } else if ((strcmp(d, "elif") == 0 || strcmp(d, "else") == 0) && nnested == 0) {
            struct cond *c = &pp->conds[pp->nconds - 1];
            int taken = c->taken;
            if (c->seen_else) {
                report(pp, diag_error(first->pos, fmt("#%s after #else", d)));
            }
            if (strcmp(d, "else") == 0) {
                c = &pp->conds[pp->nconds - 1];
                c->seen_else = 1;
                if (!taken) {
                    c->taken = 1;
                    extra_tokens(pp, args, n - 1, "else", 0);
                    heap_free(line);
                    heap_free(nested);
                    return;
                }
            } else if (!taken && condition(pp, first, args, n - 1)) {
                pp->conds[pp->nconds - 1].taken = 1;
                heap_free(line);
                heap_free(nested);
                return;
            }
        }
        heap_free(line);
    }
}

static void skip_group(struct pp *pp) {
    pp->skipping = 1;
    skip_lines(pp);
    pp->skipping = 0;
}

static void open_cond(struct pp *pp, struct pos pos, int value) {
    pp->conds = grow(pp->conds, &pp->capconds, pp->nconds + 1, sizeof(struct cond));
    pp->conds[pp->nconds].pos = pos;
    pp->conds[pp->nconds].taken = value;
    pp->conds[pp->nconds].seen_else = 0;
    pp->nconds++;
    if (!value) {
        skip_group(pp);
    }
}

/* Passes a directive on to the output as one line of text, in column 1 of
 * the line of its `#` (PP_DIRECTIVE); the text is len bytes. */
static void pass_on(struct pp *pp, struct pptok *hash, const char *text, uint len) {
    struct ptok t;
    memset(&t, 0, sizeof t);
    t.t.kind = PP_DIRECTIVE;
    t.t.text = text;
    t.t.len = len;
    t.t.pos = hash->pos;
    t.t.pos.col = 1;
    t.t.end = hash->end;
    t.t.bol = 1;
    push_back(&pp->pending, &t);
}

/* Carries out the directive of the line the innermost file's next token, a
 * `#` at the start of a line, starts. An error drops what is left of it
 * (next_raw reports it). */
static struct diag *directive_line(struct pp *pp, struct pptok *hash, struct pptok *line, uint n) {
    if (n == 0) {
        return NULL; /* `#` alone: the null directive */
    }
    struct pptok *first = &line[0];
    struct pptok *args = line + 1;
    uint nargs = n - 1;
    const char *d = first->kind == PP_IDENT ? first->text : "";
    struct cond *c;
    struct diag *e;
    if (strcmp(d, "define") == 0) {
        return define(pp, first, args, nargs);
    }
    if (strcmp(d, "undef") == 0) {
        return undef(pp, first, args, nargs);
    }
    if (strcmp(d, "include") == 0) {
        return include(pp, first, args, nargs);
    }
    if (strcmp(d, "if") == 0) {
        int v = condition(pp, first, args, nargs);
        open_cond(pp, first->pos, v);
        return NULL;
    }
    if (strcmp(d, "ifdef") == 0 || strcmp(d, "ifndef") == 0) {
        /* Without a name the group is skipped (an #else may follow). */
        const char *name;
        int v = 0;
        e = macro_name(first, args, nargs, 0, &name);
        if (e == NULL) {
            extra_tokens(pp, args + 1, nargs - 1, first->text, 0);
            v = (map_get(&pp->macros, name) != NULL) == (strcmp(d, "ifdef") == 0);
        } else {
            report(pp, e);
        }
        open_cond(pp, first->pos, v);
        return NULL;
    }
    if (strcmp(d, "elif") == 0) {
        /* The group that ends here was taken: the others are skipped, this
         * condition not even evaluated. */
        e = innermost_cond(pp, first, &c);
        if (e != NULL) {
            return e;
        }
        if (c->seen_else) {
            report(pp, diag_error(first->pos, "#elif after #else"));
        }
        skip_group(pp);
        return NULL;
    }
    if (strcmp(d, "else") == 0) {
        extra_tokens(pp, args, nargs, "else", 0);
        e = innermost_cond(pp, first, &c);
        if (e != NULL) {
            return e;
        }
        int seen = c->seen_else;
        c->seen_else = 1;
        if (seen) {
            report(pp, diag_error(first->pos, "#else after #else"));
        }
        skip_group(pp);
        return NULL;
    }
    if (strcmp(d, "endif") == 0) {
        extra_tokens(pp, args, nargs, "endif", 0);
        e = innermost_cond(pp, first, &c);
        if (e != NULL) {
            return e;
        }
        pp->nconds--;
        return NULL;
    }
    if (strcmp(d, "error") == 0 || strcmp(d, "warning") == 0) {
        uint len;
        const char *msg = raw_rest(pp, first, &len);
        if (d[0] == 'e') {
            return diag_error_n(first->pos, msg, len);
        }
        report(pp, diag_warning_n(first->pos, msg, len, "#warnings"));
        return NULL;
    }
    if (strcmp(d, "pragma") == 0) {
        if (nargs == 1 && args[0].kind == PP_IDENT && strcmp(args[0].text, "once") == 0) {
            uint file = top(pp)->file;
            /* Of the file being compiled it means nothing: clang warns, at
             * `once`. */
            if (pp->nstack == 1) {
                const char *msg = "#pragma once in main file";
                report(pp, diag_warning(args[0].pos, msg, "pragma-once-outside-header"));
            }
            map_put(&pp->once, pp->files[file].path, (void *)1);
            return NULL;
        }
        /* Any other means nothing to cc1 (C says to ignore it), and goes on
         * as clang passes it on: `#pragma`, its tokens unreplaced, one space
         * where the source had any. */
        struct buf b;
        memset(&b, 0, sizeof b);
        buf_str(&b, "#pragma");
        for (uint i = 0; i < nargs; i++) {
            if (i == 0 || args[i].space) {
                buf_byte(&b, ' ');
            }
            buf_put(&b, args[i].text, args[i].len);
        }
        pass_on(pp, hash, buf_arena(&b), b.len);
        buf_free(&b);
        return NULL;
    }
    if (strcmp(d, "ident") == 0 || strcmp(d, "sccs") == 0) {
        /* `#ident "string"` (macros replaced; anything after the string is
         * ignored), passed on as #ident in both spellings, as clang does. */
        uint m;
        struct pptok *t = expand_line(pp, args, nargs, &m);
        if (m > 0 && t[0].kind == PP_STR) {
            uint len;
            const char *text = fmtn(&len, "#ident %b", t[0].text, t[0].len);
            pass_on(pp, hash, text, len);
            heap_free(t);
            return NULL;
        }
        e = diag_error(m > 0 ? t[0].pos : first->end, "invalid #ident directive");
        heap_free(t);
        return e;
    }
    if (strcmp(d, "line") == 0) {
        return hash_line(pp, first, args, nargs);
    }
    if (first->kind == PP_NUMBER) {
        /* `# 33 "file"`: GNU's line marker, what -E writes (no directive in
         * assembly, see at_directive). */
        return line_marker(pp, first, args, nargs);
    }
    return diag_error(first->pos, "invalid preprocessing directive");
}

static struct diag *directive(struct pp *pp) {
    struct pptok hash = take_file(pp);
    uint n;
    struct pptok *line = rest_of_line(pp, &n);
    struct diag *e = directive_line(pp, &hash, line, n);
    heap_free(line);
    return e;
}

/* ---- Writing the tokens out (-E) ---- */

/* Whether a and b, written with nothing between them, would not read back
 * as the same two tokens (`-` `-` is `--`, `1` `x` is `1x`, `/` `/` starts
 * a comment): merlin's merges, for spell. */
static int merges(struct pptok *a, struct pptok *b) {
    struct buf text;
    memset(&text, 0, sizeof text);
    buf_put(&text, a->text, a->len);
    buf_put(&text, b->text, b->len);
    struct split s;
    lex_split(text.data, text.len, 0, 3, &s);
    int same = s.ok && s.count == 2 && split_is(&s, 0, a->text, a->len) && split_is(&s, 1, b->text, b->len);
    lex_split_free(&s);
    buf_free(&text);
    return !same;
}

/* C's -E (merlin's spell): a line break where the source had one between
 * two tokens (a macro's expansion stays on the line it was used on), a
 * space where it had whitespace or a comment, and a space where two tokens
 * that were not next to each other in the source would read as something
 * else when written together (`-M` with `#define M -` is `- -`), so that
 * reading the output again gives the same tokens. */
static void spell(struct printer *p, struct ptok *t) {
    if (p->count > 0) {
        struct pptok *prev = &p->prev.t;
        if (prev->pos.file == t->t.pos.file && prev->pos.line == t->t.pos.line) {
            if (t->t.space || merges(prev, &t->t)) {
                buf_byte(p->out, ' ');
            }
        } else {
            buf_byte(p->out, '\n');
        }
    }
    buf_put(p->out, t->t.text, t->t.len);
    p->prev = *t;
    p->count++;
}

/* `# N "file"` and `extra` (` 1` entering a file, ` 2` leaving one), on a
 * line of its own: the next line is line `line`. Without markers (-P),
 * only the newline that ends a line with tokens. */
static void printer_marker(struct pp *pp, struct printer *p, uint line, const char *extra) {
    if (p->started) {
        buf_byte(p->out, '\n');
    }
    p->started = 0;
    if (!p->markers) {
        return;
    }
    struct file *f = &pp->files[p->file];
    buf_fmt(p->out, "# %u \"", line);
    escape_into(p->out, f->name, f->namelen);
    buf_fmt(p->out, "\"%s\n", extra);
}

/* Goes to line `line` of the current file (clang's MoveToLine): one newline
 * to the next line, newlines up to 8 lines ahead, else a line marker;
 * without markers (-P), one newline if the current line has tokens, else
 * none. Whether the output is now at the start of a line. */
static int move_to_line(struct pp *pp, struct printer *p, uint line) {
    int moved;
    if (p->line == line) {
        moved = 0;
    } else if (line == p->line + 1) {
        buf_byte(p->out, '\n');
        moved = 1;
    } else if (!p->markers) {
        if (p->started) {
            buf_byte(p->out, '\n');
        }
        moved = p->started;
    } else {
        if (line > p->line && line - p->line <= 8) {
            for (uint k = p->line; k < line; k++) {
                buf_byte(p->out, '\n');
            }
        } else {
            printer_marker(pp, p, line, "");
        }
        moved = 1;
    }
    if (moved) {
        p->started = 0;
    }
    p->line = line;
    return moved;
}

static void printer_mark(struct pp *pp, struct printer *p, struct pmark *m) {
    if (m->kind == MARK_ENTER) {
        move_to_line(pp, p, m->line);
        p->file = m->file;
        p->line = 1;
        printer_marker(pp, p, 1, " 1");
        p->entered = 1;
    } else {
        p->file = m->file;
        p->line = m->line;
        printer_marker(pp, p, m->line, m->kind == MARK_EXIT ? " 2" : "");
        p->entered = 0;
    }
}

/* Whether tok needs a space after prev (and prev_prev before it), though
 * the source had none: clang's TokenConcatenation::AvoidConcat, for C.
 * Never for two tokens written next to each other (in the source or in one
 * macro's body), which read as two already; otherwise when writing them
 * together could read as other tokens, judged by prev's kind and the first
 * character of tok, as clang judges it. */
static int avoid_concat(struct ptok *prev_prev, struct ptok *prev, struct ptok *tok) {
    if (prev->has_spelled && tok->has_spelled && pos_eq(prev->spelled_to, tok->spelled_from)) {
        return 0;
    }
    uint first = tok->t.len > 0 ? (uchar)tok->t.text[0] : ' ';
    const char *p = prev->t.kind == PP_PUNCT ? prev->t.text : "";
    const char *t = tok->t.kind == PP_PUNCT ? tok->t.text : "";
    /* `&=`, `+=`, `==`, `<<=`...: an operator then `=` or `==`. */
    static const char *const assigning[] = {"&", "+", "-", "/",  "<",  ">", "|",
                                            "%", "*", "!", "<<", ">>", "^", "="};
    if (strcmp(t, "=") == 0 || strcmp(t, "==") == 0) {
        for (uint i = 0; i < 14; i++) {
            if (strcmp(p, assigning[i]) == 0) {
                return 1;
            }
        }
    }
    if (prev->t.kind == PP_IDENT) {
        if (tok->t.kind == PP_NUMBER) {
            return first != '.';
        }
        if (tok->t.kind == PP_IDENT) {
            return 1;
        }
        if (tok->t.kind == PP_STR || tok->t.kind == PP_CHAR) {
            /* L"x", u8"x": a string's prefix. */
            const char *s = prev->t.text;
            return strcmp(s, "L") == 0 || strcmp(s, "u") == 0 || strcmp(s, "U") == 0 || strcmp(s, "u8") == 0;
        }
        return 0;
    }
    if (prev->t.kind == PP_NUMBER) {
        int alnum = (first >= '0' && first <= '9') || (first >= 'a' && first <= 'z') ||
                    (first >= 'A' && first <= 'Z');
        return alnum || first == '_' || first == '.' || first == '+' || first == '-';
    }
    if (prev->t.kind != PP_PUNCT) {
        return 0;
    }
    if (strcmp(p, ".") == 0) {
        return (first == '.' && prev_prev != NULL && tok_is_punct(&prev_prev->t, ".")) ||
               (first >= '0' && first <= '9');
    }
    if (strcmp(p, "&") == 0) {
        return first == '&';
    }
    if (strcmp(p, "+") == 0) {
        return first == '+';
    }
    if (strcmp(p, "-") == 0) {
        return first == '-' || first == '>';
    }
    if (strcmp(p, "/") == 0) {
        return first == '*' || first == '/';
    }
    if (strcmp(p, "<") == 0) {
        return first == '<' || first == ':' || first == '%';
    }
    if (strcmp(p, ">") == 0) {
        return first == '>';
    }
    if (strcmp(p, "|") == 0) {
        return first == '|';
    }
    if (strcmp(p, "%") == 0) {
        return first == '>' || first == ':';
    }
    if (strcmp(p, ":") == 0) {
        return first == '>';
    }
    if (strcmp(p, "#") == 0) {
        return first == '#' || first == '@' || first == '%';
    }
    return 0;
}

/* Preprocessed assembly as clang's -E writes it (merlin's print_asm), one
 * token at a time: see that function's comment for the rules. Each token
 * is shown at its own line and column, or where the outermost macro that
 * produced it was used. */
static void print_asm(struct pp *pp, struct printer *p, struct ptok *t) {
    struct pos at = t->has_site ? t->site : t->t.pos;
    /* A token that starts a line is indented if the output moved to a new
     * line for it, or if it is on the first line of a file that was just
     * entered (clang's IsFirstInFile). */
    int new_line = 0;
    if (t->t.bol) {
        int moved = move_to_line(pp, p, at.line);
        int first_in_file = p->entered && at.line == 1;
        p->entered = 0;
        new_line = moved || first_in_file;
    }
    if (new_line) {
        uint col = at.col;
        if (col == 1 && t->lead) {
            col = 2;
        }
        if (col <= 1 && tok_is_punct(&t->t, "#")) {
            buf_byte(p->out, ' ');
        }
        for (uint k = 1; k < col; k++) {
            buf_byte(p->out, ' ');
        }
    } else if (t->lead || (p->started && p->count > 0 &&
                           avoid_concat(p->count > 1 ? &p->prev_prev : NULL, &p->prev, t))) {
        buf_byte(p->out, ' ');
    }
    buf_put(p->out, t->t.text, t->t.len);
    p->started = 1;
    p->entered = 0;
    p->prev_prev = p->prev;
    p->prev = *t;
    p->count++;
}

/* A token comes out: the line markers before it, then it. */
static void emit(struct pp *pp, struct ptok *t) {
    if (pp->asm) {
        for (uint i = 0; i < pp->nmarks; i++) {
            printer_mark(pp, &pp->print, &pp->marks[i]);
        }
        print_asm(pp, &pp->print, t);
    } else {
        spell(&pp->print, t);
    }
    pp->nmarks = 0;
}

/* ---- The translation unit ---- */

/* Reads a made-up file holding only directives (the predefined macros, the
 * command line's); its number in *id. */
static struct diag *read_directives(struct pp *pp, const char *name, const char *text, uint *id) {
    *id = new_file(pp, name, (uint)strlen(name), name, text, (uint)strlen(text));
    struct lexed lx;
    lex_pp(text, (uint)strlen(text), *id, 0, &lx);
    if (lx.error != NULL) {
        heap_free(lx.toks);
        return lx.error;
    }
    enter(pp, &lx, text, (uint)strlen(text), 0, *id);
    struct ptok t;
    int got;
    struct diag *e = pp_next(pp, &t, &got);
    if (e != NULL) {
        return e;
    }
    if (got) {
        return diag_error(t.t.pos, "expected a directive");
    }
    return NULL;
}

/* merlin's Pp::run: the predefined macros, then -D and -U, as directives
 * of their own files, then the file, every token out as it comes. */
static struct diag *run(struct pp *pp, const char *src, uint n) {
    struct buf b;
    memset(&b, 0, sizeof b);
    const struct predefined *pre = pp->opts->lang == LANG_ASM ? PREDEFINED_ASM : PREDEFINED_C;
    for (uint i = 0; pre[i].name != NULL; i++) {
        buf_fmt(&b, "#define %s %s\n", pre[i].name, pre[i].value);
    }
    buf_str(&b, "#define __FILE__\n#define __LINE__\n");
    struct diag *e = read_directives(pp, "<built-in>", buf_arena(&b), &pp->builtin_file);
    buf_free(&b);
    if (e != NULL) {
        return e;
    }
    ((struct macro *)map_get(&pp->macros, "__FILE__"))->kind = MACRO_FILE;
    ((struct macro *)map_get(&pp->macros, "__LINE__"))->kind = MACRO_LINE;
    for (uint i = 0; i < pp->opts->nmacros; i++) {
        struct cmd_macro *m = &pp->opts->macros[i];
        if (m->undef) {
            buf_fmt(&b, "#undef %s\n", m->text);
            continue;
        }
        /* -D NAME=VALUE is #define NAME VALUE, -D NAME #define NAME 1. */
        const char *eq = strchr(m->text, '=');
        if (eq != NULL) {
            buf_fmt(&b, "#define %s %s\n", arena_strndup(m->text, (uint)(eq - m->text)), eq + 1);
        } else {
            buf_fmt(&b, "#define %s 1\n", m->text);
        }
    }
    if (b.len > 0) {
        uint id;
        e = read_directives(pp, "<command line>", buf_arena(&b), &id);
        buf_free(&b);
        if (e != NULL) {
            return e;
        }
    }
    struct lexed lx;
    lex_pp(src, n, 0, pp->asm, &lx);
    if (lx.error != NULL) {
        heap_free(lx.toks);
        return lx.error;
    }
    pp->end = lx.end;
    enter(pp, &lx, src, n, 0, 0);
    for (;;) {
        struct ptok t;
        int got;
        e = pp_next(pp, &t, &got);
        if (e != NULL) {
            return e;
        }
        if (!got) {
            break;
        }
        pp->last_out = t.t.pos;
        if (pp->collect) {
            pp->out = grow(pp->out, &pp->capout, pp->nout + 1, sizeof(struct pptok));
            pp->out[pp->nout++] = t.t;
            if (t.has_site) {
                posmap_put(&pp->macro_sites, t.t.pos, NULL);
            }
        } else {
            emit(pp, &t);
        }
        pp->emitted++;
    }
    return NULL;
}

/* Frees what the preprocessor keeps on the heap, but its diagnostics' array, which
 * preprocess_text hands on: the files still open when an error stopped it,
 * their tokens and texts, and its tables. */
static void pp_free(struct pp *pp) {
    while (pp->nstack > 0) {
        struct cursor *c = top(pp);
        heap_free(c->toks);
        if (c->owned) {
            heap_free((char *)c->text);
        }
        pp->nstack--;
    }
    heap_free(pp->stack);
    heap_free(pp->files);
    heap_free(pp->pending.buf);
    heap_free(pp->conds);
    heap_free(pp->marks);
    heap_free(pp->after);
    map_free(&pp->macros);
    map_free(&pp->once);
    map_free(&pp->guards);
    heap_free(pp);
}

/* The files' names and their lengths, by number, for naming diagnostics:
 * in out (on the heap). */
static void file_names(struct pp *pp, struct pp_output *out) {
    out->files = heap_alloc((pp->nfiles + 1) * sizeof(char *));
    out->lens = heap_alloc((pp->nfiles + 1) * sizeof(uint));
    out->sums = heap_alloc((pp->nfiles + 1) * sizeof(struct checksum));
    for (uint i = 0; i < pp->nfiles; i++) {
        out->files[i] = pp->files[i].name;
        out->lens[i] = pp->files[i].namelen;
        out->sums[i] = pp->files[i].sum;
    }
    out->nfiles = pp->nfiles;
}

/* Preprocesses the file `name` to compile it (merlin's preprocess_recovering):
 * 1 and the translation unit's tokens with their diagnostics, errors among
 * them, in *out; or 0 if what stops clang stopped it (a header not found,
 * a lexical error in the file being compiled), out->diags then having the
 * diagnostics before it and that one. Diagnostics in headers name them.
 * What *out holds is the translation unit's, and the preprocessor's own
 * tables are freed. */
int preprocess_unit(const char *name, const char *src, uint n, struct options *opts, struct pp_output *out) {
    struct pp *pp = heap_calloc(1, sizeof(struct pp));
    pp->opts = opts;
    pp->asm = opts->lang == LANG_ASM;
    pp->collect = 1;
    new_file(pp, name, (uint)strlen(name), normalize(name), src, n);
    struct diag *e = run(pp, src, n);
    memset(out, 0, sizeof *out);
    file_names(pp, out);
    for (uint i = 0; i < pp->ndiags; i++) {
        diag_name_files(pp->diags[i], out->files, out->lens, out->nfiles);
    }
    if (e != NULL) {
        diag_name_files(e, out->files, out->lens, out->nfiles);
        report(pp, e);
    }
    /* The diagnostics and where each came are handed on, not freed with
     * the preprocessor. */
    out->diags = pp->diags;
    out->after = pp->after;
    out->ndiags = pp->ndiags;
    pp->after = NULL;
    int ok = e == NULL;
    if (ok) {
        out->toks = pp->out;
        out->ntoks = pp->nout;
        out->macros = pp->macro_sites;
        out->end = pp->end;
    } else {
        heap_free(pp->out);
    }
    pp_free(pp);
    return ok;
}

int preprocess_text(const char *name, const char *src, uint n, struct options *opts, struct buf *out,
                    struct diag ***diags, uint *ndiags) {
    struct pp *pp = heap_calloc(1, sizeof(struct pp));
    pp->opts = opts;
    pp->asm = opts->lang == LANG_ASM;
    new_file(pp, name, (uint)strlen(name), normalize(name), src, n);
    pp->print.out = out;
    pp->print.line = 1;
    pp->print.entered = 1;
    pp->print.markers = opts->line_markers;
    if (pp->asm) {
        printer_marker(pp, &pp->print, 1, "");
    }
    struct diag *e = run(pp, src, n);
    if (e != NULL) {
        report(pp, e);
    }
    /* The rest of the output: the line markers after the last token, and
     * the newline clang ends it with whatever came last (after a marker, or
     * nothing, an empty line); C's ends its last line. */
    if (pp->asm) {
        for (uint i = 0; i < pp->nmarks; i++) {
            printer_mark(pp, &pp->print, &pp->marks[i]);
        }
        buf_byte(out, '\n');
    } else if (out->len > 0) {
        buf_byte(out, '\n');
    }
    /* Diagnostics in headers name them. */
    const char **names = heap_alloc((pp->nfiles + 1) * sizeof(char *));
    uint *lens = heap_alloc((pp->nfiles + 1) * sizeof(uint));
    for (uint i = 0; i < pp->nfiles; i++) {
        names[i] = pp->files[i].name;
        lens[i] = pp->files[i].namelen;
    }
    int ok = e == NULL;
    for (uint i = 0; i < pp->ndiags; i++) {
        diag_name_files(pp->diags[i], names, lens, pp->nfiles);
        if (!diag_is_warning(pp->diags[i])) {
            ok = 0;
        }
    }
    heap_free(names);
    heap_free(lens);
    *diags = pp->diags;
    *ndiags = pp->ndiags;
    /* A successful preprocessing of assembly gives no warnings: merlin -E
     * prints none for it (preprocess_asm hands on only the text). */
    if (ok && pp->asm) {
        *ndiags = 0;
    }
    pp_free(pp);
    return ok;
}
