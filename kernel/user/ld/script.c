/* ld: the linker script subset (merlin-ld's script.rs, spec section 10,
 * "Linker scripts"):
 *
 *     OUTPUT_ARCH(riscv)                       optional; must say riscv
 *     OUTPUT_FORMAT("elf32-littleriscv")       optional; elf32-littleriscv or
 *                                              elf64-littleriscv, the objects' class
 *     ENTRY(symbol)
 *     MEMORY { RAM (rwx) : ORIGIN = expr, LENGTH = expr }      at most one region
 *     SECTIONS
 *     {
 *         . = expr;   . += expr;   symbol = expr;
 *         .name : { *(pattern pattern ...) ... . = ALIGN(n); . += n; symbol = expr; } > RAM
 *         .name (NOLOAD) : AT(lma) ALIGN(n) { ... } > RAM    each optional, in this order
 *         /DISCARD/ : { *(pattern ...) }
 *     }
 *
 * Expressions: numbers (16, 0x80000000, 64K, 128M), `.`, symbols, ALIGN(n),
 * ORIGIN(region), LENGTH(region), ADDR(section), parentheses, + and -.
 * Comments are slash-star ones. Anything else is an error naming what was
 * found.
 *
 * AT(lma) gives an output section a load address (the p_paddr of its
 * segment) other than the address it runs at; ADDR(section) is an output
 * section's address, as in Linux's `AT(ADDR(.text) - LOAD_OFFSET)`. layout.c
 * explains what they mean.
 *
 * The objects are RV32 here (object.c reads ELF32 only, until #157), so a
 * script's values are merlin-ld's ELF32 ones: exact 64-bit numbers up to
 * 2^63 - 1, an overflow an error, the 4 GiB address space checked where a
 * value is used. */
#include "ld.h"

const char DEFAULT_SCRIPT[] = "\n"
                              "ENTRY(_start)\n"
                              "SECTIONS\n"
                              "{\n"
                              "    . = 0x80000000;\n"
                              "    .text : { *(.text.start) *(.text.init) *(.text .text.*) }\n"
                              "    .rodata : { *(.rodata .rodata.* .srodata .srodata.*) }\n"
                              "    .data : { *(.data .data.* .sdata .sdata.*) }\n"
                              "    .bss : { *(.bss .bss.* .sbss .sbss.*) }\n"
                              "}\n";

/* The error of a failed parse. */
static uint err_line;
static char *err_msg;

/* Whether the script is read for ELF64 (script_parse's elf64). */
static int elf64;

static int fail(uint line, char *msg) {
    err_line = line;
    err_msg = msg;
    return 0;
}

/* ---- Tokens ---- */

enum { TK_WORD, TK_STR, TK_PUNCT };

struct tok {
    int kind;
    char *text;
    uint line;
    /* Whether it follows the token before with nothing between them, no
     * space and no comment (read_name joins such runs). */
    int glued;
};

static struct tok *toks;
static uint ntoks, captoks;
static uint pos;
/* Whether the next token follows the last one directly. */
static int adjacent;

static void push_tok(int kind, char *text, uint line) {
    toks = grow(toks, &captoks, ntoks + 1, sizeof(struct tok));
    toks[ntoks].kind = kind;
    toks[ntoks].text = text;
    toks[ntoks].line = line;
    toks[ntoks].glued = adjacent;
    adjacent = 1;
    ntoks++;
}

/* How a token is shown in messages: 'word', "string" or 'punct'. */
static char *shown(struct tok *t) {
    return t->kind == TK_STR ? fmt("\"%s\"", t->text) : fmt("'%s'", t->text);
}

static int is_word_char(int c) {
    return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '_' || c == '.' ||
           c == '$' || c == '*' || c == '?';
}

static int is_space(int c) {
    return c == ' ' || c == '\t' || c == '\r' || c == '\v' || c == '\f';
}

