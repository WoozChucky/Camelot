/* cc1: what of a function can run, as clang's analysis-based warnings see
 * it (follows merlin's flow.rs; clang's lib/Analysis/CFG.cpp and
 * lib/Sema/AnalysisBasedWarnings.cpp).
 *
 * clang builds a control-flow graph of each function body, leaving out the
 * edges a condition it can evaluate never takes (if (0), while (1)'s exit,
 * the operand of 0 && x or of a constant ?: that is not chosen, the cases
 * of a switch on a constant but the matching one), and asks two things of
 * it: which statements are reachable from the entry (the warnings of what
 * the code does when it runs are given only for those), and whether the end
 * of the body is reachable by falling off it, and whether a return is
 * (-Wreturn-type).
 *
 * A label is reachable if a reachable goto names it, which may come after
 * it: the walk is repeated until the labels it found reached stop growing. */
#include "cc.h"

/* Which labels of a switch it jumps to (merlin's Entry). */
enum { ENTRY_ALL, ENTRY_ONLY, ENTRY_NOTHING };
struct entry {
    int kind;
    /* ENTRY_ALL: default too, unless the switch covers its enum. */
    int with_default;
    /* ENTRY_ONLY: the case of this value, if one has it (matched), else
     * default. */
    i128 value;
    int matched;
};

/* One walk of a body, with the labels found reached so far. */
struct walk {
    struct map *labels;
    /* The labels a reachable goto names. */
    struct map gotos;
    struct intmap *live;
    int returns;
    /* For each loop or switch around, whether a reachable break leaves it
     * (as 0 or 1 in the pointer); for each loop, whether a reachable
     * continue goes on with it. */
    struct vec breaks;
    struct vec continues;
    struct vec switches;
};

/* -1 for unknown, else 0 or 1. */
static int truth(struct expr *e);

/* A comparison as x op c, with c an integer literal (through a conversion
 * and a unary minus) or an enumeration constant (clang's
 * tryTransformToIntOrEnumConstant). */
static int constant_operand(struct expr *e) {
    if (e->kind == E_CAST && e->implicit) {
        e = e->x;
    }
    if (e->kind == E_UNARY && e->op == UOP_NEG) {
        e = e->x;
    }
    return e->kind == E_NUM || e->kind == E_ENUMERATOR;
}

/* clang's tryNormalizeBinaryOperator: x op c, turning c op x round. */
static int normalized(struct expr *e, struct expr **x, int *op, struct expr **cst) {
    if (e->kind != E_BINARY || !binop_is_comparison(e->op)) {
        return 0;
    }
    if (constant_operand(e->y)) {
        *x = e->x;
        *op = e->op;
        *cst = e->y;
        return 1;
    }
    if (!constant_operand(e->x)) {
        return 0;
    }
    int o = e->op;
    *op = o == OP_LT ? OP_GT : o == OP_GT ? OP_LT : o == OP_LE ? OP_GE : o == OP_GE ? OP_LE : o;
    *x = e->y;
    *cst = e->x;
    return 1;
}

/* The enum of an enumeration constant, under conversions and minuses, by
 * merlin's Debug of it (its name); "" for none, NULL for no constant of an
 * enum at all. */
static const char *enum_kind(struct expr *e) {
    while ((e->kind == E_CAST && e->implicit) || (e->kind == E_UNARY && e->op == UOP_NEG)) {
        e = e->x;
    }
    if (e->kind != E_ENUMERATOR) {
        return NULL;
    }
    struct spelling *of = e->spelled;
    if (of == NULL) {
        return "";
    }
    return of->kind == SP_ENUM ? enum_name(of->en) : of->td->name;
}

/* Whether two constants are of a kind (areExprTypesCompatible): both
 * integer literals, or constants of one enum. */
static int same_kind(struct expr *a, struct expr *b) {
    const char *x = enum_kind(a);
    const char *y = enum_kind(b);
    if (x == NULL || y == NULL) {
        return x == y;
    }
    /* "" stands for merlin's Spelled(None), which no name equals. */
    if ((x[0] == 0) != (y[0] == 0)) {
        return 0;
    }
    return strcmp(x, y) == 0;
}

