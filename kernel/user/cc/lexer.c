/* cc1: C source text to preprocessing tokens, and the literals the
 * preprocessor reads (follows merlin's lexer.rs).
 *
 * As C99's translation phases go: phases 1 and 2 (splice) replace
 * trigraphs (`??=` for `#`; in C only, as clang's C99 mode does: assembly
 * keeps them as characters) and delete line splices (a backslash at the
 * end of a line, blanks allowed before the newline, with clang's warning);
 * phase 3 (lex_pp) splits what is left into preprocessing tokens:
 * identifiers, preprocessing numbers, character constants and string
 * literals as written, punctuators. Comments count as spaces. Each token
 * knows whether it starts a line and whether a space comes before it,
 * which the preprocessor needs, and carries the lexer's warnings of what
 * came before it (a trigraph, a splice's blanks, a comment that seems to
 * open inside a block comment).
 *
 * merlin works on Rust's chars, Unicode scalar values; this works on the
 * UTF-8 bytes, which comes to the same for everything the lexer compares
 * (only ASCII means anything to C), with three places where it would not:
 * a step over "one character" (bump) takes all its bytes, whitespace past
 * ASCII (U+00A0, U+3000...) is whitespace, as Rust's is_whitespace says,
 * and a token's end is just past the first byte of its last character
 * (merlin's is the position of that character, plus one).
 *
 * Positions: every byte left after phase 2 remembers where it was in the
 * file (its offset), and a table of where each line starts gives the line
 * and the column of an offset. Columns count bytes, as clang's do, a
 * UTF-8 byte-order mark's three included. */
#include "cc.h"

/* ---- Phases 1 and 2 ---- */

/* Something phases 1 and 2 did that clang warns of, but in a comment
 * (merlin's Mark), before the character at index `at`. */
enum { MARK_TRIGRAPH, MARK_BLANKS };

struct mark {
    uint at;
    int kind;
    /* MARK_TRIGRAPH: where it is, and the character it became. */
    struct pos pos;
    uint to;
    /* MARK_BLANKS: where the backslash is, and the first blank after it:
     * clang warns at the backslash in a `//` comment (and not in a block
     * one), elsewhere at the first blank. */
    struct pos backslash;
    struct pos first;
};

/* Where the lexer is, for the warnings of the marks. */
enum { CTX_CODE, CTX_LINE_COMMENT, CTX_BLOCK_COMMENT };

struct lexer {
    uint file;
    /* The bytes after phase 2, and each one's offset in the file. */
    uchar *chars;
    uint *off;
    uint n;
    uint cap;
    /* Where each line starts in the file. */
    uint *lines;
    uint nlines;
    uint caplines;
    struct mark *marks;
    uint nmarks;
    uint capmarks;
    /* Just past the last character. */
    struct pos end;
    /* The next byte. */
    uint i;
    /* A newline, and whitespace or a comment, came since the last token. */
    int newline;
    int space;
    /* The next of marks to look at, and where the lexer is. */
    uint mark;
    int context;
    /* The warnings for the next token. */
    struct diag *lexed;
};

/* What C99 replaces the trigraph `??c` by, or 0. */
static uint trigraph(uint c) {
    switch (c) {
    case '=':
        return '#';
    case '(':
        return '[';
    case '/':
        return '\\';
    case ')':
        return ']';
    case '\'':
        return '^';
    case '<':
        return '{';
    case '!':
        return '|';
    case '>':
        return '}';
    case '-':
        return '~';
    }
    return 0;
}