/* The UTF-8 character at s (for a message about it). */
static char *utf8_char(const char *s) {
    uint c = (uchar)s[0];
    uint n = c >= 0xf0 ? 4 : c >= 0xe0 ? 3 : c >= 0xc0 ? 2 : 1;
    uint k = 0;
    while (k < n && s[k] != 0) {
        k++;
    }
    return xstrndup(s, k);
}

static const char *const PUNCTS[] = {"{", "}", "(", ")", ":", ";", ",", "=", ">", "-", NULL};

static int lex(const char *src) {
    uint line = 1;
    ntoks = 0;
    adjacent = 0;
    const char *p = src;
    while (*p) {
        int c = (uchar)*p;
        if (c == '\n') {
            line++;
            p++;
            adjacent = 0;
        } else if (is_space(c)) {
            p++;
            adjacent = 0;
        } else if (c == '/') {
            p++;
            if (strncmp(p, "DISCARD/", 8) == 0) {
                p += 8;
                push_tok(TK_WORD, DISCARD_NAME, line);
                continue;
            }
            if (*p != '*') {
                return fail(line, "unexpected '/' (only /* */ comments are supported)");
            }
            p++;
            uint start = line;
            int prev = ' ';
            for (;;) {
                if (*p == 0) {
                    return fail(start, "unterminated comment");
                }
                int ch = (uchar)*p++;
                if (ch == '/' && prev == '*') {
                    break;
                }
                if (ch == '\n') {
                    line++;
                }
                prev = ch;
            }
            adjacent = 0;
        } else if (c == '"') {
            p++;
            const char *s = p;
            while (*p != '"') {
                if (*p == '\n' || *p == 0) {
                    return fail(line, "unterminated string");
                }
                p++;
            }
            push_tok(TK_STR, xstrndup(s, (uint)(p - s)), line);
            p++;
        } else if (c == '+') {
            p++;
            if (*p == '=') {
                p++;
                push_tok(TK_PUNCT, "+=", line);
            } else {
                push_tok(TK_PUNCT, "+", line);
            }
        } else if (is_word_char(c)) {
            const char *s = p;
            while (is_word_char((uchar)*p)) {
                p++;
            }
            push_tok(TK_WORD, xstrndup(s, (uint)(p - s)), line);
        } else {
            int k = 0;
            while (PUNCTS[k] != NULL && PUNCTS[k][0] != c) {
                k++;
            }
            if (PUNCTS[k] == NULL) {
                return fail(line, fmt("unexpected character '%s'", utf8_char(p)));
            }
            push_tok(TK_PUNCT, (char *)PUNCTS[k], line);
            p++;
        }
    }
    return 1;
}

/* ---- Output formats ---- */

const char *format_name(int format) {
    return format == FORMAT_ELF64 ? "elf64-littleriscv" : "elf32-littleriscv";
}

/* The class an OUTPUT_FORMAT name stands for; FORMAT_NONE if merlin-ld does
 * not know it. */
static int format_class(const char *name) {
    if (strcmp(name, "elf32-littleriscv") == 0) {
        return FORMAT_ELF32;
    }
    if (strcmp(name, "elf64-littleriscv") == 0) {
        return FORMAT_ELF64;
    }
    return FORMAT_NONE;
}

/* ---- Numbers and globs ---- */

/* a > b, as unsigned 64-bit numbers. */
static int ugt(i64 a, i64 b) {
    return a.hi != b.hi ? a.hi > b.hi : a.lo > b.lo;
}

/* parse_number, for an ELF64 script (wide) or an ELF32 one: up to 2^64 - 1,
 * or up to i64::MAX. */