/* A constant in its own type, not the comparison's. */
static int own_value(struct expr *e, struct cval *out) {
    if (e->kind == E_CAST && e->implicit) {
        return consteval_eval(e->x, out);
    }
    return consteval_eval(e, out);
}

static int test(int op, i128 v, i128 c) {
    switch (op) {
    case OP_LT:
        return i128_lt(v, c);
    case OP_LE:
        return i128_le(v, c);
    case OP_GT:
        return i128_lt(c, v);
    case OP_GE:
        return i128_le(c, v);
    case OP_EQ:
        return i128_eq(v, c);
    default:
        return !i128_eq(v, c);
    }
}

/* clang's checkIncorrectLogicOperator: l && r or l || r of two comparisons
 * of the same variable with integer literals (or constants of one enum)
 * that are true or false for every value of it. It tries what decides
 * them: a value below both constants, each of them, one just above the
 * smaller, one above both (in the constants' type). */
static int incorrect_logic(int op, struct expr *l, struct expr *r) {
    struct expr *v1;
    struct expr *c1;
    struct expr *v2;
    struct expr *c2;
    int op1;
    int op2;
    if (!normalized(l, &v1, &op1, &c1) || !normalized(r, &v2, &op2, &c2)) {
        return -1;
    }
    if (!same_operand(v1, v2) || !same_kind(c1, c2)) {
        return -1;
    }
    struct cval x;
    struct cval y;
    if (!own_value(c1, &x) || !own_value(c2, &y) || x.is_addr || y.is_addr) {
        return -1;
    }
    if (ik_signed(x.ik) != ik_signed(y.ik) || ik_size(x.ik) != ik_size(y.ik)) {
        return -1;
    }
    i128 a = x.v;
    i128 b = y.v;
    i128 values[5];
    values[0] = ik_min(x.ik);
    values[1] = a;
    values[2] = ik_wrap(x.ik, i128_add(i128_min(a, b), i128_of(1)));
    values[3] = b;
    values[4] = ik_max(x.ik);
    int always_true = 1;
    int always_false = 1;
    for (uint i = 0; i < 5; i++) {
        int r1 = test(op1, values[i], a);
        int r2 = test(op2, values[i], b);
        int both = op == OP_LOGAND ? r1 && r2 : r1 || r2;
        always_true &= both;
        always_false &= !both;
    }
    if (always_true || always_false) {
        return always_true;
    }
    return -1;
}

/* The value clang's CFG builder gives a condition, if it can (its
 * tryEvaluateBool). Of && and ||, one operand may decide it, or, when
 * neither is known, the two comparisons of one variable with constants
 * together; otherwise, whatever constant folding gives. */
static int truth(struct expr *e) {
    if (e->kind == E_BINARY && (e->op == OP_LOGAND || e->op == OP_LOGOR)) {
        int or = e->op == OP_LOGOR;
        int a = truth(e->x);
        int b = truth(e->y);
        if (a >= 0 && a == or) {
            return a;
        }
        if (a >= 0) {
            return b;
        }
        if (b >= 0 && b == or) {
            return b;
        }
        if (b >= 0) {
            return -1;
        }
        return incorrect_logic(e->op, e->x, e->y);
    }
    i128 v;
    if (ty_is_integer(e->ty)) {
        return consteval_cfg_condition(e, &v) ? !i128_is_zero(v) : -1;
    }
    struct cval c;
    if (!consteval_eval(e, &c)) {
        return -1;
    }
    if (!c.is_addr) {
        return !i128_is_zero(c.v);
    }
    if (c.base != BASE_NONE) {
        return 1;
    }
    return !i128_is_zero(c.offset);
}

static void walk_expr(struct walk *w, struct expr *e, int live);

static void walk_init(struct walk *w, struct init *init, int live) {
    if (init->kind == I_EXPR) {
        walk_expr(w, init->e, live);
    } else if (init->kind == I_LIST) {
        for (uint i = 0; i < init->items.n; i++) {
            walk_init(w, init->items.v[i], live);
        }
    } else {
        walk_init(w, init->inner, live);
    }
}

