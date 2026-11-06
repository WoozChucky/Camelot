/* cc1: the syntax tree, and --dump-ast's printer (follows merlin's ast.rs).
 *
 * The parser builds it with every variable unresolved and the types of
 * literals and casts filled in; sema does the rest: every variable says
 * where it lives, every expression has its C type, the conversions C does
 * implicitly are explicit casts, sizeof is its value, every member access
 * knows its offset, every switch its cases, and every call that passes or
 * returns a struct by value the hidden locals it needs. So code generation
 * never has to track scopes or C's conversion rules. */
#include "cc.h"

const char *binop_symbol(int op) {
    static const char *const symbols[] = {"+",  "-",  "*", "/",  "%", "<<", ">>", "&",  "|",
                                          "^",  "==", "!=", "<", "<=", ">", ">=", "&&", "||"};
    return symbols[op];
}

int binop_is_comparison(int op) {
    return op >= OP_EQ && op <= OP_GE;
}

const char *unop_symbol(int op) {
    static const char *const symbols[] = {"-", "+", "!", "~"};
    return symbols[op];
}

int var_eq(struct var a, struct var b) {
    if (a.kind != b.kind) {
        return 0;
    }
    if (a.kind == VAR_LOCAL) {
        return a.local == b.local;
    }
    if (a.kind == VAR_GLOBAL) {
        return strcmp(a.sym, b.sym) == 0;
    }
    return 1;
}

/* ---- Expressions ---- */

/* merlin's NEXT_EXPR: every expression made gets the next number. */
static uint64_t next_expr;

uint64_t expr_next_id(void) {
    return next_expr;
}

uint64_t expr_fresh_id(void) {
    return next_expr++;
}

/* A new expression, an int until something says otherwise. */
struct expr *expr_new(int kind, struct pos pos) {
    struct expr *e = arena_alloc(sizeof *e);
    e->kind = (uchar)kind;
    e->ty = ty_int(IK_INT);
    e->pos = pos;
    e->id = expr_fresh_id();
    e->result = -1;
    e->local = -1;
    return e;
}

struct expr *expr_num(i128 v, struct type *ty, struct pos pos) {
    struct expr *e = expr_new(E_NUM, pos);
    e->value = v;
    e->ty = ty;
    return e;
}

struct expr *expr_unop(int kind, struct expr *x, struct pos pos) {
    struct expr *e = expr_new(kind, pos);
    e->x = x;
    return e;
}

struct expr *expr_clone(struct expr *e) {
    if (e == NULL) {
        return NULL;
    }
    struct expr *c = arena_alloc(sizeof *c);
    *c = *e;
    c->x = expr_clone(e->x);
    c->y = expr_clone(e->y);
    c->z = expr_clone(e->z);
    if (e->nargs > 0) {
        c->args = arena_alloc(e->nargs * sizeof(struct expr *));
        for (uint i = 0; i < e->nargs; i++) {
            c->args[i] = expr_clone(e->args[i]);
        }
    }
    if (e->narg_copies > 0) {
        c->arg_copies = arena_alloc(e->narg_copies * sizeof(int));
        memcpy(c->arg_copies, e->arg_copies, e->narg_copies * sizeof(int));
    }
    c->init = init_clone(e->init);
    return c;
}

/* The symbol of the function a call goes straight to (f(x)), as opposed to
 * through a pointer. */
const char *direct_callee(struct expr *e) {
    if (e->kind == E_VAR && e->var.kind == VAR_GLOBAL && ty_is_func(e->ty)) {
        return e->var.sym;
    }
    return NULL;
}

/* ---- Initializers ---- */

struct pos init_pos(struct init *i) {
    if (i->kind == I_EXPR) {
        return i->e->pos;
    }
    if (i->kind == I_LIST) {
        return i->pos;
    }
    return i->ds[0]->pos;
}

struct init *init_clone(struct init *i) {
    if (i == NULL) {
        return NULL;
    }
    struct init *c = arena_alloc(sizeof *c);
    *c = *i;
    c->e = expr_clone(i->e);
    memset(&c->items, 0, sizeof c->items);
    for (uint k = 0; k < i->items.n; k++) {
        vec_push(&c->items, init_clone(i->items.v[k]));
    }
    if (i->nds > 0) {
        c->ds = arena_alloc(i->nds * sizeof(struct designator *));
        for (uint k = 0; k < i->nds; k++) {
            struct designator *d = arena_alloc(sizeof *d);
            *d = *i->ds[k];
            d->index = expr_clone(i->ds[k]->index);
            c->ds[k] = d;
        }
    }
    c->inner = init_clone(i->inner);
    return c;
}

struct unevaluated *unevaluated_new(struct expr *e) {
    struct unevaluated *u = arena_alloc(sizeof *u);
    u->expr = e;
    return u;
}