static int number(const char *w, i64 *out, int wide) {
    uint n = (uint)strlen(w);
    if (n == 0) {
        return 0;
    }
    uint mult = 1;
    if (w[n - 1] == 'K' || w[n - 1] == 'k') {
        mult = 1024;
        n--;
    } else if (w[n - 1] == 'M' || w[n - 1] == 'm') {
        mult = 1024 * 1024;
        n--;
    }
    /* The most either form may be before the multiplier, and a tenth of it. */
    i64 max, tenth;
    max.lo = 0xffffffffu;
    max.hi = wide ? 0xffffffffu : 0x7fffffffu;
    tenth.lo = wide ? 0x99999999u : 0xccccccccu;
    tenth.hi = wide ? 0x19999999u : 0x0cccccccu;
    i64 v = i64_u(0);
    if (n >= 2 && w[0] == '0' && (w[1] == 'x' || w[1] == 'X')) {
        if (n == 2) {
            return 0;
        }
        for (uint i = 2; i < n; i++) {
            int c = w[i];
            uint d = c >= '0' && c <= '9' ? (uint)(c - '0')
                     : c >= 'a' && c <= 'f' ? (uint)(c - 'a' + 10)
                     : c >= 'A' && c <= 'F' ? (uint)(c - 'A' + 10)
                                            : 16;
            if (d >= 16 || ugt(v, i64_shr(max, 4))) {
                return 0;
            }
            v = i64_add(i64_shl(v, 4), i64_u(d));
        }
    } else {
        if (n == 0) {
            return 0;
        }
        for (uint i = 0; i < n; i++) {
            if (w[i] < '0' || w[i] > '9') {
                return 0;
            }
            if (ugt(v, tenth)) {
                return 0;
            }
            i64 ten = i64_mul(v, i64_u(10));
            v = i64_add(ten, i64_u((uint)(w[i] - '0')));
            /* Past max, or round 2^64. */
            if (ugt(v, max) || ugt(ten, v)) {
                return 0;
            }
        }
    }
    if (mult != 1) {
        if (ugt(v, i64_shr(max, mult == 1024 ? 10 : 20))) {
            return 0;
        }
        v = i64_mul(v, i64_u(mult));
    }
    *out = v;
    return 1;
}

int parse_number(const char *w, i64 *out) {
    return number(w, out, 0);
}

int glob_match(const char *p, const char *n) {
    uint pl = (uint)strlen(p), nl = (uint)strlen(n);
    uint pi = 0, ni = 0;
    int star = 0;
    uint sp = 0, sn = 0;
    while (ni < nl) {
        if (pi < pl && (p[pi] == '?' || p[pi] == n[ni])) {
            pi++;
            ni++;
        } else if (pi < pl && p[pi] == '*') {
            star = 1;
            sp = pi;
            sn = ni;
            pi++;
        } else if (star) {
            pi = sp + 1;
            ni = sn + 1;
            sn++;
        } else {
            return 0;
        }
    }
    for (; pi < pl; pi++) {
        if (p[pi] != '*') {
            return 0;
        }
    }
    return 1;
}

int expr_uses_dot(struct expr *e) {
    if (e->kind == X_DOT || e->kind == X_ALIGN) {
        return 1;
    }
    if (e->kind == X_ADD || e->kind == X_SUB) {
        return expr_uses_dot(e->a) || expr_uses_dot(e->b);
    }
    return 0;
}

/* ld.lld's expressionsUseSymbols: ADDR and ORIGIN name no symbol. */
int expr_uses_symbols(struct expr *e) {
    if (e->kind == X_SYMBOL) {
        return 1;
    }
    if (e->kind == X_ALIGN) {
        return expr_uses_symbols(e->a);
    }
    if (e->kind == X_ADD || e->kind == X_SUB) {
        return expr_uses_symbols(e->a) || expr_uses_symbols(e->b);
    }
    return 0;
}

static int expr_names_addr(struct expr *e, const char *name) {
    if (e == NULL) {
        return 0;
    }
    if (e->kind == X_ADDR) {
        return strcmp(e->name, name) == 0;
    }
    if (e->kind == X_ALIGN) {
        return expr_names_addr(e->a, name);
    }
    if (e->kind == X_ADD || e->kind == X_SUB) {
        return expr_names_addr(e->a, name) || expr_names_addr(e->b, name);
    }
    return 0;
}

/* ld.lld's usedInExpression: an empty section still has an address, which
 * the expression needs. */
