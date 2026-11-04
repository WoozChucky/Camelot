/* as: tokens to statements (merlin-as's parser.rs).
 *
 *   line      := label* (directive | insn | instruction)?
 *   label     := IDENT ':' | INT ':'
 *   directive := '.'IDENT (dirarg (',' dirarg)*)?   (.file and .loc: spaced)
 *   insn      := '.insn' IDENT? operand (',' operand)*
 *   instr     := IDENT (operand (',' operand)*)?
 *   operand   := REG | expr '(' REG ')' | '(' REG ')' | expr
 *   expr      := | ^ & << >> + - * / % by precedence, unary - ~ +, primary
 *   primary   := INT | IDENT | LOCALREF | '(' expr ')' | '%' IDENT '(' expr ')'
 *
 * Numeric label references (`1b`, `1f`) stay names; the assembler resolves
 * them, knowing which definitions came before. */
#include "as.h"
#include "tables.h"

static struct token *toks;
static int ntoks;
static int pos;
/* A syntax error that belongs to the next statement (parse_takes_next). */
static int takes_next;
static uint takes_next_line;

int parse_takes_next(uint *line) {
    int r = takes_next;
    *line = takes_next_line;
    takes_next = 0;
    return r;
}

void parse_report_next(uint line) {
    error_at(line, "expected a modifier name after '%'");
}

static struct token *peek_at(int n) {
    int i = pos + n;
    return &toks[i < ntoks ? i : ntoks - 1];
}

static struct token *peek(void) {
    return peek_at(0);
}

static int is_punct(struct token *t, int c) {
    return t->kind == T_PUNCT && t->punct == c;
}

/* Syntax errors come with the line of the token the parser is at. */
static int fail(const char *msg) {
    error_at(peek()->line, msg);
    return -1;
}

static struct expr *new_expr(int kind) {
    struct expr *e = tmp_alloc(sizeof *e);
    e->kind = kind;
    return e;
}

int reg_parse(const char *name) {
    /* x0..x31: Rust's `parse::<u8>()` on what follows the x. */
    if (name[0] == 'x' && name[1] != 0) {
        uint v = 0;
        int ok = 1;
        for (const char *p = name + 1; *p; p++) {
            if (*p < '0' || *p > '9') {
                ok = 0;
                break;
            }
            v = v * 10 + (uint)(*p - '0');
            if (v > 255) {
                ok = 0;
                break;
            }
        }
        if (ok) {
            return v < 32 ? (int)v : -1;
        }
    }
    if (strcmp(name, "fp") == 0) {
        return 8;
    }
    for (int i = 0; i < 32; i++) {
        if (strcmp(name, REG_NAMES[i]) == 0) {
            return i;
        }
    }
    return -1;
}

const char *modifier_name(int m) {
    switch (m) {
    case M_HI:
        return "%hi";
    case M_LO:
        return "%lo";
    case M_PCREL_HI:
        return "%pcrel_hi";
    default:
        return "%pcrel_lo";
    }
}

static int reg_at(int n) {
    struct token *t = peek_at(n);
    return t->kind == T_IDENT ? reg_parse(t->text) : -1;
}

static struct expr *expr(void);