struct stmt *stmt_new(int kind, struct pos pos) {
    struct stmt *s = arena_alloc(sizeof *s);
    s->kind = (uchar)kind;
    s->pos = pos;
    s->id = -1;
    return s;
}

/* The function's type: its parameters unqualified, with how they were
 * spelled, which tells their enums. */
struct type *function_ty(struct function *f) {
    struct type **params = arena_alloc(f->nparams * sizeof(struct type *) + 1);
    for (uint i = 0; i < f->nparams; i++) {
        params[i] = ty_unqual(f->params[i].ty);
    }
    struct type *t = ty_func(f->ret, params, f->nparams, f->variadic);
    t->fn->spelled = arena_alloc(f->nparams * sizeof(struct param_spelling) + 1);
    for (uint i = 0; i < f->nparams; i++) {
        t->fn->spelled[i].ty = f->params[i].ty;
        t->fn->spelled[i].spelled = f->params[i].spelled;
    }
    t->fn->nspelled = f->nparams;
    return t;
}

/* ---- --dump-ast ----
 * The tree as indented text, one node per line, with source positions.
 * Expressions show their type after a colon; resolved variables show where
 * they live (#n for local n, @name for a global symbol). */

static void line(struct buf *out, uint depth, struct pos pos, const char *text) {
    for (uint i = 0; i < depth * 2; i++) {
        buf_byte(out, ' ');
    }
    buf_fmt(out, "%s <%u:%u>\n", text, pos.line, pos.col);
}

static char *where(struct var v) {
    if (v.kind == VAR_LOCAL) {
        return fmt(" #%u", v.local);
    }
    if (v.kind == VAR_GLOBAL) {
        return fmt(" @%s", v.sym);
    }
    return "";
}

void ast_quote(struct buf *b, uchar *bytes, uint len) {
    buf_byte(b, '"');
    for (uint i = 0; i < len; i++) {
        uint c = bytes[i];
        if (c == '"') {
            buf_str(b, "\\\"");
        } else if (c == '\\') {
            buf_str(b, "\\\\");
        } else if (c == '\n') {
            buf_str(b, "\\n");
        } else if (c == '\t') {
            buf_str(b, "\\t");
        } else if (c == '\r') {
            buf_str(b, "\\r");
        } else if (c >= 0x20 && c <= 0x7e) {
            buf_byte(b, c);
        } else {
            buf_byte(b, '\\');
            buf_byte(b, '0' + (c >> 6));
            buf_byte(b, '0' + (c >> 3 & 7));
            buf_byte(b, '0' + (c & 7));
        }
    }
    buf_byte(b, '"');
}

static void dump_expr(struct buf *out, uint d, struct expr *e);
static void dump_stmt(struct buf *out, uint d, struct stmt *s);

static void dump_init(struct buf *out, uint d, struct init *init) {
    if (init->kind == I_EXPR) {
        dump_expr(out, d, init->e);
        return;
    }
    if (init->kind == I_LIST) {
        line(out, d, init->pos, "InitList");
        for (uint i = 0; i < init->items.n; i++) {
            dump_init(out, d + 1, init->items.v[i]);
        }
        return;
    }
    struct buf t;
    memset(&t, 0, sizeof t);
    buf_str(&t, "Designated ");
    for (uint i = 0; i < init->nds; i++) {
        struct designator *ds = init->ds[i];
        if (ds->member) {
            buf_fmt(&t, ".%s", ds->name);
        } else {
            i128 v;
            buf_byte(&t, '[');
            buf_i128(&t, consteval_int(ds->index, &v) ? v : i128_of(0));
            buf_byte(&t, ']');
        }
    }
    line(out, d, init_pos(init->inner), buf_arena(&t));
    buf_free(&t);
    dump_init(out, d + 1, init->inner);
}

static void dump_declarator(struct buf *out, uint d, struct declarator *decl) {
    static const char *const storage[] = {"", "static ", "extern ", "register "};
    line(out, d, decl->pos,
         fmt("Var %s%s%s%s", storage[decl->storage], ty_declare(decl->ty, decl->name), where(decl->var),
             decl->define ? " (definition)" : ""));
    if (decl->init != NULL) {
        dump_init(out, d + 1, decl->init);
    }
}