/* An expression evaluated if live, and what in it is. */
static void walk_expr(struct walk *w, struct expr *e, int live) {
    if (live) {
        intmap_put(w->live, e->id, NULL);
    }
    switch (e->kind) {
    case E_BINARY:
        if (e->op == OP_LOGAND || e->op == OP_LOGOR) {
            walk_expr(w, e->x, live);
            int t = truth(e->x);
            int decided = e->op == OP_LOGAND ? 0 : 1;
            walk_expr(w, e->y, live && t != decided);
            return;
        }
        walk_expr(w, e->x, live);
        walk_expr(w, e->y, live);
        return;
    case E_COND: {
        walk_expr(w, e->x, live);
        int t = truth(e->x);
        walk_expr(w, e->y, live && t != 0);
        walk_expr(w, e->z, live && t != 1);
        return;
    }
    case E_COMMA:
    case E_VA_COPY:
    case E_ASSIGN:
    case E_VA_START:
        walk_expr(w, e->x, live);
        walk_expr(w, e->y, live);
        return;
    case E_INCDEC:
    case E_UNARY:
    case E_ADDR:
    case E_DEREF:
    case E_CAST:
    case E_MEMBER:
    case E_VA_ARG:
    case E_VA_END:
        walk_expr(w, e->x, live);
        return;
    case E_CALL:
        walk_expr(w, e->x, live);
        for (uint i = 0; i < e->nargs; i++) {
            walk_expr(w, e->args[i], live);
        }
        return;
    case E_COMPOUND:
        walk_init(w, e->init, live);
        return;
    default:
        /* sizeof's operand is never evaluated. */
        return;
    }
}

static int walk_stmt(struct walk *w, struct stmt *s, int live);

/* The statements of a block; whether control comes out of it. */
static int walk_block(struct walk *w, struct vec *body, int live) {
    for (uint i = 0; i < body->n; i++) {
        live = walk_stmt(w, body->v[i], live);
    }
    return live;
}

/* A loop's body: whether control comes out of its end, whether a break
 * leaves the loop (*broke), and whether a continue goes on with it
 * (*went_on). */
static int in_loop(struct walk *w, struct stmt *body, int live, int *broke, int *went_on) {
    vec_push(&w->breaks, NULL);
    vec_push(&w->continues, NULL);
    int out = walk_stmt(w, body, live);
    *went_on = vec_pop(&w->continues) != NULL;
    *broke = vec_pop(&w->breaks) != NULL;
    return out;
}