static struct expr *primary(void) {
    /* Never consume the statement's end on an error. */
    if (peek()->kind == T_NEWLINE) {
        fail("expected an expression");
        return NULL;
    }
    struct token *t = peek();
    pos++;
    if (t->kind == T_INT) {
        struct expr *e = new_expr(E_NUM);
        e->num = t->num;
        return e;
    }
    if (t->kind == T_WIDE) {
        pos--;
        error_at(t->line, "number too large for 64 bits");
        return NULL;
    }
    if (t->kind == T_IDENT) {
        struct expr *e = new_expr(E_SYM);
        e->sym = t->text;
        return e;
    }
    if (t->kind == T_LOCALREF) {
        struct expr *e = new_expr(E_SYM);
        char name[16];
        snprintf(name, sizeof name, "%u%c", t->num.lo, t->fwd ? 'f' : 'b');
        e->sym = tmp_strndup(name, (uint)strlen(name));
        return e;
    }
    if (is_punct(t, '(')) {
        struct expr *e = expr();
        if (e == NULL) {
            return NULL;
        }
        if (!is_punct(peek(), ')')) {
            fail("expected ')'");
            return NULL;
        }
        pos++;
        return e;
    }
    if (is_punct(t, '%')) {
        struct token *name = peek();
        pos++;
        if (name->kind == T_NEWLINE) {
            /* merlin-as reads the statement's end as the name: its error is
             * on the next statement's line, and it skips that one too. */
            takes_next = 1;
            takes_next_line = name->line;
            return NULL;
        }
        if (name->kind != T_IDENT) {
            fail("expected a modifier name after '%'");
            return NULL;
        }
        int m;
        if (strcmp(name->text, "hi") == 0) {
            m = M_HI;
        } else if (strcmp(name->text, "lo") == 0) {
            m = M_LO;
        } else if (strcmp(name->text, "pcrel_hi") == 0) {
            m = M_PCREL_HI;
        } else if (strcmp(name->text, "pcrel_lo") == 0) {
            m = M_PCREL_LO;
        } else {
            fail(fmt("unknown modifier '%%%s'", name->text));
            return NULL;
        }
        if (!is_punct(peek(), '(')) {
            fail("expected '('");
            return NULL;
        }
        pos++;
        struct expr *inner = expr();
        if (inner == NULL) {
            return NULL;
        }
        if (!is_punct(peek(), ')')) {
            fail("expected ')'");
            return NULL;
        }
        pos++;
        struct expr *e = new_expr(E_MOD);
        e->op = m;
        e->a = inner;
        return e;
    }
    pos--;
    fail("expected an expression");
    pos++;
    return NULL;
}

static struct expr *unary(void) {
    struct token *t = peek();
    if (is_punct(t, '-') || is_punct(t, '~')) {
        pos++;
        struct expr *a = unary();
        if (a == NULL) {
            return NULL;
        }
        struct expr *e = new_expr(t->punct == '-' ? E_NEG : E_NOT);
        e->a = a;
        return e;
    }
    if (is_punct(t, '+')) {
        pos++;
        return unary();
    }
    return primary();
}

/* Precedence climbing: a higher level binds tighter. */
static struct expr *binary(int min_level) {
    struct expr *lhs = unary();
    if (lhs == NULL) {
        return NULL;
    }
    for (;;) {
        struct token *t = peek();
        int op, level;
        if (t->kind == T_SHIFT) {
            op = t->fwd ? B_SHL : B_SHR;
            level = 3;
        } else if (t->kind != T_PUNCT) {
            return lhs;
        } else if (t->punct == '|') {
            op = B_OR, level = 0;
        } else if (t->punct == '^') {
            op = B_XOR, level = 1;
        } else if (t->punct == '&') {
            op = B_AND, level = 2;
        } else if (t->punct == '+') {
            op = B_ADD, level = 4;
        } else if (t->punct == '-') {
            op = B_SUB, level = 4;
        } else if (t->punct == '*') {
            op = B_MUL, level = 5;
        } else if (t->punct == '/') {
            op = B_DIV, level = 5;
        } else if (t->punct == '%') {
            op = B_REM, level = 5;
        } else {
            return lhs;
        }
        if (level < min_level) {
            return lhs;
        }
        pos++;
        struct expr *rhs = binary(level + 1);
        if (rhs == NULL) {
            return NULL;
        }
        struct expr *e = new_expr(E_BIN);
        e->op = op;
        e->a = lhs;
        e->b = rhs;
        lhs = e;
    }
}

static struct expr *expr(void) {
    return binary(0);
}

static int operand(struct operand *op) {
    memset(op, 0, sizeof *op);
    /* A plain register. */
    int r = reg_at(0);
    if (r >= 0 && !is_punct(peek_at(1), '(')) {
        pos++;
        op->kind = OPD_REG;
        op->reg = r;
        return 0;
    }
    /* `(reg)` with no offset. */
    if (is_punct(peek(), '(') && is_punct(peek_at(2), ')')) {
        int base = reg_at(1);
        if (base >= 0) {
            pos += 3;
            op->kind = OPD_MEM;
            op->e = new_expr(E_NUM);
            op->reg = base;
            op->int_token = 1;
            return 0;
        }
    }
    int start = pos;
    struct expr *e = expr();
    if (e == NULL) {
        return -1;
    }
    int int_token = pos == start + 1 && toks[start].kind == T_INT;
    /* `expr(reg)` */
    if (is_punct(peek(), '(')) {
        int base = reg_at(1);
        if (base >= 0 && is_punct(peek_at(2), ')')) {
            pos += 3;
            op->kind = OPD_MEM;
            op->e = e;
            op->reg = base;
            op->int_token = int_token;
            return 0;
        }
    }
    op->kind = OPD_EXPR;
    op->e = e;
    return 0;
}