int script_names_addr(struct script *s, const char *name) {
    for (uint c = 0; c < s->ncommands; c++) {
        struct command *cmd = &s->commands[c];
        if (!cmd->is_output) {
            if (expr_names_addr(cmd->assign.e, name)) {
                return 1;
            }
            continue;
        }
        struct output_desc *o = &cmd->out;
        if (expr_names_addr(o->at, name) || expr_names_addr(o->align, name)) {
            return 1;
        }
        for (uint i = 0; i < o->nitems; i++) {
            if (o->items[i].is_assign && expr_names_addr(o->items[i].assign.e, name)) {
                return 1;
            }
        }
    }
    return 0;
}

/* ---- The parser ---- */

static struct tok *peek(void) {
    return pos < ntoks ? &toks[pos] : NULL;
}

static uint cur_line(void) {
    if (pos < ntoks) {
        return toks[pos].line;
    }
    return ntoks > 0 ? toks[ntoks - 1].line : 1;
}

static struct tok *next(void) {
    if (pos < ntoks) {
        return &toks[pos++];
    }
    fail(cur_line(), "unexpected end of script");
    return NULL;
}

static int is(const char *p) {
    struct tok *t = peek();
    return t != NULL && t->kind == TK_PUNCT && strcmp(t->text, p) == 0;
}

static int eat(const char *p) {
    if (is(p)) {
        pos++;
        return 1;
    }
    return 0;
}

static int expect(const char *p) {
    uint line = cur_line();
    struct tok *t = next();
    if (t == NULL) {
        return 0;
    }
    if (t->kind == TK_PUNCT && strcmp(t->text, p) == 0) {
        return 1;
    }
    return fail(line, fmt("expected '%s', found %s", p, shown(t)));
}

static int word(const char *what, char **out) {
    uint line = cur_line();
    struct tok *t = next();
    if (t == NULL) {
        return 0;
    }
    if (t->kind == TK_WORD) {
        *out = t->text;
        return 1;
    }
    return fail(line, fmt("expected %s, found %s", what, shown(t)));
}

static int is_dash(struct tok *t) {
    return t->kind == TK_PUNCT && strcmp(t->text, "-") == 0;
}

/* A name: a section name, a pattern, a symbol an assignment sets, a region,
 * a format. ld.lld's lexer reads these outside expressions, where `-` is a
 * character of a name as letters are, so elf64-littleriscv and *(.text-x)
 * are one token each, while inside an expression a-b is a - b. This lexer
 * keeps `-` apart and joins it back here: the name is the word or `-` there
 * and every word and `-` that follows it with nothing between them. */
static int read_name(const char *what, char **out) {
    uint line = cur_line();
    struct tok *t = next();
    if (t == NULL) {
        return 0;
    }
    if (t->kind != TK_WORD && !is_dash(t)) {
        return fail(line, fmt("expected %s, found %s", what, shown(t)));
    }
    char *name = t->text;
    while (pos < ntoks && toks[pos].glued && (toks[pos].kind == TK_WORD || is_dash(&toks[pos]))) {
        name = fmt("%s%s", name, toks[pos].text);
        pos++;
    }
    *out = name;
    return 1;
}

/* A name or a quoted string, as in OUTPUT_ARCH("riscv"). */
static int name_or_str(const char *what, char **out) {
    struct tok *t = peek();
    if (t != NULL && t->kind == TK_STR) {
        pos++;
        *out = t->text;
        return 1;
    }
    return read_name(what, out);
}

static struct expr *new_expr(int kind) {
    struct expr *e = xcalloc(sizeof(struct expr));
    e->kind = kind;
    return e;
}

static int expr(struct expr **out);