/* A statement reached if live; whether control comes out of it at its end. */
static int walk_stmt(struct walk *w, struct stmt *s, int live) {
    int broke;
    int went_on;
    int t;
    switch (s->kind) {
    case S_BLOCK:
        return walk_block(w, &s->list, live);
    case S_UNEVALUATED:
    case S_EMPTY:
        return live;
    case S_DECL:
        for (uint i = 0; i < s->list.n; i++) {
            struct declarator *d = s->list.v[i];
            if (d->init != NULL) {
                walk_init(w, d->init, live);
            }
        }
        return live;
    case S_EXPR:
        walk_expr(w, s->e, live);
        return live;
    case S_RETURN:
        if (s->e != NULL) {
            walk_expr(w, s->e, live);
        }
        w->returns |= live;
        return 0;
    case S_IF: {
        walk_expr(w, s->e, live);
        t = truth(s->e);
        int then_out = walk_stmt(w, s->body, live && t != 0);
        int else_live = live && t != 1;
        int else_out = s->els != NULL ? walk_stmt(w, s->els, else_live) : else_live;
        return then_out || else_out;
    }
    case S_WHILE:
        walk_expr(w, s->e, live);
        t = truth(s->e);
        in_loop(w, s->body, live && t != 0, &broke, &went_on);
        return (live && t != 1) || broke;
    case S_DOWHILE: {
        int out = in_loop(w, s->body, live, &broke, &went_on);
        int cond_live = out || went_on;
        walk_expr(w, s->e, cond_live);
        return (cond_live && truth(s->e) != 1) || broke;
    }
    case S_FOR: {
        if (s->init != NULL) {
            live = walk_stmt(w, s->init, live);
        }
        if (s->e != NULL) {
            walk_expr(w, s->e, live);
            t = truth(s->e);
        } else {
            t = 1;
        }
        int out = in_loop(w, s->body, live && t != 0, &broke, &went_on);
        if (s->step != NULL) {
            walk_expr(w, s->step, out || went_on);
        }
        return (live && t != 1) || broke;
    }
    case S_SWITCH: {
        walk_expr(w, s->e, live);
        i128 v;
        int known = consteval_int(s->e, &v);
        /* A switch that covers its enum takes a case or nothing: clang's CFG
         * has no edge to default, nor past the switch. */
        int matched = s->covered;
        if (!matched && known) {
            for (uint i = 0; i < s->ncases; i++) {
                if (i128_eq(s->cases[i].value, v)) {
                    matched = 1;
                }
            }
        }
        struct entry *en = arena_alloc(sizeof *en);
        if (!live) {
            en->kind = ENTRY_NOTHING;
        } else if (!known) {
            en->kind = ENTRY_ALL;
            en->with_default = !s->covered;
        } else {
            en->kind = ENTRY_ONLY;
            en->value = v;
            en->matched = matched;
        }
        vec_push(&w->switches, en);
        vec_push(&w->breaks, NULL);
        /* Nothing before the first case runs but what a label reaches. */
        int out = walk_stmt(w, s->body, 0);
        broke = vec_pop(&w->breaks) != NULL;
        vec_pop(&w->switches);
        /* With no case taken, the switch is passed by. */
        int passed = live && !s->covered && s->id < 0 && (!known || !matched);
        return out || broke || passed;
    }
    case S_CASE: {
        int entry = 0;
        if (w->switches.n > 0) {
            struct entry *en = vec_last(&w->switches);
            entry = en->kind == ENTRY_ALL || (en->kind == ENTRY_ONLY && i128_eq(en->value, s->value));
        }
        return walk_stmt(w, s->body, live || entry);
    }
    case S_DEFAULT: {
        int entry = 0;
        if (w->switches.n > 0) {
            struct entry *en = vec_last(&w->switches);
            entry = (en->kind == ENTRY_ALL && en->with_default) || (en->kind == ENTRY_ONLY && !en->matched);
        }
        return walk_stmt(w, s->body, live || entry);
    }
    case S_LABEL:
        return walk_stmt(w, s->body, live || map_get(w->labels, s->name) != NULL);
    case S_GOTO:
        if (live) {
            map_put(&w->gotos, s->name, (void *)s->name);
        }
        return 0;
    case S_BREAK:
        if (w->breaks.n > 0 && live) {
            w->breaks.v[w->breaks.n - 1] = (void *)1;
        }
        return 0;
    case S_CONTINUE:
        if (w->continues.n > 0 && live) {
            w->continues.v[w->continues.n - 1] = (void *)1;
        }
        return 0;
    default:
        die("flow: a statement of no kind");
        return 0;
    }
}

/* Walks a function's checked body (merlin's analyze): the expressions that
 * may be evaluated, by id, in *live; whether control can fall off its end,
 * and whether a return can be reached. */
int flow_analyze(struct vec *body, struct intmap *live, int *falls_off, int *returns) {
    struct map labels;
    memset(&labels, 0, sizeof labels);
    for (;;) {
        struct walk w;
        memset(&w, 0, sizeof w);
        w.labels = &labels;
        struct intmap found;
        memset(&found, 0, sizeof found);
        w.live = &found;
        int out = walk_block(&w, body, 1);
        /* Whether the gotos found are among the labels known. */
        int subset = 1;
        for (uint i = 0; i < w.gotos.cap; i++) {
            if (w.gotos.keys[i] != NULL && map_get(&labels, w.gotos.keys[i]) == NULL) {
                subset = 0;
            }
        }
        if (subset) {
            *live = found;
            *falls_off = out;
            *returns = w.returns;
            return 0;
        }
        for (uint i = 0; i < w.gotos.cap; i++) {
            if (w.gotos.keys[i] != NULL) {
                map_put(&labels, w.gotos.keys[i], (void *)w.gotos.keys[i]);
            }
        }
    }
}