/* Operands, separated by commas, up to the statement's end. */
static int operands(struct stmt *s) {
    if (peek()->kind == T_NEWLINE) {
        return 0;
    }
    uint cap = 0;
    for (;;) {
        if (s->nops == cap) {
            uint c = cap ? cap * 2 : 4;
            struct operand *ops = tmp_alloc(c * sizeof(struct operand));
            memcpy(ops, s->ops, s->nops * sizeof(struct operand));
            s->ops = ops;
            cap = c;
        }
        if (operand(&s->ops[s->nops]) < 0) {
            return -1;
        }
        s->nops++;
        if (!is_punct(peek(), ',')) {
            return 0;
        }
        pos++;
    }
}

static struct dirarg *new_arg(struct stmt *s, uint *cap) {
    if (s->nargs == *cap) {
        uint c = *cap ? *cap * 2 : 4;
        struct dirarg *args = tmp_alloc(c * sizeof(struct dirarg));
        memcpy(args, s->args, s->nargs * sizeof(struct dirarg));
        s->args = args;
        *cap = c;
    }
    struct dirarg *a = &s->args[s->nargs++];
    memset(a, 0, sizeof *a);
    return a;
}

/* A directive's arguments: expressions or strings, separated by commas.
 * With `empty`, one may be left out after a comma when another comma
 * follows (the fill of `.p2align 3,,4`, as LLVM reads it). */
static int dir_args(struct stmt *s, int empty) {
    if (peek()->kind == T_NEWLINE) {
        return 0;
    }
    uint cap = 0;
    for (;;) {
        struct dirarg *a = new_arg(s, &cap);
        struct token *t = peek();
        if (empty && s->nargs > 1 && is_punct(t, ',')) {
            a->kind = DA_EMPTY;
        } else if (t->kind == T_STR) {
            pos++;
            a->kind = DA_STR;
            a->str = (uchar *)t->text;
            a->len = t->len;
        } else {
            a->kind = DA_EXPR;
            a->e = expr();
            if (a->e == NULL) {
                return -1;
            }
        }
        if (!is_punct(peek(), ',')) {
            return 0;
        }
        pos++;
    }
}

/* Makes `a` the string of the n bytes at p. */
static void str_arg(struct dirarg *a, const char *p, uint n) {
    a->kind = DA_STR;
    a->str = (uchar *)p;
    a->len = n;
}

/* The arguments of `.section name, "flags", @type, entsize`, read as
 * dir_args reads them, except the type, the third: LLVM takes it as
 * `@progbits`, `%progbits` or `"progbits"` (a name or a number), and
 * whichever it is, it becomes the string `progbits` for the assembler.
 * `%progbits` alone would not parse as an expression. */
static int section_args(struct stmt *s) {
    if (peek()->kind == T_NEWLINE) {
        return 0;
    }
    uint cap = 0;
    for (;;) {
        struct dirarg *a = new_arg(s, &cap);
        struct token *t = peek();
        struct token *next = peek_at(1);
        if (s->nargs == 3 && t->kind == T_IDENT && t->text[0] == '@') {
            pos++;
            str_arg(a, t->text + 1, (uint)strlen(t->text) - 1);
        } else if (s->nargs == 3 && is_punct(t, '%') && next->kind == T_IDENT) {
            pos += 2;
            str_arg(a, next->text, (uint)strlen(next->text));
        } else if (s->nargs == 3 && is_punct(t, '%') && next->kind == T_INT) {
            /* The number in decimal, as the type name's digits. */
            pos += 2;
            const char *d = i64_dec(next->num);
            str_arg(a, tmp_strndup(d, (uint)strlen(d)), (uint)strlen(d));
        } else if (t->kind == T_STR) {
            pos++;
            str_arg(a, t->text, t->len);
        } else {
            a->kind = DA_EXPR;
            a->e = expr();
            if (a->e == NULL) {
                return -1;
            }
        }
        if (!is_punct(peek(), ',')) {
            return 0;
        }
        pos++;
    }
}

/* The arguments of `.file` and `.loc`, separated by spaces (or commas). */
static int spaced_args(struct stmt *s) {
    uint cap = 0;
    for (;;) {
        struct token *t = peek();
        if (t->kind == T_NEWLINE) {
            return 0;
        }
        if (is_punct(t, ',')) {
            pos++;
            continue;
        }
        struct dirarg *a = new_arg(s, &cap);
        if (t->kind == T_STR) {
            pos++;
            a->kind = DA_STR;
            a->str = (uchar *)t->text;
            a->len = t->len;
        } else if (t->kind == T_WIDE) {
            pos++;
            a->kind = DA_WIDE;
            memcpy(a->wide, t->wide, 16);
        } else {
            a->kind = DA_EXPR;
            a->e = expr();
            if (a->e == NULL) {
                return -1;
            }
        }
    }
}