static int atom(struct expr **out) {
    uint line = cur_line();
    if (eat("(")) {
        return expr(out) && expect(")");
    }
    char *w;
    if (!word("an expression", &w)) {
        return 0;
    }
    if (strcmp(w, ".") == 0) {
        *out = new_expr(X_DOT);
        return 1;
    }
    if (w[0] >= '0' && w[0] <= '9') {
        struct expr *e = new_expr(X_NUM);
        if (!number(w, &e->num, elf64)) {
            return fail(line, fmt("bad number '%s'", w));
        }
        *out = e;
        return 1;
    }
    if (!is("(")) {
        if (strchr(w, '*') != NULL || strchr(w, '?') != NULL) {
            return fail(line, fmt("'%s' is not a valid symbol name", w));
        }
        struct expr *e = new_expr(X_SYMBOL);
        e->name = w;
        *out = e;
        return 1;
    }
    if (!expect("(")) {
        return 0;
    }
    struct expr *e;
    if (strcmp(w, "ALIGN") == 0) {
        e = new_expr(X_ALIGN);
        if (!expr(&e->a)) {
            return 0;
        }
        if (is(",")) {
            return fail(line, "only the one-argument form ALIGN(n) is supported");
        }
    } else if (strcmp(w, "ORIGIN") == 0 || strcmp(w, "LENGTH") == 0) {
        e = new_expr(w[0] == 'O' ? X_ORIGIN : X_LENGTH);
        if (!read_name("a memory region name", &e->name)) {
            return 0;
        }
    } else if (strcmp(w, "ADDR") == 0) {
        e = new_expr(X_ADDR);
        if (!read_name("a section name", &e->name)) {
            return 0;
        }
    } else {
        return fail(line, fmt("unsupported function '%s' in expression", w));
    }
    *out = e;
    return expect(")");
}

static int expr(struct expr **out) {
    struct expr *e;
    if (!atom(&e)) {
        return 0;
    }
    for (;;) {
        int kind;
        if (eat("+")) {
            kind = X_ADD;
        } else if (eat("-")) {
            kind = X_SUB;
        } else {
            *out = e;
            return 1;
        }
        struct expr *b;
        if (!atom(&b)) {
            return 0;
        }
        struct expr *sum = new_expr(kind);
        sum->a = e;
        sum->b = b;
        e = sum;
    }
}

/* Evaluates an expression made only of numbers. */
static int const_eval(struct expr *e, i64 *out) {
    if (e->kind == X_NUM) {
        *out = e->num;
        return 1;
    }
    if (e->kind != X_ADD && e->kind != X_SUB) {
        return 0;
    }
    i64 a, b;
    if (!const_eval(e->a, &a) || !const_eval(e->b, &b)) {
        return 0;
    }
    i64 r = e->kind == X_ADD ? i64_add(a, b) : i64_sub(a, b);
    /* ELF64: modulo 2^64. ELF32: checked_add / checked_sub, no signed
     * overflow. */
    if (elf64) {
        *out = r;
        return 1;
    }
    int na = i64_is_neg(a), nb = i64_is_neg(b) ^ (e->kind == X_SUB), nr = i64_is_neg(r);
    if (e->kind == X_SUB && b.hi == 0x80000000u && b.lo == 0) {
        /* -i64::MIN does not exist: a - MIN overflows unless a < 0. */
        if (!na) {
            return 0;
        }
    } else if (na == nb && nr != na) {
        return 0;
    }
    *out = r;
    return 1;
}

/* `ORIGIN = constant-expr` (or one of the other names). */
static int region_field(const char *const *names, i64 *out) {
    uint line = cur_line();
    char *w;
    if (!word(names[0], &w)) {
        return 0;
    }
    int known = 0;
    for (int i = 0; names[i] != NULL; i++) {
        known |= strcmp(names[i], w) == 0;
    }
    if (!known) {
        return fail(line, fmt("expected %s, found '%s'", names[0], w));
    }
    struct expr *e;
    if (!expect("=") || !expr(&e)) {
        return 0;
    }
    if (!const_eval(e, out)) {
        return fail(line, fmt("%s must be a constant expression", names[0]));
    }
    return 1;
}

static const char *const ORIGIN_NAMES[] = {"ORIGIN", "org", "o", NULL};
static const char *const LENGTH_NAMES[] = {"LENGTH", "len", "l", NULL};

static int fits_u32(i64 v) {
    return v.hi == 0;
}