/* The line and column of offset `off` of the file. */
static struct pos pos_of(struct lexer *lx, uint off) {
    /* The last line that starts at or before off. */
    uint lo = 0;
    uint hi = lx->nlines;
    while (hi - lo > 1) {
        uint mid = lo + (hi - lo) / 2;
        if (lx->lines[mid] <= off) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    return pos_at(lx->file, lo + 1, off - lx->lines[lo] + 1);
}

static void new_line(struct lexer *lx, uint start) {
    lx->lines = grow(lx->lines, &lx->caplines, lx->nlines + 1, sizeof(uint));
    lx->lines[lx->nlines++] = start;
}

static void add_mark(struct lexer *lx, struct mark *m) {
    lx->marks = grow(lx->marks, &lx->capmarks, lx->nmarks + 1, sizeof(struct mark));
    lx->marks[lx->nmarks++] = *m;
}

/* Replaces trigraphs (unless asm) and deletes line splices (merlin's
 * splice). Blanks between the backslash and the newline are allowed, as
 * clang allows them, with a warning; a `\r` before the newline is a line's
 * end, no blank. */
static void splice(struct lexer *lx, const uchar *src, uint n, int asm) {
    uint line = 1;
    uint col = 1;
    uint k = 0;
    new_line(lx, 0);
    /* A UTF-8 byte-order mark is no character of the source: clang skips
     * it, its three bytes still in the first line's columns. */
    if (n >= 3 && src[0] == 0xef && src[1] == 0xbb && src[2] == 0xbf) {
        k = 3;
        col = 4;
    }
    lx->chars = malloc(n + 1);
    lx->off = malloc((n + 1) * sizeof(uint));
    if (lx->chars == NULL || lx->off == NULL) {
        die("out of memory");
    }
    while (k < n) {
        /* The character here after phase 1, and how many bytes it was
         * written as. */
        uint c = src[k];
        uint len = 1;
        if (!asm && c == '?' && k + 2 < n && src[k + 1] == '?' && trigraph(src[k + 2]) != 0) {
            c = trigraph(src[k + 2]);
            len = 3;
        }
        struct mark m;
        memset(&m, 0, sizeof m);
        m.at = lx->n;
        if (c == '\\') {
            uint j = k + len;
            while (j < n && (src[j] == ' ' || src[j] == '\t' || src[j] == '\r')) {
                j++;
            }
            if (j < n && src[j] == '\n') {
                if (len == 3) {
                    m.kind = MARK_TRIGRAPH;
                    m.pos = pos_at(lx->file, line, col);
                    m.to = c;
                    add_mark(lx, &m);
                }
                int blanks = 0;
                for (uint b = k + len; b < j; b++) {
                    if (src[b] != '\r') {
                        blanks = 1;
                    }
                }
                if (blanks) {
                    m.kind = MARK_BLANKS;
                    m.backslash = pos_at(lx->file, line, col + len - 1);
                    m.first = pos_at(lx->file, line, col + len);
                    add_mark(lx, &m);
                }
                line++;
                col = 1;
                k = j + 1;
                new_line(lx, k);
                continue;
            }
        }
        if (len == 3) {
            m.kind = MARK_TRIGRAPH;
            m.pos = pos_at(lx->file, line, col);
            m.to = c;
            add_mark(lx, &m);
        }
        lx->chars[lx->n] = (uchar)c;
        lx->off[lx->n] = k;
        lx->n++;
        if (c == '\n') {
            line++;
            col = 1;
            new_line(lx, k + 1);
        } else {
            col += len;
        }
        k += len;
    }
    lx->chars[lx->n] = 0;
    /* The end of a file that ends with a newline is that newline, as clang
     * has it: no line follows it. */
    if (lx->n > 0 && lx->chars[lx->n - 1] == '\n') {
        lx->end = pos_of(lx, lx->off[lx->n - 1]);
    } else {
        lx->end = pos_at(lx->file, line, col);
    }
}

/* ---- Phase 3 ---- */

/* The byte n after the next one, or 0 past the end (no C character is 0
 * that matters here: a NUL in the source is a stray character). */
static uint peek(struct lexer *lx, uint n) {
    return lx->i + n < lx->n ? lx->chars[lx->i + n] : 0;
}

static int at_end(struct lexer *lx) {
    return lx->i >= lx->n;
}

/* Whether the input continues with s. */
static int at(struct lexer *lx, const char *s) {
    for (uint k = 0; s[k]; k++) {
        if (lx->i + k >= lx->n || lx->chars[lx->i + k] != (uchar)s[k]) {
            return 0;
        }
    }
    return 1;
}

static struct pos here(struct lexer *lx) {
    return lx->i < lx->n ? pos_of(lx, lx->off[lx->i]) : lx->end;
}

static const char BLANKS[] = "backslash and newline separated by space";

/* clang's warnings of the marks before index `end`, as where the lexer is
 * has them: none for a trigraph in a comment, none for a splice's blanks
 * in a block comment. */
static void marks_before(struct lexer *lx, uint end) {
    while (lx->mark < lx->nmarks && lx->marks[lx->mark].at < end) {
        struct mark *m = &lx->marks[lx->mark++];
        struct diag *w = NULL;
        if (m->kind == MARK_TRIGRAPH && lx->context == CTX_CODE) {
            w = diag_warning(m->pos, fmt("trigraph converted to '%c' character", m->to), "trigraphs");
        } else if (m->kind == MARK_BLANKS && lx->context == CTX_LINE_COMMENT) {
            w = diag_warning(m->backslash, BLANKS, "backslash-newline-escape");
        } else if (m->kind == MARK_BLANKS && lx->context == CTX_CODE) {
            w = diag_warning(m->first, BLANKS, "backslash-newline-escape");
        }
        if (w != NULL) {
            diag_append(&lx->lexed, w);
        }
    }
}

/* Takes one character: all its bytes. */
static void bump(struct lexer *lx) {
    if (at_end(lx)) {
        return;
    }
    uint len = utf8_len(lx->chars[lx->i]);
    if (lx->i + len > lx->n) {
        len = lx->n - lx->i;
    }
    marks_before(lx, lx->i + len);
    lx->i += len;
}

/* Whether the character at the next byte is whitespace, as Rust's
 * char::is_whitespace says (Unicode's White_Space): its length, or 0. */
static uint whitespace(struct lexer *lx) {
    uint c = peek(lx, 0);
    if (c == ' ' || (c >= '\t' && c <= '\r')) {
        return 1;
    }
    if (c < 0x80 || at_end(lx)) {
        return 0;
    }
    uint len;
    uint v = utf8_decode(lx->chars + lx->i, lx->n - lx->i, &len);
    if (v == 0x85 || v == 0xa0 || v == 0x1680 || (v >= 0x2000 && v <= 0x200a) || v == 0x2028 ||
        v == 0x2029 || v == 0x202f || v == 0x205f || v == 0x3000) {
        return len;
    }
    return 0;
}

/* Skips whitespace and comments, noting newlines and spaces. An
 * unterminated block comment is the only error of the lexer. */
static struct diag *skip_trivia(struct lexer *lx) {
    for (;;) {
        if (whitespace(lx)) {
            if (peek(lx, 0) == '\n') {
                lx->newline = 1;
            }
            lx->space = 1;
            bump(lx);
        } else if (at(lx, "//")) {
            lx->space = 1;
            lx->context = CTX_LINE_COMMENT;
            while (!at_end(lx) && peek(lx, 0) != '\n') {
                bump(lx);
            }
            lx->context = CTX_CODE;
        } else if (at(lx, "/*")) {
            lx->space = 1;
            struct pos start = here(lx);
            lx->context = CTX_BLOCK_COMMENT;
            bump(lx);
            bump(lx);
            for (;;) {
                if (at(lx, "*/")) {
                    bump(lx);
                    bump(lx);
                    break;
                }
                if (at(lx, "/*")) {
                    /* A comment does not nest: clang warns of what looks like
                     * a second one starting. */
                    diag_append(&lx->lexed, diag_warning(here(lx), "'/*' within block comment", "comment"));
                    bump(lx);
                } else if (!at_end(lx)) {
                    bump(lx);
                } else {
                    return diag_error(start, "unterminated /* comment");
                }
            }
            lx->context = CTX_CODE;
        } else {
            return NULL;
        }
    }
}

static int is_digit(uint c) {
    return c >= '0' && c <= '9';
}

static int is_alpha(uint c) {
    return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z');
}

static int is_alnum(uint c) {
    return is_alpha(c) || is_digit(c);
}

/* A preprocessing number, from its first character: then any letters,
 * digits, `_`, `.` and `e+ e- E+ E- p+ p- P+ P-`. */
static void number(struct lexer *lx) {
    bump(lx);
    while (!at_end(lx)) {
        uint c = peek(lx, 0);
        if ((c == 'e' || c == 'E' || c == 'p' || c == 'P') && (peek(lx, 1) == '+' || peek(lx, 1) == '-')) {
            bump(lx);
            bump(lx);
        } else if (is_alnum(c) || c == '_' || c == '.') {
            bump(lx);
        } else {
            break;
        }
    }
}

/* A character constant or string literal, from its opening quote, up to the
 * closing one (escapes are decoded later). Without the closing quote on
 * the same line it is an invalid token running to the end of the line,
 * and so is `''`: *msg says what clang warns of it. */
static int quoted(struct lexer *lx, uint quote, const char **msg) {
    bump(lx);
    if (quote == '\'' && peek(lx, 0) == '\'' && !at_end(lx)) {
        bump(lx);
        *msg = "empty character constant";
        return PP_INVALID;
    }
    for (;;) {
        if (at_end(lx) || peek(lx, 0) == '\n') {
            *msg = quote == '"' ? "missing terminating '\"' character" : "missing terminating ' character";
            return PP_INVALID;
        }
        uint c = peek(lx, 0);
        bump(lx);
        if (c == quote) {
            return quote == '"' ? PP_STR : PP_CHAR;
        }
        if (c == '\\' && !at_end(lx) && peek(lx, 0) != '\n') {
            bump(lx);
        }
    }
}

/* One token, or NULL at the end; *err for an error. Its text is left as
 * the index of its first byte in lx->chars (start) and its length. */
static int next_token(struct lexer *lx, struct pptok *t, uint *start, struct diag **err) {
    *err = skip_trivia(lx);
    if (*err != NULL) {
        return 0;
    }
    memset(t, 0, sizeof *t);
    t->pos = here(lx);
    if (at_end(lx)) {
        return 0;
    }
    *start = lx->i;
    uint c = peek(lx, 0);
    if (is_digit(c) || (c == '.' && is_digit(peek(lx, 1)))) {
        number(lx);
        t->kind = PP_NUMBER;
    } else if (c == '\'' || c == '"') {
        t->kind = (uchar)quoted(lx, c, &t->msg);
    } else if (is_alpha(c) || c == '_') {
        while (!at_end(lx) && (is_alnum(peek(lx, 0)) || peek(lx, 0) == '_')) {
            bump(lx);
        }
        t->kind = PP_IDENT;
    } else {
        /* Punctuators, longest first. */
        uint p = 0;
        while (p < NPUNCTS && !at(lx, PUNCTS[p])) {
            p++;
        }
        if (p < NPUNCTS) {
            for (uint k = 0; PUNCTS[p][k]; k++) {
                bump(lx);
            }
            t->kind = PP_PUNCT;
            t->punct = (uchar)p;
        } else {
            bump(lx);
            t->kind = PP_INVALID;
        }
    }
    t->len = lx->i - *start;
    /* Just past the last character: the position of its first byte, plus
     * one; after a splice inside the token, still on the line where it
     * ends. */
    uint last = lx->i - 1;
    while (last > *start && (lx->chars[last] & 0xc0) == 0x80) {
        last--;
    }
    struct pos end = pos_of(lx, lx->off[last]);
    end.col++;
    t->end = end;
    t->bol = (uchar)lx->newline;
    t->space = (uchar)lx->space;
    t->lexed = lx->lexed;
    lx->newline = 0;
    lx->space = 0;
    lx->lexed = NULL;
    return 1;
}

static void lexer_start(struct lexer *lx, const char *src, uint n, uint file, int asm) {
    memset(lx, 0, sizeof *lx);
    lx->file = file;
    lx->newline = 1;
    lx->context = CTX_CODE;
    splice(lx, (const uchar *)src, n, asm);
}

static void lexer_free(struct lexer *lx) {
    free(lx->chars);
    free(lx->off);
    free(lx->lines);
    free(lx->marks);
}

void lex_pp(const char *src, uint n, uint file, int asm, struct lexed *out) {
    struct lexer lx;
    lexer_start(&lx, src, n, file, asm);
    memset(out, 0, sizeof *out);
    uint cap = 0;
    for (;;) {
        struct pptok t;
        uint start = 0;
        struct diag *err;
        if (!next_token(&lx, &t, &start, &err)) {
            if (err != NULL) {
                out->error = err;
                break;
            }
            /* What comes after the last token goes with it. */
            marks_before(&lx, 0xffffffffu);
            if (out->n > 0) {
                diag_append(&out->toks[out->n - 1].lexed, lx.lexed);
            }
            out->end = lx.end;
            break;
        }
        const char *text = (const char *)lx.chars + start;
        if (t.kind == PP_IDENT) {
            t.text = intern(text, t.len);
        } else if (t.kind == PP_PUNCT) {
            t.text = PUNCTS[t.punct];
        } else {
            t.text = arena_strndup(text, t.len);
        }
        out->toks = grow(out->toks, &cap, out->n + 1, sizeof(struct pptok));
        out->toks[out->n++] = t;
    }
    lexer_free(&lx);
}

int lex_split(const char *text, uint n, int asm, uint max, struct split *s) {
    struct lexer lx;
    lexer_start(&lx, text, n, 0, asm);
    memset(s, 0, sizeof *s);
    s->ok = 1;
    while (s->count < max) {
        struct pptok t;
        uint start = 0;
        struct diag *err;
        if (!next_token(&lx, &t, &start, &err)) {
            s->ok = err == NULL;
            break;
        }
        s->kind[s->count] = t.kind;
        s->punct[s->count] = t.punct;
        s->start[s->count] = start;
        s->len[s->count] = t.len;
        s->count++;
    }
    /* The bytes the tokens are in: the caller's to free (lex_split_free). */
    s->chars = lx.chars;
    lx.chars = NULL;
    lexer_free(&lx);
    return s->ok;
}

void lex_split_free(struct split *s) {
    free(s->chars);
    s->chars = NULL;
}

/* Whether token k of s is the n bytes at text. */
int split_is(struct split *s, uint k, const char *text, uint n) {
    return k < s->count && s->len[k] == n && memcmp(s->chars + s->start[k], text, n) == 0;
}

int tok_is_punct(const struct pptok *t, const char *p) {
    return t->kind == PP_PUNCT && strcmp(PUNCTS[t->punct], p) == 0;
}

const char *tok_ident(const struct pptok *t) {
    return t->kind == PP_IDENT ? t->text : NULL;
}

/* ---- Literals ---- */

static uint lower(uint c) {
    return c >= 'A' && c <= 'Z' ? c + 32 : c;
}

/* The value of digit c in radix, or -1. */
static int digit_value(uint c, uint radix) {
    int v = -1;
    if (c >= '0' && c <= '9') {
        v = (int)(c - '0');
    } else if (lower(c) >= 'a' && lower(c) <= 'z') {
        v = (int)(lower(c) - 'a') + 10;
    }
    return v >= 0 && (uint)v < radix ? v : -1;
}

/* Whether the lower-cased text (len bytes) has c. */
static int has_lower(const char *text, uint len, uint c) {
    for (uint i = 0; i < len; i++) {
        if (lower((uchar)text[i]) == c) {
            return 1;
        }
    }
    return 0;
}

/* Reads an integer constant as clang's NumericLiteralParser does (merlin's
 * lexer::integer): the digits of its radix (`0x` hex, a leading `0` octal),
 * then a suffix of `u` and `l` (`ll` in one case). A digit the radix has
 * not right after the digits is `invalid digit '9' in octal constant` (`a`
 * to `f` in a decimal one too), at the digit; anything else after them is
 * `invalid suffix 'abc' on integer constant`, at the suffix. */
struct diag *lex_integer(const char *text, uint len, struct pos pos, struct intlit *out) {
    memset(out, 0, sizeof *out);
    int hex = len >= 2 && text[0] == '0' && lower((uchar)text[1]) == 'x';
    if (has_lower(text, len, '.') || (!hex && has_lower(text, len, 'e')) || (hex && has_lower(text, len, 'p'))) {
        return diag_error(pos, "floating-point literals are not supported");
    }
    /* The leading `0` of an octal constant is one of its digits (`0` and
     * `0u` are octal). */
    uint radix = 10;
    uint start = 0;
    if (hex) {
        radix = 16;
        start = 2;
    } else if (len > 1 && text[0] == '0') {
        radix = 8;
    }
    uint k = start;
    while (k < len && digit_value((uchar)text[k], radix) >= 0) {
        k++;
    }
    /* An octal constant with an 8 or a 9, or a decimal or octal one with a
     * hex digit right after its digits (not `e`, which would make it
     * floating), has an invalid digit. */
    if (radix != 16 && k < len) {
        uint c = (uchar)text[k];
        if (is_digit(c) || (digit_value(c, 16) >= 0 && lower(c) != 'e')) {
            return diag_error(pos_at(pos.file, pos.line, pos.col + k),
                              fmt("invalid digit '%c' in %s constant", c, radix == 8 ? "octal" : "decimal"));
        }
    }
    uint digits_start = start;
    uint digits_end = k;
    /* `0x` with no digits is the octal `0` with the suffix `x...`. */
    if (hex && k == start) {
        k = 1;
    }
    const char *suffix = text + k;
    /* The suffix, lower-cased, to match. */
    char low[8];
    uint slen = len - k;
    int lng = 3;
    int is_unsigned = 0;
    if (slen < sizeof low && !(hex && digits_end == digits_start)) {
        for (uint j = 0; j < slen; j++) {
            low[j] = (char)lower((uchar)suffix[j]);
        }
        low[slen] = 0;
        if (slen == 0) {
            lng = 0;
        } else if (strcmp(low, "u") == 0) {
            is_unsigned = 1;
            lng = 0;
        } else if (strcmp(low, "l") == 0) {
            lng = 1;
        } else if (strcmp(low, "ul") == 0 || strcmp(low, "lu") == 0) {
            is_unsigned = 1;
            lng = 1;
        } else if ((strcmp(low, "ll") == 0 || strcmp(low, "ull") == 0 || strcmp(low, "llu") == 0) &&
                   (strstr(suffix, "ll") != NULL || strstr(suffix, "LL") != NULL)) {
            /* `ll` is `ll` or `LL`, never `lL` (C99 6.4.4.1). */
            is_unsigned = strchr(suffix, 'u') != NULL || strchr(suffix, 'U') != NULL;
            lng = 2;
        }
    }
    if (lng == 3) {
        return diag_error(pos_at(pos.file, pos.line, pos.col + k),
                          fmt("invalid suffix '%b' on integer constant", suffix, slen));
    }
    /* The digits are valid, so the only way to fail is to overflow. */
    uint64_t low64 = 0;
    uint64_t v = 0;
    int fits = 1;
    for (uint j = digits_start; j < digits_end; j++) {
        uint d = (uint)digit_value((uchar)text[j], radix);
        low64 = low64 * radix + d;
        if (v > (0xffffffffffffffffull - d) / radix) {
            fits = 0;
        }
        v = v * radix + d;
    }
    out->fits = fits;
    out->value = fits ? v : 0;
    out->low = low64;
    out->decimal = radix == 10;
    out->is_unsigned = is_unsigned;
    out->lng = lng;
    return NULL;
}

/* The position of byte `b` of a literal's body (after its opening quote),
 * the literal starting at pos. */
static struct pos body_at(struct pos pos, uint b) {
    return pos_at(pos.file, pos.line, pos.col + 1 + b);
}

/* clang's error for a character constant whose character is more than its
 * one byte. */
static const char TOO_LARGE[] = "character too large for enclosing character literal type";

static int is_hex(uint c) {
    return digit_value(c, 16) >= 0;
}

/* The bytes of a character constant or string literal as written (with its
 * quotes, closed), escapes decoded, with clang's diagnostics for what is
 * wrong in it (merlin's decode_units): those it goes on after in *diags,
 * the one it stops at returned. Characters outside ASCII come out as their
 * UTF-8 bytes; in a character constant (char_const) each is too large for
 * it. *units: how many characters it has, each escape one. */
static struct diag *decode_units(const char *text, uint len, struct pos pos, struct diag **diags,
                                 int char_const, struct buf *out, uint *units) {
    const uchar *body = (const uchar *)text + 1;
    /* Between the quotes (which the lexer made sure of). */
    uint n = len >= 2 ? len - 2 : 0;
    uint k = 0;
    *units = 0;
    while (k < n) {
        uint c = body[k];
        *units += 1;
        if (c != '\\') {
            uint len = utf8_len(c);
            if (k + len > n) {
                len = n - k;
            }
            /* A character past ASCII is more than a character constant's one
             * byte of it: clang's error, the constant's. */
            if (char_const && c >= 0x80) {
                diag_append(diags, diag_error(pos, TOO_LARGE));
                k += len;
                continue;
            }
            buf_put(out, body + k, len);
            k += len;
            continue;
        }
        uint start = k;
        /* The lexer never ends a literal with a lone `\` (it would escape the
         * quote), but a literal made some other way might. */
        if (k + 1 >= n) {
            return diag_error(body_at(pos, start), "'\\' at the end of a literal escapes nothing");
        }
        uint e = body[k + 1];
        uint elen = utf8_len(e);
        if (k + 1 + elen > n) {
            elen = n - k - 1;
        }
        k += 1 + elen;
        uint byte;
        switch (e) {
        case 'n':
            byte = '\n';
            break;
        case 't':
            byte = '\t';
            break;
        case 'r':
            byte = '\r';
            break;
        case 'a':
            byte = 7;
            break;
        case 'b':
            byte = 8;
            break;
        case 'f':
            byte = 12;
            break;
        case 'v':
            byte = 11;
            break;
        case '\\':
        case '\'':
        case '"':
        case '?':
            byte = e;
            break;
        case '0':
        case '1':
        case '2':
        case '3':
        case '4':
        case '5':
        case '6':
        case '7': {
            /* Up to three octal digits. */
            uint v = e - '0';
            for (int j = 0; j < 2 && k < n && body[k] >= '0' && body[k] <= '7'; j++) {
                v = v * 8 + (body[k] - '0');
                k++;
            }
            if (v > 255) {
                return diag_error(body_at(pos, start), "octal escape sequence out of range");
            }
            byte = v;
            break;
        }
        case 'x': {
            /* As many hex digits as follow, as in C. */
            uint digits = k;
            uint64_t v = 0;
            int over = 0;
            while (k < n && is_hex(body[k])) {
                if (v > 0x0fffffffffffffffull) {
                    over = 1;
                }
                v = v * 16 + (uint)digit_value(body[k], 16);
                k++;
            }
            if (k == digits) {
                return diag_error(body_at(pos, start), "\\x used with no following hex digits");
            }
            if (over || v > 255) {
                return diag_error(body_at(pos, start), "hex escape sequence out of range");
            }
            byte = (uint)v;
            break;
        }
        case 'e':
        case 'E':
            /* GNU's escape for ESC, which clang takes without a word. */
            byte = 27;
            break;
        case '(':
        case '{':
        case '[':
        case '%':
            /* clang's other escapes: the character, which its pedantic
             * warning only says C has not. */
            byte = e;
            break;
        case 'u':
        case 'U': {
            /* A universal character name (C99 6.4.3): `\u` and four hex
             * digits, `\U` and eight, the character in UTF-8. */
            uint want = e == 'u' ? 4 : 8;
            uint got = 0;
            uint v = 0;
            while (got < want && k < n && is_hex(body[k])) {
                v = v * 16 + (uint)digit_value(body[k], 16);
                k++;
                got++;
            }
            if (got < want) {
                diag_append(diags, diag_error(body_at(pos, start), "incomplete universal character name"));
                continue;
            }
            /* What C99 lets no universal character name be: a control
             * character, the basic character set but `$`, `@` and the
             * backquote, a surrogate, or what is past Unicode. */
            const char *problem = NULL;
            if ((v >= 0xd800 && v <= 0xdfff) || v > 0x10ffff) {
                problem = "invalid universal character";
            } else if (v < 0x20 || (v >= 0x7f && v <= 0x9f)) {
                problem = "universal character name refers to a control character";
            } else if (v < 0xa0 && v != '$' && v != '@' && v != '`') {
                problem = fmt("character '%c' cannot be specified by a universal character name", v);
            } else if (char_const && v > 0x7f) {
                /* clang's error is the constant's. */
                diag_append(diags, diag_error(pos, TOO_LARGE));
                continue;
            }
            if (problem != NULL) {
                diag_append(diags, diag_error(body_at(pos, start), problem));
            } else {
                utf8_put(out, v);
            }
            continue;
        }
        default: {
            /* clang: the character itself (a NUL too), and a warning. */
            uint mlen;
            const char *msg = fmtn(&mlen, "unknown escape sequence '\\%b'", (const char *)body + start + 1, elen);
            diag_append(diags, diag_warning_n(body_at(pos, start), msg, mlen, "unknown-escape-sequence"));
            buf_put(out, body + start + 1, elen);
            continue;
        }
        }
        buf_byte(out, byte);
    }
    return NULL;
}

struct diag *lex_decode(const char *text, uint len, struct pos pos, struct diag **diags, struct buf *out) {
    uint units;
    return decode_units(text, len, pos, diags, 0, out, &units);
}

/* A character constant's value, an int (C99 6.4.4.4): its byte, or for
 * several (`'ab'`) the bytes as the digits of a number in base 256, as
 * clang computes it, keeping the low 32 bits, with clang's warning for each
 * (and one more past four characters, which do not all fit). */
struct diag *lex_char_constant(const char *text, uint len, struct pos pos, struct diag **diags,
                               int64_t *value) {
    /* Where the diagnostics of this constant start. */
    struct diag **mine = diags;
    while (*mine != NULL) {
        mine = &(*mine)->next;
    }
    struct buf bytes;
    memset(&bytes, 0, sizeof bytes);
    uint chars;
    struct diag *err = decode_units(text, len, pos, diags, 1, &bytes, &chars);
    if (err != NULL) {
        buf_free(&bytes);
        return err;
    }
    /* A character too large for it leaves the constant no value whose length
     * clang tells of. */
    int too_large = 0;
    for (struct diag *d = *mine; d != NULL; d = d->next) {
        if (!diag_is_warning(d)) {
            too_large = 1;
        }
    }
    /* Four characters are clang's -Wfour-char-constants, off by default. */
    if (chars > 1 && chars != 4) {
        diag_append(diags, diag_warning(pos, "multi-character character constant", "multichar"));
    }
    if (chars > 4 && !too_large) {
        diag_append(diags, diag_warning_unflagged(pos, "character constant too long for its type"));
    }
    if (bytes.len == 0) {
        *value = 0;
    } else if (bytes.len == 1) {
        *value = (uchar)bytes.data[0];
    } else {
        uint v = 0;
        for (uint i = 0; i < bytes.len; i++) {
            v = v << 8 | (uchar)bytes.data[i];
        }
        *value = (int)v;
    }
    buf_free(&bytes);
    return NULL;
}