static char *lowercase(const char *s) {
    uint n = (uint)strlen(s);
    char *r = tmp_strndup(s, n);
    for (uint i = 0; i < n; i++) {
        if (r[i] >= 'A' && r[i] <= 'Z') {
            r[i] = (char)(r[i] - 'A' + 'a');
        }
    }
    return r;
}

int parse_statement(struct token *t, int n, struct stmt **out) {
    toks = t;
    ntoks = n;
    pos = 0;
    /* At most a label per token pair, and one statement. */
    struct stmt *stmts = tmp_alloc((uint)(n / 2 + 2) * sizeof(struct stmt));
    int count = 0;
    uint line = peek()->line;
    /* Labels. */
    for (;;) {
        struct token *a = peek(), *b = peek_at(1);
        if (!is_punct(b, ':')) {
            break;
        }
        if (a->kind == T_IDENT) {
            stmts[count].kind = S_LABEL;
            stmts[count].line = line;
            stmts[count].name = a->text;
            count++;
            pos += 2;
        } else if (a->kind == T_INT && !i64_is_neg(a->num)) {
            stmts[count].kind = S_LABEL;
            stmts[count].line = line;
            stmts[count].name = tmp_strndup(i64_dec(a->num), (uint)strlen(i64_dec(a->num)));
            count++;
            pos += 2;
        } else {
            break;
        }
    }
    struct token *head = peek();
    pos++;
    struct stmt *s = &stmts[count];
    memset(s, 0, sizeof *s);
    s->line = line;
    if (head->kind == T_NEWLINE) {
        *out = stmts;
        return count;
    }
    if (head->kind == T_IDENT && strcmp(head->text, ".insn") == 0) {
        /* `.insn i ...` is an instruction named `.insn i`; `.insn` alone
         * is the raw form. */
        s->kind = S_INSN;
        s->name = head->text;
        struct token *f = peek();
        if (f->kind == T_IDENT && !is_punct(peek_at(1), ',') && peek_at(1)->kind != T_NEWLINE) {
            pos++;
            char *lf = lowercase(f->text);
            uint ln = (uint)strlen(lf);
            char *name = tmp_alloc(ln + 7);
            memcpy(name, ".insn ", 6);
            memcpy(name + 6, lf, ln);
            s->name = name;
        }
        if (operands(s) < 0) {
            return -1;
        }
    } else if (head->kind == T_IDENT && head->text[0] == '.') {
        s->kind = S_DIRECTIVE;
        s->name = head->text;
        int r;
        if (strcmp(head->text, ".file") == 0 || strcmp(head->text, ".loc") == 0) {
            r = spaced_args(s);
        } else if (strcmp(head->text, ".section") == 0) {
            r = section_args(s);
        } else {
            int align = strcmp(head->text, ".align") == 0 || strcmp(head->text, ".p2align") == 0 ||
                        strcmp(head->text, ".balign") == 0;
            r = dir_args(s, align);
        }
        if (r < 0) {
            return -1;
        }
    } else if (head->kind == T_IDENT && is_punct(peek(), '=')) {
        /* `sym = expr` is shorthand for `.set sym, expr`. */
        pos++;
        struct expr *e = expr();
        if (e == NULL) {
            return -1;
        }
        s->kind = S_DIRECTIVE;
        s->name = ".set";
        s->args = tmp_alloc(2 * sizeof(struct dirarg));
        s->nargs = 2;
        s->args[0].kind = DA_EXPR;
        s->args[0].e = new_expr(E_SYM);
        s->args[0].e->sym = head->text;
        s->args[1].kind = DA_EXPR;
        s->args[1].e = e;
    } else if (head->kind == T_IDENT) {
        s->kind = S_INSN;
        s->name = lowercase(head->text);
        if (operands(s) < 0) {
            return -1;
        }
    } else {
        error_at(line, "expected a label, directive or instruction");
        return -1;
    }
    if (peek()->kind != T_NEWLINE) {
        error_at(line, "unexpected text at end of line");
        return -1;
    }
    count++;
    *out = stmts;
    return count;
}