static int memory(struct script *s) {
    if (!expect("{")) {
        return 0;
    }
    int have = 0;
    while (!eat("}")) {
        uint line = cur_line();
        char *name;
        if (!read_name("a memory region name", &name)) {
            return 0;
        }
        if (have) {
            return fail(line, "only one MEMORY region is supported");
        }
        if (eat("(")) {
            char *attrs;
            if (!word("region attributes", &attrs)) {
                return 0;
            }
            for (char *c = attrs; *c; c++) {
                if (strchr("rwxaiRWXAI", *c) == NULL) {
                    return fail(line, fmt("unsupported region attributes '%s'", attrs));
                }
            }
            if (!expect(")")) {
                return 0;
            }
        }
        i64 origin, length;
        if (!expect(":") || !region_field(ORIGIN_NAMES, &origin) || !expect(",") ||
            !region_field(LENGTH_NAMES, &length)) {
            return 0;
        }
        if (elf64) {
            /* ELF64: the region ends below 2^64. */
            if (ugt(origin, i64_add(origin, length))) {
                return fail(line, fmt("MEMORY region %s extends past the 64-bit address space", name));
            }
        } else {
            if (!fits_u32(origin)) {
                return fail(line, fmt("MEMORY ORIGIN %s does not fit in 32 bits", i64_hex(origin)));
            }
            if (!fits_u32(length)) {
                return fail(line, fmt("MEMORY LENGTH %s does not fit in 32 bits", i64_hex(length)));
            }
            /* origin + length > 2^32: the region may end exactly at 4 GiB. */
            i64 end = i64_add(origin, length);
            if (end.hi > 1 || (end.hi == 1 && end.lo != 0)) {
                return fail(line, fmt("MEMORY region %s extends past 4 GiB", name));
            }
        }
        have = 1;
        s->has_memory = 1;
        s->region_name = name;
        s->origin = origin.lo;
        s->length = length.lo;
    }
    return 1;
}

/* The rest of an assignment after its target name. */
static int assign(char *name, uint line, struct assign *a) {
    a->add = eat("+=");
    if (!a->add && !expect("=")) {
        return 0;
    }
    a->symbol = strcmp(name, ".") == 0 ? NULL : name;
    a->line = line;
    if (a->add && a->symbol != NULL) {
        return fail(line, "'+=' is only supported for the location counter '.'");
    }
    return expr(&a->e) && expect(";");
}

/* The rest of an output section after its name, its (NOLOAD) and the colon:
 * [AT(lma)] [ALIGN(n)] { items } [> REGION]. ld.lld reads AT and ALIGN in
 * that order only (`ALIGN(n) AT(lma)` is its "{ expected, but got AT"). */