static void dump_function(struct buf *out, struct function *f) {
    struct buf p;
    memset(&p, 0, sizeof p);
    for (uint i = 0; i < f->nparams; i++) {
        if (i > 0) {
            buf_str(&p, ", ");
        }
        buf_str(&p, ty_declare(f->params[i].ty, f->params[i].name));
    }
    if (f->variadic) {
        if (f->nparams > 0) {
            buf_str(&p, ", ");
        }
        buf_str(&p, "...");
    }
    char *params = p.len == 0 ? "void" : buf_arena(&p);
    buf_free(&p);
    line(out, 0, f->pos,
         fmt("%s %s%s%s", f->has_body ? "Function" : "Prototype", f->is_static ? "static " : "",
             f->is_inline ? "inline " : "", ty_declare(f->ret, fmt("%s(%s)", f->name, params))));
    if (!f->has_body) {
        return;
    }
    for (uint i = 0; i < f->locals.n; i++) {
        struct local *l = f->locals.v[i];
        char *text;
        if (l->temp) {
            text = fmt("Temp #%u %s (%s)", i, ty_str(l->ty), l->name);
        } else {
            text = fmt("%s #%u %s", i < f->nparams ? "Param" : "Local", i, ty_declare(l->ty, l->name));
        }
        line(out, 1, l->pos, text);
    }
    for (uint i = 0; i < f->body.n; i++) {
        dump_stmt(out, 1, f->body.v[i]);
    }
}

static void dump_stmt(struct buf *out, uint d, struct stmt *s) {
    switch (s->kind) {
    case S_BLOCK:
        line(out, d, s->pos, "Block");
        for (uint i = 0; i < s->list.n; i++) {
            dump_stmt(out, d + 1, s->list.v[i]);
        }
        break;
    case S_DECL:
        line(out, d, s->pos, "Decl");
        for (uint i = 0; i < s->list.n; i++) {
            dump_declarator(out, d + 1, s->list.v[i]);
        }
        break;
    case S_EXPR:
        line(out, d, s->pos, "ExprStmt");
        dump_expr(out, d + 1, s->e);
        break;
    case S_EMPTY:
        line(out, d, s->pos, "Empty");
        break;
    case S_UNEVALUATED:
        line(out, d, s->pos, "Unevaluated");
        for (uint i = 0; i < s->list.n; i++) {
            dump_expr(out, d + 1, ((struct unevaluated *)s->list.v[i])->expr);
        }
        break;
    case S_IF:
        line(out, d, s->pos, "If");
        dump_expr(out, d + 1, s->e);
        dump_stmt(out, d + 1, s->body);
        if (s->els != NULL) {
            line(out, d, s->els->pos, "Else");
            dump_stmt(out, d + 1, s->els);
        }
        break;
    case S_WHILE:
        line(out, d, s->pos, "While");
        dump_expr(out, d + 1, s->e);
        dump_stmt(out, d + 1, s->body);
        break;
    case S_DOWHILE:
        line(out, d, s->pos, "DoWhile");
        dump_stmt(out, d + 1, s->body);
        dump_expr(out, d + 1, s->e);
        break;
    case S_FOR:
        line(out, d, s->pos, "For");
        if (s->init != NULL) {
            dump_stmt(out, d + 1, s->init);
        } else {
            line(out, d + 1, s->pos, "NoInit");
        }
        if (s->e != NULL) {
            dump_expr(out, d + 1, s->e);
        } else {
            line(out, d + 1, s->pos, "NoCond");
        }
        if (s->step != NULL) {
            dump_expr(out, d + 1, s->step);
        } else {
            line(out, d + 1, s->pos, "NoStep");
        }
        dump_stmt(out, d + 1, s->body);
        break;
    case S_RETURN:
        line(out, d, s->pos, "Return");
        if (s->e != NULL) {
            dump_expr(out, d + 1, s->e);
        }
        break;
    case S_BREAK:
        line(out, d, s->pos, "Break");
        break;
    case S_CONTINUE:
        line(out, d, s->pos, "Continue");
        break;
    case S_SWITCH:
        line(out, d, s->pos, "Switch");
        dump_expr(out, d + 1, s->e);
        dump_stmt(out, d + 1, s->body);
        break;
    case S_CASE:
        line(out, d, s->pos, fmt("Case %s", i128_str(s->value)));
        dump_stmt(out, d + 1, s->body);
        break;
    case S_DEFAULT:
        line(out, d, s->pos, "Default");
        dump_stmt(out, d + 1, s->body);
        break;
    case S_LABEL:
        line(out, d, s->pos, fmt("Label %s", s->name));
        dump_stmt(out, d + 1, s->body);
        break;
    default:
        line(out, d, s->pos, fmt("Goto %s", s->name));
        break;
    }
}

/* The node's line: its text, then its type. */
static void node(struct buf *out, uint d, struct expr *e, const char *text) {
    line(out, d, e->pos, fmt("%s : %s", text, ty_str(e->ty)));
}