static int output_section(char *name, int noload, uint line, struct output_desc *o) {
    memset(o, 0, sizeof *o);
    o->name = name;
    o->noload = noload;
    o->line = line;
    if (strcmp(name, ".") == 0) {
        return fail(line, fmt("'%s' is not a valid output section name", name));
    }
    int discard = strcmp(name, DISCARD_NAME) == 0;
    if (discard && noload) {
        return fail(line, "/DISCARD/ cannot be NOLOAD");
    }
    struct tok *t = peek();
    if (t != NULL && t->kind == TK_WORD && strcmp(t->text, "AT") == 0) {
        if (discard) {
            return fail(cur_line(), "/DISCARD/ cannot have an AT");
        }
        pos++;
        if (!expect("(") || !expr(&o->at) || !expect(")")) {
            return 0;
        }
        t = peek();
    }
    if (t != NULL && t->kind == TK_WORD && strcmp(t->text, "ALIGN") == 0) {
        uint aline = cur_line();
        if (discard) {
            return fail(aline, "/DISCARD/ cannot have an ALIGN");
        }
        pos++;
        if (!expect("(") || !expr(&o->align)) {
            return 0;
        }
        if (is(",")) {
            return fail(aline, "only the one-argument form ALIGN(n) is supported");
        }
        if (!expect(")")) {
            return 0;
        }
        t = peek();
    }
    if (t == NULL) {
        return fail(cur_line(), "unexpected end of script");
    }
    if (!(t->kind == TK_PUNCT && strcmp(t->text, "{") == 0)) {
        return fail(cur_line(), fmt("unsupported syntax after '%s :': found %s (only AT(lma) and then ALIGN(n) may "
                                    "come before '{')",
                                    name, shown(t)));
    }
    if (!expect("{")) {
        return 0;
    }
    uint cap = 0;
    while (!eat("}")) {
        uint iline = cur_line();
        char *w;
        if (!read_name("an input section list or an assignment", &w)) {
            return 0;
        }
        if (is("=") || is("+=")) {
            if (discard) {
                return fail(iline, "/DISCARD/ may only list input sections");
            }
            struct assign a;
            if (!assign(w, iline, &a)) {
                return 0;
            }
            if (a.symbol == NULL && !a.add && !expr_uses_dot(a.e)) {
                return fail(iline, "inside an output section only '. = ALIGN(n)', '. += n' or expressions using "
                                   "'.' may move the location counter");
            }
            o->items = grow(o->items, &cap, o->nitems + 1, sizeof(struct item));
            memset(&o->items[o->nitems], 0, sizeof(struct item));
            o->items[o->nitems].is_assign = 1;
            o->items[o->nitems].assign = a;
            o->nitems++;
            continue;
        }
        if (!is("(")) {
            return fail(iline, fmt("expected '*(...)' or an assignment, found '%s'", w));
        }
        if (strcmp(w, "*") != 0) {
            return fail(iline, fmt("only '*(...)' input section lists are supported, not '%s(...)'", w));
        }
        if (!expect("(")) {
            return 0;
        }
        char **patterns = NULL;
        uint npatterns = 0, pcap = 0;
        while (!eat(")")) {
            uint pline = cur_line();
            char *p;
            if (!read_name("a section name pattern", &p)) {
                return 0;
            }
            if (is("(")) {
                return fail(pline, fmt("'%s(...)' is not supported inside an input section list", p));
            }
            patterns = grow(patterns, &pcap, npatterns + 1, sizeof(char *));
            patterns[npatterns++] = p;
        }
        if (npatterns == 0) {
            return fail(iline, "empty input section list '*()'");
        }
        o->items = grow(o->items, &cap, o->nitems + 1, sizeof(struct item));
        memset(&o->items[o->nitems], 0, sizeof(struct item));
        o->items[o->nitems].patterns = patterns;
        o->items[o->nitems].npatterns = npatterns;
        o->nitems++;
    }
    if (eat(">")) {
        if (discard) {
            return fail(cur_line(), "/DISCARD/ cannot go in a memory region");
        }
        if (!read_name("a memory region name", &o->region)) {
            return 0;
        }
    }
    eat(";");
    return 1;
}

static int sections(struct script *s) {
    if (!expect("{")) {
        return 0;
    }
    uint cap = 0;
    while (!eat("}")) {
        uint line = cur_line();
        char *name;
        if (!read_name("a section name, '.' or a symbol", &name)) {
            return 0;
        }
        s->commands = grow(s->commands, &cap, s->ncommands + 1, sizeof(struct command));
        struct command *c = &s->commands[s->ncommands];
        memset(c, 0, sizeof *c);
        if (is("=") || is("+=")) {
            if (!assign(name, line, &c->assign)) {
                return 0;
            }
            s->ncommands++;
            continue;
        }
        /* `name (NOLOAD) :`, the only output section type supported. */
        int noload = is("(") && pos + 1 < ntoks && toks[pos + 1].kind == TK_WORD &&
                     strcmp(toks[pos + 1].text, "NOLOAD") == 0;
        if (noload) {
            pos += 2;
            if (!expect(")")) {
                return 0;
            }
            if (!is(":")) {
                return fail(line, fmt("expected ':' after '%s (NOLOAD)'", name));
            }
        }
        if (eat(":")) {
            c->is_output = 1;
            if (!output_section(name, noload, line, &c->out)) {
                return 0;
            }
            s->ncommands++;
            continue;
        }
        struct tok *t = peek();
        if (t == NULL) {
            return fail(line, "unexpected end of script");
        }
        if (t->kind == TK_PUNCT && strcmp(t->text, "(") == 0) {
            return fail(line, fmt("unsupported linker script command '%s'", name));
        }
        return fail(line, fmt("unsupported syntax after '%s': found %s (output sections are `name [(NOLOAD)] : "
                              "[AT(lma)] [ALIGN(n)] { ... }`, without an address or other attributes)",
                              name, shown(t)));
    }
    return 1;
}

static int script(struct script *s) {
    int seen_sections = 0, seen_memory = 0;
    while (peek() != NULL) {
        uint line = cur_line();
        char *kw;
        if (!word("a linker script command", &kw)) {
            return 0;
        }
        if (strcmp(kw, "ENTRY") == 0) {
            if (!expect("(") || !read_name("an entry symbol", &s->entry) || !expect(")")) {
                return 0;
            }
        } else if (strcmp(kw, "OUTPUT_ARCH") == 0) {
            char *arch;
            if (!expect("(") || !name_or_str("an architecture", &arch)) {
                return 0;
            }
            char low[8];
            uint n = (uint)strlen(arch);
            for (uint i = 0; i < n && i < 7; i++) {
                low[i] = arch[i] >= 'A' && arch[i] <= 'Z' ? (char)(arch[i] - 'A' + 'a') : arch[i];
            }
            low[n < 7 ? n : 7] = 0;
            if (n != 5 || strcmp(low, "riscv") != 0) {
                return fail(line, fmt("OUTPUT_ARCH(%s): only riscv is supported", arch));
            }
            if (!expect(")")) {
                return 0;
            }
        } else if (strcmp(kw, "OUTPUT_FORMAT") == 0) {
            if (!expect("(")) {
                return 0;
            }
            /* OUTPUT_FORMAT(default, big, little): ld.lld goes by the first;
             * merlin-ld also wants the others to be names it knows. */
            for (;;) {
                char *f;
                if (!name_or_str("an output format", &f)) {
                    return 0;
                }
                int format = format_class(f);
                if (format == FORMAT_NONE) {
                    return fail(line, fmt("OUTPUT_FORMAT(%s): only elf32-littleriscv and elf64-littleriscv are "
                                          "supported",
                                          f));
                }
                if (s->format == FORMAT_NONE) {
                    s->format = format;
                }
                if (!eat(",")) {
                    break;
                }
            }
            if (!expect(")")) {
                return 0;
            }
        } else if (strcmp(kw, "MEMORY") == 0) {
            if (seen_memory) {
                return fail(line, "only one MEMORY command is supported");
            }
            seen_memory = 1;
            if (!memory(s)) {
                return 0;
            }
        } else if (strcmp(kw, "SECTIONS") == 0) {
            if (seen_sections) {
                return fail(line, "only one SECTIONS command is supported");
            }
            seen_sections = 1;
            s->commands = NULL;
            s->ncommands = 0;
            if (!sections(s)) {
                return 0;
            }
        } else {
            return fail(line, fmt("unsupported linker script command '%s'", kw));
        }
        eat(";");
    }
    return 1;
}

int script_parse(const char *src, struct script *out, uint *line, char **msg, int for_elf64) {
    memset(out, 0, sizeof *out);
    elf64 = for_elf64;
    pos = 0;
    if (!lex(src) || !script(out)) {
        *line = err_line;
        *msg = err_msg;
        return 0;
    }
    /* `> REGION` must name the region, and only when there is one. */
    for (uint i = 0; i < out->ncommands; i++) {
        struct output_desc *o = &out->commands[i].out;
        if (out->commands[i].is_output && o->region != NULL &&
            !(out->has_memory && strcmp(out->region_name, o->region) == 0)) {
            *line = o->line;
            *msg = fmt("output section %s refers to unknown region '%s'", o->name, o->region);
            return 0;
        }
    }
    return 1;
}