static void dump_expr(struct buf *out, uint d, struct expr *e) {
    switch (e->kind) {
    case E_NUM:
        node(out, d, e, fmt("Num %s", i128_str(e->value)));
        break;
    case E_ENUMERATOR:
        node(out, d, e, fmt("Enumerator %s = %s", e->name, i128_str(e->value)));
        break;
    case E_STR: {
        struct buf q;
        memset(&q, 0, sizeof q);
        buf_str(&q, "Str ");
        ast_quote(&q, e->bytes, e->len);
        node(out, d, e, buf_arena(&q));
        buf_free(&q);
        break;
    }
    case E_VAR:
        node(out, d, e, fmt("Var %s%s", e->name, where(e->var)));
        break;
    case E_ASSIGN:
        node(out, d, e, e->op == OP_NONE ? "Assign" : fmt("Assign %s=", binop_symbol(e->op)));
        dump_expr(out, d + 1, e->x);
        dump_expr(out, d + 1, e->y);
        break;
    case E_INCDEC:
        node(out, d, e, fmt("%s%s", e->prefix ? "Pre" : "Post", e->flag ? "++" : "--"));
        dump_expr(out, d + 1, e->x);
        break;
    case E_BINARY:
        node(out, d, e, fmt("Binary %s", binop_symbol(e->op)));
        dump_expr(out, d + 1, e->x);
        dump_expr(out, d + 1, e->y);
        break;
    case E_UNARY:
        node(out, d, e, fmt("Unary %s", unop_symbol(e->op)));
        dump_expr(out, d + 1, e->x);
        break;
    case E_ADDR:
        node(out, d, e, "Addr");
        dump_expr(out, d + 1, e->x);
        break;
    case E_DEREF:
        node(out, d, e, "Deref");
        dump_expr(out, d + 1, e->x);
        break;
    case E_CAST:
        node(out, d, e, e->implicit ? "ImplicitCast" : "Cast");
        dump_expr(out, d + 1, e->x);
        break;
    case E_SIZEOF_TYPE:
        node(out, d, e, fmt("Sizeof (%s)", ty_str(e->of)));
        break;
    case E_SIZEOF_EXPR:
        node(out, d, e, "Sizeof");
        dump_expr(out, d + 1, e->x);
        break;
    case E_COND:
        node(out, d, e, "Cond");
        dump_expr(out, d + 1, e->x);
        dump_expr(out, d + 1, e->y);
        dump_expr(out, d + 1, e->z);
        break;
    case E_COMMA:
        node(out, d, e, "Comma");
        dump_expr(out, d + 1, e->x);
        dump_expr(out, d + 1, e->y);
        break;
    case E_MEMBER:
        node(out, d, e, fmt("Member %s%s (offset %u)", e->flag ? "->" : ".", e->name, e->offset));
        dump_expr(out, d + 1, e->x);
        break;
    case E_CALL: {
        const char *callee = direct_callee(e->x);
        char *text = callee != NULL ? fmt("Call %s", callee) : "Call";
        if (e->result >= 0) {
            text = fmt("%s (result in #%u)", text, (uint)e->result);
        }
        node(out, d, e, text);
        if (callee == NULL) {
            dump_expr(out, d + 1, e->x);
        }
        for (uint i = 0; i < e->nargs; i++) {
            if (i < e->narg_copies && e->arg_copies[i] >= 0) {
                line(out, d + 1, e->args[i]->pos, fmt("ByReference (copied to #%u)", (uint)e->arg_copies[i]));
                dump_expr(out, d + 2, e->args[i]);
            } else {
                dump_expr(out, d + 1, e->args[i]);
            }
        }
        break;
    }
    case E_VA_START:
        node(out, d, e, "VaStart");
        dump_expr(out, d + 1, e->x);
        dump_expr(out, d + 1, e->y);
        break;
    case E_VA_ARG:
        node(out, d, e, "VaArg");
        dump_expr(out, d + 1, e->x);
        break;
    case E_VA_END:
        node(out, d, e, "VaEnd");
        dump_expr(out, d + 1, e->x);
        break;
    case E_VA_COPY:
        node(out, d, e, "VaCopy");
        dump_expr(out, d + 1, e->x);
        dump_expr(out, d + 1, e->y);
        break;
    default:
        node(out, d, e, e->local >= 0 ? fmt("CompoundLiteral (#%u)", (uint)e->local) : "CompoundLiteral");
        dump_init(out, d + 1, e->init);
        break;
    }
}

void ast_dump(struct program *p, struct buf *out) {
    for (uint i = 0; i < p->items.n; i++) {
        struct item *it = p->items.v[i];
        if (it->kind == ITEM_FUNC) {
            dump_function(out, it->func);
        } else {
            for (uint k = 0; k < it->decls.n; k++) {
                dump_declarator(out, 0, it->decls.v[k]);
            }
        }
    }
}
