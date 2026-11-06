/* cc1: name resolution and type checking (follows merlin's sema.rs, whose
 * module comment says what it does in full).
 *
 * Every name must be declared before use, with C's block scoping; every
 * parameter and local goes in its function's locals, and every variable
 * reference and declarator says where it lives. Every expression gets its
 * C type by C99's rules with LP64 sizes; operands must make sense for their
 * operator, assignments, arguments and returns must convert as C allows,
 * and the conversions C applies there become explicit implicit casts.
 * break, continue, case and default must be where C allows them, every
 * goto must have its label; global and static initializers must be
 * constant, and of several declarations of one global one is its
 * definition. Initializers are rewritten fully braced, calls that pass or
 * return structs by value get the hidden locals the calling convention
 * needs, sizeof becomes its value, and a compound literal at file scope a
 * hidden static variable (.compoundliteral, as clang names it).
 *
 * It reports every error, in source order, and clang's warnings, in its
 * words and in its order. An expression with an error doesn't produce more
 * errors about the expressions around it: a check that can fail returns
 * nonzero once it has reported the error (merlin's Reported). */
#include "cc.h"

/* The name of the hidden variable of a file-scope compound literal. */
static const char LITERAL[] = ".compoundliteral";

/* The key of the nth symbol sema makes under the name base, the name clang
 * gives it: base itself for the first, base#n after (merlin's symbol_key). */
char *symbol_key(const char *base, uint n) {
    return n == 0 ? (char *)base : fmt("%s#%u", base, n);
}

/* The name key stands for: what comes before its `#`. */
char *symbol_base(const char *key) {
    const char *h = strchr(key, '#');
    return h == NULL ? (char *)key : arena_strndup(key, (uint)(h - key));
}

/* ---- What sema keeps ---- */

/* A parameter of a function's latest declaration: its name ("" for one it
 * leaves unnamed), where it is and how it spelled its type. */
struct sig_param {
    const char *name;
    struct pos pos;
    struct spelling *spelled;
};

/* What the file's declarations say of a global name (merlin's Global, a
 * FuncSig or a GlobalVar). */
struct global {
    int is_func;
    /* Its type: a function's (TY_FUNC), or a variable's. */
    struct type *ty;
    int is_static;
    /* Where the latest declaration is. */
    struct pos pos;
    struct spelling *spelled;
    /* Declared at file scope (not only in a block). */
    int visible;
    /* A function: defined, every declaration says inline (without extern),
     * where the definition is, the latest declaration's parameters, the
     * type of a definition `f()`. */
    int defined;
    int inline_only;
    int has_def;
    struct pos def;
    struct sig_param *params;
    uint nparams;
    struct type *bare_def;
    /* A variable: where its latest declaration begins, whether that is a
     * definition (not extern), the declarator with an initializer (item,
     * declarator) and where it is, the first that is not extern, and the
     * type as of the latest that is not extern. */
    struct pos begin;
    int defines;
    int has_init;
    uint init_item;
    uint init_decl;
    int has_init_pos;
    struct pos init_pos;
    int has_tentative;
    uint tent_item;
    uint tent_decl;
    struct type *tentative_ty;
};

/* What a block declared with linkage: its name, type, place, spelling. */
struct linked {
    const char *name;
    struct type *ty;
    struct pos pos;
    struct spelling *spelled;
};

/* The file's checker (merlin's Checker). */
struct checker {
    /* name -> struct global *. */
    struct map globals;
    /* struct diag *. */
    struct vec errors;
    /* Where clang reports each error the preprocessor and the lexer found. */
    struct pos *earlier;
    uint nearlier;
    /* Where a syntax error cut the program short, if one did. */
    int has_cut;
    struct pos cut;
    /* Where in errors the diagnostics clang gives after it begin, or -1. */
    int late;
    /* struct declarator *: the objects of file-scope compound literals. */
    struct vec literals;
    struct written written;
};

/* A name in a block scope: where it lives, its type, where it is declared. */
struct binding {
    struct var var;
    struct type *ty;
    struct pos pos;
};

/* A case of a switch (merlin's CaseLabel). */
struct case_label {
    i128 value;
    uint id;
    struct pos begin;
    struct pos loc;
    /* The enumeration constant it names, or NULL. */
    const char *name;
};

/* The case labels of a switch being checked (merlin's SwitchCtx). */
struct switch_ctx {
    int kind;
    /* struct case_label *. */
    struct vec labels;
    int incomplete;
    int has_default;
    uint default_id;
    struct pos default_pos;
};

/* A warning of what runs, given at the end of the function if the
 * expression it is of can run. */
struct pending {
    struct diag *d;
    uint64_t of;
};

/* A conversion's warning kept to come with its declaration's list. */
struct deferred {
    struct diag *d;
    int has_of;
    uint64_t of;
};

/* A goto, checked at the function's end. */
struct goto_ref {
    const char *name;
    struct pos pos;
};

/* The sizeof sema made a constant (merlin's SizeofOf). */
struct sizeof_of;

/* The checker of a function's body, or of what is checked at file scope
 * (merlin's FnChecker, whose comments explain each field). */
struct fnc {
    struct checker *c;
    uint list_depth;
    int subscript;
    int subscript_add;
    /* id -> char *: the lists of the compound literals, as written. */
    struct intmap literal_texts;
    struct posmap param_pos;
    /* pos -> struct pos *. */
    struct posmap begins;
    /* id -> struct sizeof_of *. */
    struct intmap sizeofs;
    /* pos -> struct type *. */
    struct posmap array_params;
    /* pos -> struct spelling * (NULL for none). */
    struct posmap spellings;
    struct spelling *ret_spelled;
    struct spelling *init_spelled;
    struct spelling *item_spelled;
    /* "" at file scope. */
    const char *func;
    struct type *ret;
    /* NULL for none. */
    const char *last_param;
    /* struct map * (name -> struct binding *), innermost last. */
    struct vec scopes;
    /* struct local *. */
    struct vec locals;
    uint loop_depth;
    uint break_depth;
    /* struct switch_ctx *. */
    struct vec switches;
    uint next_case;
    /* name -> struct pos *. */
    struct map labels;
    /* struct goto_ref *. */
    struct vec gotos;
    struct map static_labels;
    /* struct declarator *. */
    struct vec literals;
    uint literal_base;
    int constant;
    int address_taken;
    struct vec *errors;
    /* struct pending *. */
    struct vec pending;
    int has_conversions;
    /* struct deferred *. */
    struct vec conversions;
    uint unevaluated;
    /* Local ids declared register. */
    struct intmap registers;
    /* struct linked *. */
    struct vec linked;
    struct posmap invalid;
};

static struct diag *err_at(struct fnc *c, struct pos pos, const char *msg) {
    struct diag *d = diag_error(pos, msg);
    vec_push(c->errors, d);
    return d;
}

/* An error, reported: what a failing check returns. */
static int fail(struct fnc *c, struct pos pos, const char *msg) {
    err_at(c, pos, msg);
    return 1;
}

static void push(struct fnc *c, struct diag *d) {
    vec_push(c->errors, d);
}

static struct fnc *fnc_new(struct checker *ck, struct vec *errors) {
    struct fnc *c = heap_calloc(1, sizeof(struct fnc));
    c->c = ck;
    c->errors = errors;
    c->func = "";
    c->ret = TY_VOID_T;
    return c;
}

/* A checker for global initializers, outside any function. */
static struct fnc *file_scope(struct checker *ck) {
    struct fnc *c = fnc_new(ck, &ck->errors);
    c->literal_base = ck->literals.n;
    return c;
}

static struct global *global_of(struct fnc *c, const char *name) {
    return map_get(&c->c->globals, name);
}

/* ---- Helpers of the tree ---- */

/* The type of l op r for the arithmetic, bitwise and shift operators, with
 * both operands already decayed; NULL if C doesn't allow the operands
 * (merlin's arith_type, which code generation uses too). */
struct type *arith_type(int op, struct type *l, struct type *r) {
    struct type *lu = ty_unqual(l);
    struct type *ru = ty_unqual(r);
    if (lu->kind == TY_INT && ru->kind == TY_INT) {
        return ty_int(op == OP_SHL || op == OP_SHR ? ik_promoted(lu->ik) : ik_common(lu->ik, ru->ik));
    }
    /* Pointer arithmetic needs to know the size of what is pointed to. */
    if ((op == OP_ADD || op == OP_SUB) && lu->kind == TY_PTR && ru->kind == TY_INT && ty_step(l) >= 0) {
        return l;
    }
    if (op == OP_ADD && lu->kind == TY_INT && ru->kind == TY_PTR && ty_step(r) >= 0) {
        return r;
    }
    /* The difference of two pointers counts elements: ptrdiff_t. */
    if (op == OP_SUB && lu->kind == TY_PTR && ru->kind == TY_PTR &&
        ty_compatible(ty_without_quals(lu->to), ty_without_quals(ru->to)) && ty_step(l) >= 0) {
        return ty_int(ik_ptrdiff_t());
    }
    return NULL;
}

/* The type of e if it is a compound literal, or a member of one, before
 * sema has checked it. */
static struct type *literal_type_of(struct expr *e) {
    if (e->kind == E_COMPOUND) {
        return e->ty;
    }
    if (e->kind == E_MEMBER && !e->flag) {
        struct type *outer = literal_type_of(e->x);
        struct record *r = outer != NULL ? ty_record(outer) : NULL;
        if (r == NULL || r->body == NULL) {
            return NULL;
        }
        struct member *m = record_member(r->body, e->name);
        if (m == NULL) {
            return NULL;
        }
        return ty_qualified(m->ty, ty_quals(outer));
    }
    return NULL;
}

/* Where an expression begins, as clang points at it. */
static struct pos begin_of(struct expr *e) {
    switch (e->kind) {
    case E_BINARY:
    case E_COMMA:
    case E_COND:
    case E_ASSIGN:
    case E_MEMBER:
    case E_CALL:
        return begin_of(e->x);
    case E_INCDEC:
        return e->prefix ? e->pos : begin_of(e->x);
    case E_DEREF:
        /* a[i] is *(a + i), at the `[`. */
        return pos_eq(e->x->pos, e->pos) ? begin_of(e->x) : e->pos;
    case E_CAST:
        return e->implicit ? begin_of(e->x) : e->pos;
    default:
        return e->pos;
    }
}

/* Where clang reports an expression (its getExprLoc): at its operator, a
 * member at its name, anything else where it begins. */
static struct pos expr_loc(struct expr *e) {
    switch (e->kind) {
    case E_CAST:
        return e->implicit ? begin_of(e->x) : e->pos;
    case E_DEREF:
        return pos_eq(e->x->pos, e->pos) ? begin_of(e) : e->pos;
    case E_BINARY:
    case E_COMMA:
    case E_ASSIGN:
    case E_INCDEC:
    case E_UNARY:
    case E_ADDR:
        return e->pos;
    case E_MEMBER:
        return e->at;
    default:
        return begin_of(e);
    }
}

/* The operands of e, in order. */
static uint children(struct expr *e, struct expr ***out) {
    static struct expr *buf[3];
    switch (e->kind) {
    case E_BINARY:
    case E_COMMA:
    case E_VA_COPY:
    case E_ASSIGN:
    case E_VA_START:
        buf[0] = e->x;
        buf[1] = e->y;
        *out = buf;
        return 2;
    case E_COND:
        buf[0] = e->x;
        buf[1] = e->y;
        buf[2] = e->z;
        *out = buf;
        return 3;
    case E_INCDEC:
    case E_UNARY:
    case E_ADDR:
    case E_DEREF:
    case E_CAST:
    case E_MEMBER:
    case E_VA_ARG:
    case E_VA_END:
        buf[0] = e->x;
        *out = buf;
        return 1;
    case E_CALL: {
        struct expr **all = arena_alloc((e->nargs + 1) * sizeof(struct expr *));
        all[0] = e->x;
        for (uint i = 0; i < e->nargs; i++) {
            all[i + 1] = e->args[i];
        }
        *out = all;
        return e->nargs + 1;
    }
    default:
        *out = buf;
        return 0;
    }
}

/* The operands of e, in a vector of their own. */
static struct vec child_vec(struct expr *e) {
    struct expr **k;
    uint n = children(e, &k);
    struct vec v;
    memset(&v, 0, sizeof v);
    for (uint i = 0; i < n; i++) {
        vec_push(&v, k[i]);
    }
    return v;
}

/* Whether evaluating the constant e overflows a signed type somewhere. */
static int out_of_type(i128 v, struct type *t) {
    int k = ty_int_kind(t);
    return k >= 0 && ik_signed(k) && (i128_lt(v, ik_min(k)) || i128_lt(ik_max(k), v));
}

static int overflows(struct expr *e) {
    int here = 0;
    i128 a;
    i128 b;
    if (e->kind == E_BINARY && consteval_int(e->x, &a) && consteval_int(e->y, &b)) {
        if (e->op == OP_ADD) {
            here = out_of_type(i128_add(a, b), e->ty);
        } else if (e->op == OP_SUB) {
            here = out_of_type(i128_sub(a, b), e->ty);
        } else if (e->op == OP_MUL) {
            here = out_of_type(i128_mul(a, b), e->ty);
        }
    } else if (e->kind == E_UNARY && e->op == UOP_NEG && consteval_int(e->x, &a)) {
        here = out_of_type(i128_neg(a), e->ty);
    }
    if (here) {
        return 1;
    }
    struct vec k = child_vec(e);
    int any = 0;
    for (uint i = 0; i < k.n && !any; i++) {
        any = overflows(k.v[i]);
    }
    heap_free(k.v);
    return any;
}

/* Of pointers to a and b, whether they are the signed and unsigned type of
 * one integer rank (clang's -Wpointer-sign): -1 if not, else whether one
 * of them is plain char. */
static int sign_rank(int k) {
    switch (k) {
    case IK_CHAR:
    case IK_SCHAR:
    case IK_UCHAR:
        return 0;
    case IK_SHORT:
    case IK_USHORT:
        return 1;
    case IK_INT:
    case IK_UINT:
        return 2;
    case IK_LONG:
    case IK_ULONG:
        return 3;
    case IK_LLONG:
    case IK_ULLONG:
        return 4;
    default:
        return -1;
    }
}

static int sign_pair(struct type *a, struct type *b) {
    int x = ty_int_kind(a);
    int y = ty_int_kind(b);
    if (x < 0 || y < 0 || x == y || sign_rank(x) < 0 || sign_rank(y) < 0 || sign_rank(x) != sign_rank(y)) {
        return -1;
    }
    return x == IK_CHAR || y == IK_CHAR;
}

/* Whether pointers to a and b, as sa and sb spelled them, point to
 * different types because of an enum, which merlin's types leave out. */
static int enums_differ(struct type *a, struct spelling *sa, struct type *b, struct spelling *sb) {
    struct enum_type *x = enum_of(sa, a);
    struct enum_type *y = enum_of(sb, b);
    if (x != NULL && y != NULL) {
        return x != y;
    }
    if (x == NULL && y == NULL) {
        return 0;
    }
    struct enum_type *one = x != NULL ? x : y;
    return ty_is_unsigned(one->ty) && !ty_eq(ty_unqual(a), ty_unqual(b)) && ty_is_integer(b) && ty_is_integer(a);
}

/* Whether e surely changes something when evaluated (clang's
 * HasSideEffects without what may: a call). */
static int side_effects(struct expr *e) {
    switch (e->kind) {
    case E_ASSIGN:
    case E_INCDEC:
    case E_VA_START:
    case E_VA_ARG:
    case E_VA_END:
    case E_VA_COPY:
        return 1;
    case E_BINARY:
    case E_COMMA:
        return side_effects(e->x) || side_effects(e->y);
    case E_COND:
        return side_effects(e->x) || side_effects(e->y) || side_effects(e->z);
    case E_UNARY:
    case E_ADDR:
    case E_DEREF:
    case E_CAST:
    case E_MEMBER:
        return side_effects(e->x);
    case E_CALL:
        if (side_effects(e->x)) {
            return 1;
        }
        for (uint i = 0; i < e->nargs; i++) {
            if (side_effects(e->args[i])) {
                return 1;
            }
        }
        return 0;
    default:
        return 0;
    }
}

/* n as clang writes a value in a message, its digits grouped by threes. */
static char *digit_groups(i128 n) {
    int neg = i128_neg_p(n);
    char *digits = i128_str(neg ? i128_neg(n) : n);
    uint len = (uint)strlen(digits);
    struct buf b;
    memset(&b, 0, sizeof b);
    if (neg) {
        buf_byte(&b, '-');
    }
    for (uint i = 0; i < len; i++) {
        if (i > 0 && (len - i) % 3 == 0) {
            buf_byte(&b, '\'');
        }
        buf_byte(&b, (uchar)digits[i]);
    }
    char *r = buf_arena(&b);
    buf_free(&b);
    return r;
}

/* clang's warning of the `;` at pos that is the empty body of what. */
static struct diag *empty_body_warning(struct pos pos, const char *what) {
    return diag_note(diag_warning(pos, fmt("%s has empty body", what), "empty-body"), pos,
                     "put the semicolon on a separate line to silence this warning");
}

static struct init *init_list_new(struct pos pos) {
    struct init *i = arena_alloc(sizeof *i);
    i->kind = I_LIST;
    i->pos = pos;
    return i;
}

static struct init *init_expr_new(struct expr *e) {
    struct init *i = arena_alloc(sizeof *i);
    i->kind = I_EXPR;
    i->e = e;
    return i;
}

/* The initializer of a member a braced list leaves out: zero. */
static struct init *zero_init(struct type *ty, struct pos pos) {
    if (ty_is_scalar(ty)) {
        return init_expr_new(expr_num(i128_of(0), ty_unqual(ty), pos));
    }
    return init_list_new(pos);
}

/* Whether e designates an object (C's lvalue). */
static int is_lvalue(struct expr *e) {
    switch (e->kind) {
    case E_VAR:
    case E_DEREF:
    case E_STR:
    case E_COMPOUND:
        return 1;
    case E_MEMBER:
        return e->flag || is_lvalue(e->x);
    default:
        return 0;
    }
}

/* Wraps e in an implicit conversion to `to`, unless it already has that
 * type (qualifiers aside: a value has none). In place. */
void coerce(struct expr *e, struct type *to) {
    to = ty_unqual(to);
    if (ty_eq(ty_unqual(e->ty), to)) {
        return;
    }
    struct expr *inner = arena_alloc(sizeof *inner);
    *inner = *e;
    /* merlin makes a placeholder (an id) and then the cast (another). */
    expr_fresh_id();
    memset(e, 0, sizeof *e);
    e->kind = E_CAST;
    e->implicit = 1;
    e->x = inner;
    e->ty = to;
    e->pos = inner->pos;
    e->id = expr_fresh_id();
    e->result = -1;
    e->local = -1;
}

/* clang's note on an earlier declaration a new one clashes with. */
static const char *previous_what(int definition) {
    return definition ? "previous definition is here" : "previous declaration is here";
}

/* clang's error for a call with too many or too few arguments for the
 * function type ft, if it has (merlin's arity_error). A function called by
 * name (sig, its name) gets clang's note on its declaration. */
static struct diag *arity_error(struct fnc *c, struct functype *ft, struct expr **args, uint have, const char *name,
                                struct global *sig, struct pos close, struct pos (*begin)(struct fnc *, struct expr *)) {
    uint arity = ft->nparams;
    /* The one parameter of a function called by name, if it is named. */
    const char *single = NULL;
    if (sig != NULL && sig->nparams > 0 && sig->params[0].name[0] != 0) {
        single = sig->params[0].name;
    }
    struct diag *e;
    if (have > arity && !ft->variadic) {
        const char *msg;
        if (single != NULL && arity == 1) {
            msg = fmt("too many arguments to function call, expected single argument '%s', have %u arguments", single,
                      have);
        } else {
            msg = fmt("too many arguments to function call, expected %u, have %u", arity, have);
        }
        e = diag_error(begin(c, args[arity]), msg);
    } else if (have < arity) {
        const char *msg;
        if (single != NULL && arity == 1 && !ft->variadic) {
            msg = fmt("too few arguments to function call, single argument '%s' was not specified", single);
        } else if (single != NULL && arity == 1) {
            msg = fmt("too few arguments to function call, at least argument '%s' must be specified", single);
        } else if (!ft->variadic) {
            msg = fmt("too few arguments to function call, expected %u, have %u", arity, have);
        } else {
            msg = fmt("too few arguments to function call, expected at least %u, have %u", arity, have);
        }
        e = diag_error(close, msg);
    } else {
        return NULL;
    }
    if (sig != NULL) {
        diag_note(e, sig->pos, fmt("'%s' declared here", name));
    }
    return e;
}

/* Whether two declarations agree: compatible types, of the same enums. */
static int agree(struct type *a, struct spelling *sa, struct type *b, struct spelling *sb) {
    return ty_compatible(a, b) && !distinct_enums(a, sa, b, sb);
}

/* A function type declared with (), which says nothing of the parameters. */
static int unprototyped(struct functype *f) {
    return f->unprototyped && f->nparams == 0 && !f->variadic;
}

/* clang's warning of two declarations of a function, one without a
 * prototype and one with parameters (earlier at at, later at bt and whether
 * it is a definition): C23 makes them conflict. */
static struct diag *deprecated_conflict(struct functype *a, struct pos at, struct functype *b, struct pos bt,
                                        int defined) {
    const char *bare = "a function declaration without a prototype is deprecated in all versions of C and is "
                       "treated as a zero-parameter prototype in C23, conflicting with a";
    const char *msg;
    struct pos here;
    struct pos there;
    if (unprototyped(a) && !unprototyped(b) && b->nparams > 0) {
        msg = fmt("%s subsequent %s", bare, defined ? "definition" : "declaration");
        here = at;
        there = bt;
    } else if (unprototyped(b) && !unprototyped(a) && a->nparams > 0) {
        msg = fmt("%s previous declaration", bare);
        here = bt;
        there = at;
    } else {
        return NULL;
    }
    return diag_note(diag_warning(here, msg, "deprecated-non-prototype"), there, "conflicting prototype is here");
}

/* A copy of the function type t, its own to change. */
static struct type *func_copy(struct type *t) {
    struct type *c = ty_func(t->fn->ret, t->fn->params, t->fn->nparams, t->fn->variadic);
    c->fn->unprototyped = t->fn->unprototyped;
    c->fn->spelled = t->fn->spelled;
    c->fn->nspelled = t->fn->nspelled;
    return c;
}

/* Whether two function types are compatible, of the same result's enum. */
static int functions_compatible(struct type *a, struct spelling *sa, struct type *b, struct spelling *sb) {
    return agree(a, sa, b, sb);
}

/* ---- Forward declarations ---- */

static int check_expr(struct fnc *c, struct expr *e);
static int value(struct fnc *c, struct expr *e, int constant);
static void full_expression(struct fnc *c, struct expr *e);
static void bounds(struct fnc *c, struct expr *e);
static void overflow(struct fnc *c, struct expr *e);
static struct spelling *spelled_of(struct fnc *c, struct expr *e);
static int literal_type(struct fnc *c, struct type *ty, struct pos pos);
static void incomplete_variable(struct fnc *c, struct pos pos, struct type *ty);
/* compared: -1 for none, else whether the comparison is != (merlin's
 * Option<bool>). */
static struct diag *never_null(struct fnc *c, struct expr *e, int compared);
static void out_of_range(struct fnc *c, int op, struct pos pos, uint64_t id, struct expr *l, struct expr *r);
static void stack_address(struct fnc *c, struct expr *e, struct vec *found);
static void constant_conversion(struct fnc *c, struct expr *e, struct type *to);
static int is_char_array(struct type *t);
static int is_char_array_elem(struct type *t);
static struct expr *strip_implicit(struct expr *e);
static void block(struct fnc *c, struct vec *body);
static int init_check(struct fnc *c, struct type **ty, struct init **init, int constant, int has_decl,
                      struct pos decl);
static int init_expr(struct fnc *c, struct type **ty, struct expr *e, int constant, int checked, int has_decl,
                     struct pos decl);
static struct pos begin_written(struct fnc *c, struct expr *e);
static void unevaluated_list(struct fnc *c, struct vec *us);

/* ---- Scopes ---- */

static struct local *local_new(const char *name, struct type *ty, struct pos pos, int temp) {
    struct local *l = arena_alloc(sizeof *l);
    l->name = name;
    l->ty = ty;
    l->pos = pos;
    l->temp = (uchar)temp;
    return l;
}

static void bind_local(struct fnc *c, const char *name, struct var var, struct type *ty, struct pos pos, const char *what) {
    /* An unnamed parameter is no name. */
    if (name[0] == 0) {
        return;
    }
    struct map *scope = vec_last(&c->scopes);
    struct binding *prev = map_get(scope, name);
    struct binding *b = arena_alloc(sizeof *b);
    b->var = var;
    b->ty = ty;
    b->pos = pos;
    map_put(scope, name, b);
    if (prev != NULL) {
        /* clang's note on a parameter is of its declaration, but a local of
         * the body's outermost block clashes with its definition. */
        struct diag *e = diag_error(pos, fmt("redefinition of %s'%s'", what, name));
        push(c, diag_note(e, prev->pos, previous_what(what[0] == 0)));
    }
}

static uint declare(struct fnc *c, const char *name, struct type *ty, struct pos pos, const char *what) {
    uint id = c->locals.n;
    vec_push(&c->locals, local_new(name, ty, pos, 0));
    struct var v;
    v.kind = VAR_LOCAL;
    v.local = id;
    v.sym = NULL;
    bind_local(c, name, v, ty, pos, what);
    return id;
}

/* A hidden local for the calling convention: what says what it holds. */
static uint temp(struct fnc *c, struct type *ty, const char *what, struct pos pos) {
    uint id = c->locals.n;
    vec_push(&c->locals, local_new(what, ty, pos, 1));
    return id;
}

static struct binding *lookup(struct fnc *c, const char *name) {
    for (uint i = c->scopes.n; i > 0; i--) {
        struct binding *b = map_get(c->scopes.v[i - 1], name);
        if (b != NULL) {
            return b;
        }
    }
    return NULL;
}

static void push_scope(struct fnc *c) {
    vec_push(&c->scopes, arena_alloc(sizeof(struct map)));
}

static void pop_scope(struct fnc *c) {
    vec_pop(&c->scopes);
}

/* A file-scope declaration of name this checker sees: not one only a block
 * declared. */
static struct global *visible_global(struct fnc *c, const char *name) {
    struct global *g = global_of(c, name);
    return g != NULL && g->visible ? g : NULL;
}

/* A warning of what the code does when it runs, which clang gives at the
 * function's end if the expression it is of can run; never for what is not
 * evaluated. */
static void runtime(struct fnc *c, struct diag *d, uint64_t of) {
    if (c->unevaluated == 0) {
        struct pending *p = arena_alloc(sizeof *p);
        p->d = d;
        p->of = of;
        vec_push(&c->pending, p);
    }
}

/* ---- The file ---- */

static void check_function(struct checker *ck, struct function *f);
static void global_var(struct checker *ck, struct declarator *d, uint item, uint decl);
static void mark_definitions(struct checker *ck, struct program *prog);

/* Checks the file-scope expressions the parser evaluated away that come
 * before item `before`, from `from` on; returns where the rest start. */
static uint file_unevaluated_before(struct checker *ck, struct vec *us, uint from, uint before) {
    uint to = from;
    while (to < us->n && ((struct file_unevaluated *)us->v[to])->after <= before) {
        to++;
    }
    if (to > from) {
        struct fnc *c = file_scope(ck);
        struct vec list;
        memset(&list, 0, sizeof list);
        for (uint i = from; i < to; i++) {
            vec_push(&list, ((struct file_unevaluated *)us->v[i])->u);
        }
        unevaluated_list(c, &list);
        heap_free(list.v);
    }
    return to;
}

/* Checks what the parser evaluated away of a declarator or function at file
 * scope. */
static void file_unevaluated(struct checker *ck, struct vec *us) {
    if (us->n > 0) {
        unevaluated_list(file_scope(ck), us);
    }
}

/* Checks the program (merlin's checked): every diagnostic, in order, in
 * *diags; with a syntax error at cut, where those clang gives after it
 * begin (or -1). */
static int checked(struct program *prog, struct pos *earlier, uint nearlier, int has_cut, struct pos cut,
                   struct vec *diags) {
    struct checker *ck = heap_calloc(1, sizeof(struct checker));
    ck->earlier = earlier;
    ck->nearlier = nearlier;
    ck->has_cut = has_cut;
    ck->cut = cut;
    ck->late = -1;
    ck->written = prog->written;
    memset(&prog->written, 0, sizeof prog->written);
    /* What the parser evaluated away at file scope is checked where it
     * was, after the items before it. */
    struct vec unevaluated = vec_take(&prog->unevaluated);
    uint next = 0;
    for (uint i = 0; i < prog->items.n; i++) {
        struct item *it = prog->items.v[i];
        next = file_unevaluated_before(ck, &unevaluated, next, i);
        if (it->kind == ITEM_FUNC) {
            file_unevaluated(ck, &it->func->unevaluated);
            check_function(ck, it->func);
        } else {
            for (uint j = 0; j < it->decls.n; j++) {
                struct declarator *d = it->decls.v[j];
                /* A declarator's own before it is declared, its
                 * initializer's after. */
                file_unevaluated(ck, &d->sized);
                global_var(ck, d, i, j);
                file_unevaluated(ck, &d->unevaluated);
            }
        }
    }
    file_unevaluated_before(ck, &unevaluated, next, 0xffffffffu);
    prog->unevaluated = unevaluated;
    /* What clang finds at the end of the file it finds after a syntax
     * error. */
    if (ck->has_cut && ck->late < 0) {
        ck->late = (int)ck->errors.n;
    }
    mark_definitions(ck, prog);
    /* File-scope compound literals: the objects their expressions name. */
    if (ck->literals.n > 0) {
        struct item *it = arena_alloc(sizeof *it);
        it->kind = ITEM_DECL;
        it->decls = vec_take(&ck->literals);
        vec_push(&prog->items, it);
    }
    prog->written = ck->written;
    /* Whether a definition is an inline definition depends on every
     * declaration of the function, so it is decided at the end. */
    for (uint i = 0; i < prog->items.n; i++) {
        struct item *it = prog->items.v[i];
        if (it->kind == ITEM_FUNC) {
            struct global *g = map_get(&ck->globals, it->func->name);
            if (g != NULL && g->is_func) {
                it->func->inline_only = it->func->has_body && !it->func->is_static && g->inline_only;
            }
        }
    }
    *diags = ck->errors;
    return ck->late;
}

/* merlin's check_after: 0 with the warnings added to the program's, or 1
 * with every diagnostic in *errors. */
int sema_check(struct program *prog, struct pos *earlier, uint nearlier, struct vec *errors) {
    struct vec diags;
    struct pos none = pos_at(0, 0, 0);
    checked(prog, earlier, nearlier, 0, none, &diags);
    for (uint i = 0; i < diags.n; i++) {
        if (!diag_is_warning(diags.v[i])) {
            *errors = diags;
            return 1;
        }
    }
    vec_extend(&prog->warnings, &diags);
    return 0;
}

/* merlin's check_cut: of what the parser read before a syntax error at cut,
 * the diagnostics clang gives before that error (*before) and after it
 * (*after). */
void sema_check_cut(struct program *prog, struct pos *earlier, uint nearlier, struct pos cut, struct vec *before,
                    struct vec *after) {
    struct vec diags;
    int late = checked(prog, earlier, nearlier, 1, cut, &diags);
    *after = vec_split_off(&diags, late < 0 ? diags.n : (uint)late);
    *before = diags;
}

/* The params of a function's latest declaration. */
static struct sig_param *sig_params(struct function *f) {
    struct sig_param *ps = arena_alloc(f->nparams * sizeof(struct sig_param) + 1);
    for (uint i = 0; i < f->nparams; i++) {
        ps[i].name = f->params[i].name;
        ps[i].pos = f->params[i].pos;
        ps[i].spelled = f->params[i].spelled;
    }
    return ps;
}

static int init_object(struct fnc *c, struct type **ty, struct init **init, int constant, struct pos at);

static void incomplete_param(struct fnc *c, struct param *p) {
    struct diag *e = diag_error(p->pos, fmt("variable has incomplete type '%s'", ty_str(p->ty)));
    struct record *r = ty_record(p->ty);
    if (r != NULL) {
        diag_note(e, r->pos, fmt("forward declaration of '%s'", record_name(r)));
    }
    push(c, e);
}

static void check_function(struct checker *ck, struct function *f) {
    struct type *ty = func_copy(function_ty(f));
    ty->fn->unprototyped = (uchar)!f->prototyped;
    struct global *sig = arena_alloc(sizeof *sig);
    sig->is_func = 1;
    sig->ty = ty;
    sig->is_static = f->is_static;
    sig->defined = f->has_body;
    sig->inline_only = f->is_inline && !f->is_extern;
    sig->pos = f->pos;
    sig->has_def = f->has_body;
    sig->def = f->pos;
    sig->params = sig_params(f);
    sig->nparams = f->nparams;
    sig->spelled = f->ret_spelled;
    /* A definition with (), which takes no arguments. */
    sig->bare_def = f->has_body && !f->prototyped ? ty : NULL;
    sig->visible = 1;
    struct global *prev = map_get(&ck->globals, f->name);
    if (prev != NULL && !prev->is_func) {
        /* clang's note on a declaration of another kind is always this. */
        struct diag *e = diag_error(f->pos, fmt("redefinition of '%s' as different kind of symbol", f->name));
        vec_push(&ck->errors, diag_note(e, prev->pos, previous_what(1)));
    } else if (prev != NULL) {
        /* clang's note names the latest declaration before this one, or the
         * definition when this is one more. */
        int note_def = prev->has_def && pos_eq(prev->def, prev->pos);
        /* One declaration without a prototype and one with parameters:
         * what C23 makes a conflict, clang warns of. Not of a definition
         * f(), which conflicts outright. */
        if (!(sig->defined && unprototyped(sig->ty->fn))) {
            struct diag *w = deprecated_conflict(prev->ty->fn, prev->pos, sig->ty->fn, f->pos, sig->defined);
            if (w != NULL) {
                vec_push(&ck->errors, w);
            }
        }
        /* A definition with () has no parameters, and a prototype before it
         * must agree in their number; one after it is compatible with its
         * type if its parameters promote to themselves. */
        struct functype *pt = prev->ty->fn;
        int takes = fn_has_prototype(pt) && (pt->nparams > 0 || pt->variadic);
        const char *msg = NULL;
        struct pos note_at = prev->pos;
        const char *note_what = previous_what(note_def);
        if (!functions_compatible(prev->ty, prev->spelled, sig->ty, sig->spelled) ||
            (sig->defined && unprototyped(sig->ty->fn) && takes)) {
            msg = fmt("conflicting types for '%s'", f->name);
        } else if (prev->has_def && sig->defined) {
            msg = fmt("redefinition of '%s'", f->name);
            note_at = prev->def;
            note_what = previous_what(1);
        } else if (sig->is_static && !prev->is_static) {
            msg = fmt("static declaration of '%s' follows non-static declaration", f->name);
        }
        if (msg != NULL) {
            vec_push(&ck->errors, diag_note(diag_error(f->pos, msg), note_at, note_what));
        } else {
            /* The type the declarations make together. */
            struct type *comp = ty_composite(prev->ty, sig->ty);
            if (comp->kind == TY_FUNC) {
                prev->ty = comp;
            }
            if (prev->bare_def == NULL) {
                prev->bare_def = sig->bare_def;
            }
            prev->defined |= sig->defined;
            if (!prev->has_def && sig->has_def) {
                prev->has_def = 1;
                prev->def = sig->def;
            }
            /* Of declarations in blocks only, none said inline. */
            prev->inline_only = (prev->inline_only || !prev->visible) && sig->inline_only;
            prev->pos = sig->pos;
            prev->params = sig->params;
            prev->nparams = sig->nparams;
            prev->spelled = sig->spelled;
            prev->visible = 1;
        }
    } else {
        map_put(&ck->globals, f->name, sig);
    }
    /* A static function keeps that from its first declaration on. */
    struct global *g = map_get(&ck->globals, f->name);
    if (g != NULL && g->is_func) {
        f->is_static = (uchar)g->is_static;
    }
    if (!f->has_body) {
        return;
    }
    struct fnc *c = fnc_new(ck, &ck->errors);
    /* A definition needs complete parameter and result types. */
    for (uint i = 0; i < f->nparams; i++) {
        struct param *p = &f->params[i];
        if (ty_record(p->ty) != NULL && ty_size(p->ty) < 0) {
            incomplete_param(c, p);
        }
    }
    int ret_complete = ty_record(f->ret) == NULL || ty_size(f->ret) >= 0;
    if (!ret_complete) {
        struct diag *e =
            diag_error(f->pos, fmt("incomplete result type '%s' in function definition", ty_str(f->ret)));
        struct record *r = ty_record(f->ret);
        if (r != NULL) {
            diag_note(e, r->pos, fmt("forward declaration of '%s'", record_name(r)));
        }
        push(c, e);
    }
    for (uint i = 0; i < f->nparams; i++) {
        posmap_put(&c->spellings, f->params[i].pos, f->params[i].spelled);
    }
    c->ret_spelled = f->ret_spelled;
    c->func = f->name;
    /* What return converts to: the value, unqualified. */
    c->ret = ty_unqual(f->ret);
    if (f->variadic && f->nparams > 0) {
        c->last_param = f->params[f->nparams - 1].name;
    }
    push_scope(c);
    for (uint i = 0; i < f->nparams; i++) {
        struct param *p = &f->params[i];
        uint id = declare(c, p->name, p->ty, p->pos, "parameter ");
        if (p->array != NULL) {
            posmap_put(&c->array_params, p->pos, p->array);
        }
        posmap_put(&c->param_pos, p->pos, NULL);
        if (p->register_) {
            intmap_put(&c->registers, id, NULL);
        }
    }
    /* A big struct result goes where the caller says: keep that address. */
    f->ret_ptr = ret_complete && abi_returns_in_memory(f->ret) ? (int)temp(c, ty_ptr(f->ret), "result address", f->pos)
                                                                 : -1;
    /* The body's outermost block shares the parameters' scope, as in C. */
    block(c, &f->body);
    /* A function a syntax error cut short: clang reads on to its `}`, which
     * merlin does not, and what it finds at the end comes after that error.
     * A label after it is not known, so neither is a goto to a label that
     * is nowhere. */
    int cut = ck->has_cut && pos_eq(ck->cut, f->end);
    if (cut) {
        ck->late = (int)c->errors->n;
    } else {
        for (uint i = 0; i < c->gotos.n; i++) {
            struct goto_ref *g2 = c->gotos.v[i];
            if (map_get(&c->labels, g2->name) == NULL) {
                fail(c, g2->pos, fmt("use of undeclared label '%s'", g2->name));
            }
        }
        c->gotos.n = 0;
    }
    /* clang's analysis of the body, unless the program has an error so far
     * (in sema's, or before the function's end in the preprocessor's and
     * the lexer's): then the warnings of what runs are all given,
     * unanalysed. */
    int erred = cut;
    for (uint i = 0; i < ck->nearlier && !erred; i++) {
        if (pos_cmp(ck->earlier[i], f->end) <= 0) {
            erred = 1;
        }
    }
    for (uint i = 0; i < c->errors->n && !erred; i++) {
        if (!diag_is_warning(c->errors->v[i])) {
            erred = 1;
        }
    }
    if (erred) {
        for (uint i = 0; i < c->pending.n; i++) {
            push(c, ((struct pending *)c->pending.v[i])->d);
        }
    } else {
        struct intmap live;
        memset(&live, 0, sizeof live);
        int falls_off;
        int returns;
        flow_analyze(&f->body, &live, &falls_off, &returns);
        for (uint i = 0; i < c->pending.n; i++) {
            struct pending *p = c->pending.v[i];
            if (intmap_has(&live, p->of)) {
                push(c, p->d);
            }
        }
        /* Falling off the end of a function that returns a value: clang
         * warns. */
        if (!ty_is_void(f->ret) && falls_off) {
            const char *msg = returns ? "non-void function does not return a value in all control paths"
                                      : "non-void function does not return a value";
            push(c, diag_warning(f->end, msg, "return-type"));
        }
    }
    f->locals = c->locals;
    for (uint i = 0; i < c->linked.n; i++) {
        struct linked *l = c->linked.v[i];
        if (map_get(&ck->globals, l->name) == NULL) {
            /* merlin's Global::hidden: declared in a block, in scope only
             * there until file scope declares it. */
            struct global *h = arena_alloc(sizeof *h);
            h->is_func = ty_is_func(l->ty);
            h->ty = l->ty;
            h->pos = l->pos;
            h->begin = l->pos;
            h->spelled = l->spelled;
            map_put(&ck->globals, l->name, h);
        }
    }
}

static void global_var(struct checker *ck, struct declarator *d, uint item, uint decl) {
    if (ty_is_void(d->ty)) {
        vec_push(&ck->errors, diag_error(d->pos, "variable has incomplete type 'void'"));
        return;
    }
    if (d->init != NULL && d->storage == ST_EXTERN) {
        /* clang warns, and the declaration is the variable's definition;
         * extern takes the linkage a declaration before gave. */
        vec_push(&ck->errors, diag_warning(d->pos, "'extern' variable has an initializer", "extern-initializer"));
        struct global *p = map_get(&ck->globals, d->name);
        int internal = p != NULL && !p->is_func && p->is_static;
        d->storage = internal ? ST_STATIC : ST_NONE;
    }
    d->var.kind = VAR_GLOBAL;
    d->var.sym = d->name;
    /* As in C, the name is in scope from the end of its declarator, so it
     * is declared before its initializer is checked. */
    int is_static = d->storage == ST_STATIC;
    int tentative = d->storage != ST_EXTERN;
    int init = d->init != NULL;
    struct global *prev = map_get(&ck->globals, d->name);
    if (prev != NULL && prev->is_func) {
        struct diag *e = diag_error(d->pos, fmt("redefinition of '%s' as different kind of symbol", d->name));
        vec_push(&ck->errors, diag_note(e, prev->pos, previous_what(1)));
        return;
    }
    if (prev != NULL) {
        const char *msg = NULL;
        struct pos note_at = prev->pos;
        const char *note_what = previous_what(prev->defines);
        if (!agree(prev->ty, prev->spelled, d->ty, d->spelled)) {
            msg = fmt("redefinition of '%s' with a different type: %s vs %s", d->name, ty_named(d->ty, d->spelled),
                      ty_named(prev->ty, prev->spelled));
        } else if (prev->has_init_pos && init) {
            msg = fmt("redefinition of '%s'", d->name);
            note_at = prev->init_pos;
            note_what = previous_what(1);
        } else if (is_static && !prev->is_static) {
            msg = fmt("static declaration of '%s' follows non-static declaration", d->name);
        } else if (d->storage == ST_NONE && prev->is_static) {
            msg = fmt("non-static declaration of '%s' follows static declaration", d->name);
        }
        if (msg != NULL) {
            vec_push(&ck->errors, diag_note(diag_error(d->pos, msg), note_at, note_what));
            return;
        }
        /* The type the declarations make together. */
        prev->ty = ty_composite(prev->ty, d->ty);
        d->ty = prev->ty;
        if (tentative) {
            prev->tentative_ty = d->ty;
        }
        if (!prev->has_init && init) {
            prev->has_init = 1;
            prev->init_item = item;
            prev->init_decl = decl;
        }
        if (!prev->has_init_pos && init) {
            prev->has_init_pos = 1;
            prev->init_pos = d->pos;
        }
        if (!prev->has_tentative && tentative) {
            prev->has_tentative = 1;
            prev->tent_item = item;
            prev->tent_decl = decl;
        }
        prev->pos = d->pos;
        prev->defines = d->storage != ST_EXTERN;
        prev->visible = 1;
        prev->spelled = d->spelled;
        prev->begin = d->begin;
    } else {
        struct global *g = arena_alloc(sizeof *g);
        g->ty = d->ty;
        g->is_static = is_static;
        g->pos = d->pos;
        g->defines = d->storage != ST_EXTERN;
        g->begin = d->begin;
        g->spelled = d->spelled;
        g->has_init = init;
        g->init_item = item;
        g->init_decl = decl;
        g->has_init_pos = init;
        g->init_pos = d->pos;
        g->has_tentative = tentative;
        g->tent_item = item;
        g->tent_decl = decl;
        g->tentative_ty = tentative ? d->ty : NULL;
        g->visible = 1;
        map_put(&ck->globals, d->name, g);
    }
    if (d->init == NULL) {
        return;
    }
    struct fnc *fc = file_scope(ck);
    fc->init_spelled = d->spelled;
    int failed = init_object(fc, &d->ty, &d->init, 1, d->pos);
    /* The warnings of what an initializer does when it runs come after its
     * other diagnostics. */
    for (uint i = 0; i < fc->pending.n; i++) {
        push(fc, ((struct pending *)fc->pending.v[i])->d);
    }
    vec_extend(&ck->literals, &fc->literals);
    struct global *g = map_get(&ck->globals, d->name);
    if (failed) {
        /* Still declared, so later uses don't report it as undeclared, but
         * not defined here: there is nothing to emit. */
        if (g->has_init && g->init_item == item && g->init_decl == decl) {
            g->has_init = 0;
        }
        if (g->has_tentative && g->tent_item == item && g->tent_decl == decl) {
            g->has_tentative = 0;
        }
    } else if (ty_size(g->ty) < 0) {
        /* The initializer completed an array's size. */
        g->ty = d->ty;
    }
}

/* A definition to mark: where its declarator is, whether it has an
 * initializer, and the type it is defined with. */
struct definition {
    uint item;
    uint decl;
    int init;
    struct type *ty;
};

/* Marks the declarator that defines each global variable: the one with an
 * initializer, or the first that is not extern (C's tentative definition,
 * which then means zero). */
static void mark_definitions(struct checker *ck, struct program *prog) {
    struct definition *defs = NULL;
    uint n = 0;
    uint cap = 0;
    for (uint i = 0; i < ck->globals.cap; i++) {
        struct global *v = ck->globals.values[i];
        if (ck->globals.keys[i] == NULL || v->is_func || !(v->has_init || v->has_tentative)) {
            continue;
        }
        defs = grow(defs, &cap, n + 1, sizeof(struct definition));
        defs[n].item = v->has_init ? v->init_item : v->tent_item;
        defs[n].decl = v->has_init ? v->init_decl : v->tent_decl;
        defs[n].init = v->has_init;
        /* The type it is defined with, as clang has it: the initialized
         * declarator's, or the last tentative one's. */
        defs[n].ty = !v->has_init && v->tentative_ty != NULL ? v->tentative_ty : v->ty;
        n++;
    }
    /* By where the declarator is (each at one place: a stable sort is not
     * needed). */
    for (uint i = 1; i < n; i++) {
        struct definition d = defs[i];
        uint j = i;
        while (j > 0 && (defs[j - 1].item > d.item || (defs[j - 1].item == d.item && defs[j - 1].decl > d.decl))) {
            defs[j] = defs[j - 1];
            j--;
        }
        defs[j] = d;
    }
    for (uint k = 0; k < n; k++) {
        struct item *it = prog->items.v[defs[k].item];
        struct declarator *d = it->decls.v[defs[k].decl];
        struct type *ty = defs[k].init && ty_size(d->ty) >= 0 ? d->ty : defs[k].ty;
        if (ty->kind == TY_ARRAY && !ty->has_len && ty_size(ty->to) >= 0) {
            /* `int a[];` and nothing more: an array of one element, with
             * clang's warning. */
            vec_push(&ck->errors, diag_warning(d->pos, "tentative array definition assumed to have one element",
                                               "tentative-definition-array"));
            ty = ty_array(ty->to, 1, 1);
        } else if (ty_size(ty) < 0) {
            struct diag *e =
                diag_error(d->pos, fmt("tentative definition has type '%s' that is never completed", ty_str(ty)));
            struct record *r = ty_record(ty);
            if (r != NULL) {
                diag_note(e, r->pos, fmt("forward declaration of '%s'", record_name(r)));
            }
            vec_push(&ck->errors, e);
        }
        d->define = 1;
        d->ty = ty;
    }
    heap_free(defs);
}

/* ---- Conversions ---- */

enum { CONV_ASSIGN, CONV_INIT, CONV_ARG, CONV_RETURN };

/* What converts what, as clang words it. */
static char *conv_phrase(int how, const char *to, const char *from) {
    switch (how) {
    case CONV_ASSIGN:
        return fmt("assigning to %s from %s", to, from);
    case CONV_INIT:
        return fmt("initializing %s with an expression of type %s", to, from);
    case CONV_ARG:
        return fmt("passing %s to parameter of type %s", from, to);
    default:
        return fmt("returning %s from a function with result type %s", from, to);
    }
}

/* clang's message for a conversion between types C does not convert
 * between at all. */
static char *conv_incompatible(int how, const char *to, const char *from) {
    switch (how) {
    case CONV_ASSIGN:
        return fmt("assigning to %s from incompatible type %s", to, from);
    case CONV_INIT:
        return fmt("initializing %s with an expression of incompatible type %s", to, from);
    case CONV_ARG:
        return fmt("passing %s to parameter of incompatible type %s", from, to);
    default:
        return fmt("returning %s from a function with incompatible result type %s", from, to);
    }
}

/* to is from but for qualifiers it has more of. */
static int fits_quals(struct type *to, struct type *from) {
    uint tq = ty_quals(to);
    uint fq = ty_quals(from);
    return ty_eq(ty_unqual(to), ty_unqual(from)) && (tq & fq) == fq;
}

/* The fix clang suggests after a conversion it rejects (its
 * ConversionFixItGenerator). */
static const char *conversion_hint(struct expr *e, struct type *from, struct type *to) {
    struct expr *bare = e;
    while (bare->kind == E_CAST) {
        bare = bare->x;
    }
    i128 v;
    int null = ty_is_integer(bare->ty) && consteval_int(bare, &v) && i128_is_zero(v);
    struct type *fu = ty_unqual(from);
    struct type *tu = ty_unqual(to);
    if (fu->kind == TY_PTR && fits_quals(to, fu->to) && !null) {
        return "; dereference with *";
    }
    if (tu->kind == TY_PTR && is_lvalue(e) && fits_quals(tu->to, from)) {
        return "; take the address with &";
    }
    return "";
}

/* clang's error for assigning to target whose struct or union type has a
 * const member, at any depth (DiagnoseRecursiveConstFields): the first
 * such member, breadth first, with a note on each; NULL if there is none. */
static struct diag *const_fields(struct expr *target, struct pos at) {
    struct record *top = ty_record(target->ty);
    if (top == NULL) {
        return NULL;
    }
    const char *whose;
    if (target->kind == E_VAR) {
        whose = fmt("variable '%s'", target->name);
    } else if (target->kind == E_MEMBER) {
        whose = fmt("non-static data member '%s'", target->name);
    } else {
        whose = "lvalue";
    }
    struct vec records;
    memset(&records, 0, sizeof records);
    vec_push(&records, top);
    struct diag *error = NULL;
    for (uint next = 0; next < records.n; next++) {
        const char *nested = next > 0 ? "nested " : "";
        struct record_body *body = ((struct record *)records.v[next])->body;
        for (uint i = 0; body != NULL && i < body->nmembers; i++) {
            struct member *m = &body->members[i];
            /* A member's type, an array's element type for an array. */
            struct type *elem = m->ty;
            while (elem->kind == TY_ARRAY) {
                elem = elem->to;
            }
            if (ty_quals(elem) & Q_CONST) {
                if (error == NULL) {
                    error = diag_error(at, fmt("cannot assign to %s with %sconst-qualified data member '%s'", whose,
                                               nested, m->name));
                }
                diag_note(error, m->pos, fmt("%sdata member '%s' declared const here", nested, m->name));
            }
            struct record *r = ty_record(m->ty);
            if (r != NULL) {
                int seen = 0;
                for (uint k = 0; k < records.n; k++) {
                    if (records.v[k] == r) {
                        seen = 1;
                    }
                }
                if (!seen) {
                    vec_push(&records, r);
                }
            }
        }
    }
    heap_free(records.v);
    return error;
}

/* ---- Switches ---- */

/* Whether e's value can only be 0 or 1 (clang's isKnownToHaveBooleanValue,
 * -Wswitch-bool). */
static int known_boolean(struct expr *e) {
    if (ty_eq(ty_unqual(e->ty), ty_int(IK_BOOL))) {
        return 1;
    }
    if (!ty_is_integer(e->ty)) {
        return 0;
    }
    switch (e->kind) {
    case E_UNARY:
        if (e->op == UOP_PLUS) {
            return known_boolean(e->x);
        }
        return e->op == UOP_NOT;
    case E_CAST:
        /* Only conversions it does not write. */
        return e->implicit && known_boolean(e->x);
    case E_BINARY:
        if (e->op == OP_LOGAND || e->op == OP_LOGOR) {
            return 1;
        }
        if (e->op == OP_BITAND || e->op == OP_BITOR || e->op == OP_BITXOR) {
            return known_boolean(e->x) && known_boolean(e->y);
        }
        return binop_is_comparison(e->op);
    case E_COMMA:
        return known_boolean(e->y);
    case E_ASSIGN:
        return e->op == OP_NONE && known_boolean(e->y);
    case E_COND:
        return known_boolean(e->y) && known_boolean(e->z);
    default:
        return 0;
    }
}

/* The enumeration constant a case's expression names, through casts. */
static const char *case_name(struct expr *e) {
    if (e->kind == E_CAST) {
        return case_name(e->x);
    }
    if (e->kind == E_ENUMERATOR) {
        return e->name;
    }
    return NULL;
}

/* A stable sort of case labels by value (insertion sort). */
static void sort_labels(struct case_label **l, uint n) {
    for (uint i = 1; i < n; i++) {
        struct case_label *x = l[i];
        uint j = i;
        while (j > 0 && i128_lt(x->value, l[j - 1]->value)) {
            l[j] = l[j - 1];
            j--;
        }
        l[j] = x;
    }
}

/* Where clang reports e where it does not look through parentheses: at the
 * outermost `(` around it. */
static struct pos loc_written(struct fnc *c, struct expr *e) {
    struct pos *open = posmap_get(&c->c->written.parens, e->pos);
    return open != NULL ? *open : expr_loc(e);
}

/* Where e begins as written: at the outermost `(` around it or around what
 * it begins with (clang's getBeginLoc). */
static struct pos begin_written(struct fnc *c, struct expr *e) {
    struct pos *open = posmap_get(&c->c->written.parens, e->pos);
    if (open != NULL) {
        return *open;
    }
    switch (e->kind) {
    case E_BINARY:
    case E_COMMA:
    case E_COND:
    case E_ASSIGN:
    case E_MEMBER:
    case E_CALL:
        return begin_written(c, e->x);
    case E_INCDEC:
        return e->prefix ? e->pos : begin_written(c, e->x);
    case E_DEREF:
        return pos_eq(e->x->pos, e->pos) ? begin_written(c, e->x) : e->pos;
    case E_CAST:
        return e->implicit ? begin_written(c, e->x) : e->pos;
    default:
        return e->pos;
    }
}

/* Where e begins inside the parentheses around it (getBeginLoc of e
 * without its parentheses, IgnoreParenCasts). */
static struct pos begin_inside(struct fnc *c, struct expr *e) {
    switch (e->kind) {
    case E_BINARY:
    case E_COMMA:
    case E_COND:
    case E_ASSIGN:
    case E_MEMBER:
    case E_CALL:
        return begin_written(c, e->x);
    case E_INCDEC:
        return e->prefix ? e->pos : begin_written(c, e->x);
    case E_DEREF:
        return pos_eq(e->x->pos, e->pos) ? begin_written(c, e->x) : e->pos;
    case E_CAST:
        return e->implicit ? begin_inside(c, e->x) : e->pos;
    default:
        return e->pos;
    }
}

static int has_value(struct case_label **l, uint n, i128 v) {
    for (uint i = 0; i < n; i++) {
        if (i128_eq(l[i]->value, v)) {
            return 1;
        }
    }
    return 0;
}

/* clang's checks of a switch's cases, as it ends (ActOnFinishSwitchStmt):
 * values that are the same; a constant condition no case has, without a
 * default; and of a condition of an enum (en, named as messages name its
 * type), cases that are none of its constants, and without a default,
 * constants no case has (-Wswitch). Whether the cases cover every constant
 * of the enum. */
static int finish_switch(struct fnc *c, struct switch_ctx *ctx, struct expr *cond, struct enum_type *en,
                         const char *named) {
    /* clang sorts the cases by value (keeping their order among equal
     * ones): each duplicate is reported against the one before it. */
    uint n = ctx->labels.n;
    struct case_label **sorted = heap_alloc((n + 1) * sizeof(struct case_label *));
    for (uint i = 0; i < n; i++) {
        sorted[i] = ctx->labels.v[i];
    }
    sort_labels(sorted, n);
    int erroneous = 0;
    for (uint i = 0; i + 1 < n; i++) {
        struct case_label *prev = sorted[i];
        struct case_label *cur = sorted[i + 1];
        if (!i128_eq(prev->value, cur->value)) {
            continue;
        }
        erroneous = 1;
        char *v = i128_str(prev->value);
        const char *pn = prev->name != NULL ? prev->name : v;
        const char *cn = cur->name != NULL ? cur->name : v;
        int same = (prev->name == NULL && cur->name == NULL) ||
                   (prev->name != NULL && cur->name != NULL && strcmp(prev->name, cur->name) == 0);
        const char *msg = same ? fmt("duplicate case value '%s'", pn)
                               : fmt("duplicate case value: '%s' and '%s' both equal '%s'", pn, cn, v);
        push(c, diag_note(diag_error(cur->begin, msg), prev->begin, "previous case defined here"));
    }
    if (erroneous || ctx->incomplete) {
        heap_free(sorted);
        return 0;
    }
    /* A constant condition no case matches, with no default. */
    int constant = 0;
    i128 v;
    if (!ctx->has_default && consteval_int(cond, &v)) {
        constant = 1;
        int any = 0;
        for (uint i = 0; i < n; i++) {
            if (i128_eq(((struct case_label *)ctx->labels.v[i])->value, v)) {
                any = 1;
            }
        }
        if (!any) {
            push(c, diag_warning_unflagged(loc_written(c, cond),
                                           fmt("no case matching constant switch condition '%s'", i128_str(v))));
        }
    }
    if (en == NULL || constant) {
        heap_free(sorted);
        return 0;
    }
    /* The enum's constants as the condition's promoted type has them, by
     * value, the first of each value. */
    uint nc = en->nconstants;
    struct case_label *consts = heap_alloc((nc + 1) * sizeof(struct case_label));
    struct case_label **cs = heap_alloc((nc + 1) * sizeof(struct case_label *));
    for (uint i = 0; i < nc; i++) {
        consts[i].value = ik_wrap(ctx->kind, en->constants[i].value);
        consts[i].name = en->constants[i].name;
        cs[i] = &consts[i];
    }
    sort_labels(cs, nc);
    uint m = 0;
    for (uint i = 0; i < nc; i++) {
        if (m == 0 || !i128_eq(cs[m - 1]->value, cs[i]->value)) {
            cs[m++] = cs[i];
        }
    }
    for (uint i = 0; i < n; i++) {
        if (!has_value(cs, m, sorted[i]->value)) {
            push(c, diag_warning(sorted[i]->loc, fmt("case value not in enumerated type %s", named), "switch"));
        }
    }
    struct vec unhandled;
    memset(&unhandled, 0, sizeof unhandled);
    for (uint i = 0; i < m; i++) {
        if (!has_value(sorted, n, cs[i]->value)) {
            vec_push(&unhandled, fmt("'%s'", cs[i]->name));
        }
    }
    if (!ctx->has_default && unhandled.n > 0) {
        char **u = (char **)unhandled.v;
        const char *msg;
        if (unhandled.n == 1) {
            msg = fmt("enumeration value %s not handled in switch", u[0]);
        } else if (unhandled.n == 2) {
            msg = fmt("enumeration values %s and %s not handled in switch", u[0], u[1]);
        } else if (unhandled.n == 3) {
            msg = fmt("enumeration values %s, %s, and %s not handled in switch", u[0], u[1], u[2]);
        } else {
            msg = fmt("%u enumeration values not handled in switch: %s, %s, %s...", unhandled.n, u[0], u[1], u[2]);
        }
        /* At where the condition begins: clang's condition of an enum is
         * always converted, which it reports where it begins. */
        push(c, diag_warning(begin_written(c, cond), msg, "switch"));
    }
    int covered = unhandled.n == 0;
    heap_free(unhandled.v);
    heap_free(consts);
    heap_free(cs);
    heap_free(sorted);
    return covered;
}

/* ---- Statements ---- */

/* Whether s is a `;` on the line of the `)` before it, not of a macro's
 * expansion: an empty body clang warns of. */
static int empty_body(struct fnc *c, struct stmt *s) {
    return s->kind == S_EMPTY && s->glued && !posmap_has(&c->c->written.macros, s->pos);
}

/* clang's -Wempty-body for a while or for loop s whose body is a `;` on the
 * line of its `)`, followed by next, which looks like the body meant. */
static void empty_loop_body(struct fnc *c, struct stmt *s, struct stmt *next) {
    const char *what;
    if (s->kind == S_WHILE) {
        what = "while loop";
    } else if (s->kind == S_FOR) {
        what = "for loop";
    } else {
        return;
    }
    int typo = next->kind == S_BLOCK || next->pos.col > s->pos.col;
    if (typo && empty_body(c, s->body)) {
        push(c, empty_body_warning(s->body->pos, what));
    }
}

static void stmt(struct fnc *c, struct stmt *s);

/* The statements of a block, then clang's warnings of its loops' empty
 * bodies, which it gives at the block's end. */
static void block(struct fnc *c, struct vec *body) {
    for (uint i = 0; i < body->n; i++) {
        stmt(c, body->v[i]);
    }
    for (uint i = 0; i + 1 < body->n; i++) {
        empty_loop_body(c, body->v[i], body->v[i + 1]);
    }
}

static void scoped_stmt(struct fnc *c, struct stmt *s) {
    push_scope(c);
    stmt(c, s);
    pop_scope(c);
}

/* A loop's body: in a loop, in a scope of its own. */
static void loop_body(struct fnc *c, struct stmt *s) {
    c->loop_depth++;
    c->break_depth++;
    scoped_stmt(c, s);
    c->loop_depth--;
    c->break_depth--;
}

static void local_decl(struct fnc *c, struct declarator *d);
static void ret(struct fnc *c, struct expr *e, struct pos pos);
static int condition(struct fnc *c, struct expr *e, struct pos at);
static void unused(struct fnc *c, struct expr *e);

/* An expression statement's checks, and the for's step. */
static void expression_statement(struct fnc *c, struct expr *e) {
    if (check_expr(c, e) == 0) {
        full_expression(c, e);
        unused(c, e);
        bounds(c, e);
    }
}

static void switch_stmt(struct fnc *c, struct stmt *s) {
    struct expr *cond = s->e;
    /* The type of the condition before it is promoted: an enum's constants
     * are what clang's -Wswitch checks the cases with. */
    struct enum_type *before = NULL;
    const char *named = NULL;
    /* The controlling expression is promoted; case values convert to it. */
    int kind = IK_INT;
    if (check_expr(c, cond) == 0) {
        struct type *t = ty_decay(cond->ty);
        if (t->kind == TY_INT) {
            if (known_boolean(cond)) {
                push(c, diag_warning(s->pos, "switch condition has boolean value", "switch-bool"));
            }
            struct spelling *spelled = spelled_of(c, cond);
            before = enum_of(spelled, cond->ty);
            if (before != NULL) {
                named = ty_named(cond->ty, spelled);
            }
            coerce(cond, ty_int(ik_promoted(t->ik)));
            kind = ik_promoted(t->ik);
        } else {
            fail(c, s->pos, fmt("statement requires expression of integer type ('%s' invalid)", ty_str(t)));
        }
    }
    /* The condition is a full expression, checked as one. */
    if (ty_is_integer(cond->ty)) {
        full_expression(c, cond);
    }
    struct switch_ctx *ctx = arena_alloc(sizeof *ctx);
    ctx->kind = kind;
    vec_push(&c->switches, ctx);
    c->break_depth++;
    scoped_stmt(c, s->body);
    c->break_depth--;
    vec_pop(&c->switches);
    s->covered = (uchar)finish_switch(c, ctx, cond, before, named);
    if (empty_body(c, s->body)) {
        push(c, empty_body_warning(s->body->pos, "switch statement"));
    }
    /* The first case of each value is the one taken. */
    s->cases = arena_alloc(ctx->labels.n * sizeof(struct case_ref) + 1);
    s->ncases = 0;
    for (uint i = 0; i < ctx->labels.n; i++) {
        struct case_label *l = ctx->labels.v[i];
        int seen = 0;
        for (uint k = 0; k < s->ncases; k++) {
            if (i128_eq(s->cases[k].value, l->value)) {
                seen = 1;
            }
        }
        if (!seen) {
            s->cases[s->ncases].value = l->value;
            s->cases[s->ncases].id = l->id;
            s->ncases++;
        }
    }
    s->id = ctx->has_default ? (int)ctx->default_id : -1;
}

static void case_stmt(struct fnc *c, struct stmt *s) {
    struct pos pos = s->pos;
    struct expr *e = s->e;
    /* A case's value is a constant: nothing of what it does when it runs is
     * warned of, but its overflows, which clang's evaluator finds. */
    c->unevaluated++;
    int failed = check_expr(c, e);
    c->unevaluated--;
    if (!failed) {
        overflow(c, e);
    }
    if (c->switches.n == 0) {
        fail(c, pos, "'case' statement not in switch statement");
    } else if (failed) {
        ((struct switch_ctx *)vec_last(&c->switches))->incomplete = 1;
    } else {
        i128 v;
        if (!(consteval_int(e, &v) && ty_is_integer(e->ty))) {
            struct diag *d = diag_error(begin_written(c, e), "expression is not an integer constant expression");
            struct pos at;
            const char *note;
            if (consteval_why_not(e, &at, &note)) {
                diag_note(d, at, note);
            }
            push(c, d);
            ((struct switch_ctx *)vec_last(&c->switches))->incomplete = 1;
        } else {
            s->id = (int)c->next_case++;
            struct case_label *l = arena_alloc(sizeof *l);
            l->begin = begin_written(c, e);
            l->loc = loc_written(c, e);
            struct switch_ctx *sw = vec_last(&c->switches);
            s->value = ik_wrap(sw->kind, v);
            l->value = s->value;
            l->id = (uint)s->id;
            l->name = case_name(e);
            vec_push(&sw->labels, l);
        }
    }
    stmt(c, s->body);
}

static void stmt(struct fnc *c, struct stmt *s) {
    switch (s->kind) {
    case S_BLOCK:
        push_scope(c);
        block(c, &s->list);
        pop_scope(c);
        break;
    case S_DECL:
        for (uint i = 0; i < s->list.n; i++) {
            local_decl(c, s->list.v[i]);
        }
        break;
    case S_EXPR:
        expression_statement(c, s->e);
        break;
    case S_RETURN:
        ret(c, s->e, s->pos);
        break;
    case S_EMPTY:
        break;
    case S_UNEVALUATED:
        unevaluated_list(c, &s->list);
        break;
    case S_IF:
        /* clang reports a condition that is no scalar at the keyword. */
        condition(c, s->e, s->pos);
        scoped_stmt(c, s->body);
        if (s->els != NULL) {
            scoped_stmt(c, s->els);
        } else if (empty_body(c, s->body)) {
            push(c, empty_body_warning(s->body->pos, "if statement"));
        }
        break;
    case S_WHILE:
        condition(c, s->e, s->pos);
        loop_body(c, s->body);
        break;
    case S_DOWHILE:
        loop_body(c, s->body);
        condition(c, s->e, s->pos);
        break;
    case S_FOR:
        push_scope(c);
        if (s->init != NULL) {
            stmt(c, s->init);
        }
        unevaluated_list(c, &s->list);
        if (s->e != NULL) {
            condition(c, s->e, s->pos);
        }
        if (s->step != NULL) {
            expression_statement(c, s->step);
        }
        loop_body(c, s->body);
        pop_scope(c);
        break;
    case S_BREAK:
        if (c->break_depth == 0) {
            fail(c, s->pos, "'break' statement not in loop or switch statement");
        }
        break;
    case S_CONTINUE:
        if (c->loop_depth == 0) {
            fail(c, s->pos, "'continue' statement not in loop statement");
        }
        break;
    case S_SWITCH:
        switch_stmt(c, s);
        break;
    case S_CASE:
        case_stmt(c, s);
        break;
    case S_DEFAULT: {
        struct pos pos = s->pos;
        s->id = (int)c->next_case++;
        if (c->switches.n == 0) {
            fail(c, pos, "'default' statement not in switch statement");
        } else {
            struct switch_ctx *sw = vec_last(&c->switches);
            if (sw->has_default) {
                push(c, diag_note(diag_error(pos, "multiple default labels in one switch"), sw->default_pos,
                                  "previous case defined here"));
            } else {
                sw->has_default = 1;
                sw->default_id = (uint)s->id;
                sw->default_pos = pos;
            }
        }
        stmt(c, s->body);
        break;
    }
    case S_LABEL: {
        /* clang defines a label after the statement it labels. */
        stmt(c, s->body);
        struct pos *prev = map_get(&c->labels, s->name);
        map_put(&c->labels, s->name, pos_box(s->pos));
        if (prev != NULL) {
            push(c, diag_note(diag_error(s->pos, fmt("redefinition of label '%s'", s->name)), *prev,
                              "previous definition is here"));
        }
        break;
    }
    default: {
        struct goto_ref *g = arena_alloc(sizeof *g);
        g->name = s->name;
        g->pos = s->at;
        vec_push(&c->gotos, g);
        break;
    }
    }
}

static int convert(struct fnc *c, struct expr *e, struct type *to, struct spelling *to_spelled, int how,
                   struct pos at);

static void ret(struct fnc *c, struct expr *e, struct pos pos) {
    struct type *r = c->ret;
    if (e == NULL) {
        if (!ty_eq(r, TY_VOID_T)) {
            struct diag *d = diag_error(pos, fmt("non-void function '%s' should return a value", c->func));
            d->flag = "return-mismatch";
            push(c, d);
        }
        return;
    }
    if (check_expr(c, e)) {
        return;
    }
    if (ty_eq(r, TY_VOID_T)) {
        /* `return v();` of a void function is C99's too. */
        if (!ty_is_void(e->ty)) {
            struct diag *d = diag_error(pos, fmt("void function '%s' should not return a value", c->func));
            d->flag = "return-mismatch";
            push(c, d);
        }
    } else if (convert(c, e, r, c->ret_spelled, CONV_RETURN, begin_written(c, e)) == 0) {
        coerce(e, r);
    }
    /* The address of an automatic object returned: clang's warning, at the
     * object (checkExprLifetime). */
    if (!ty_is_void(r)) {
        struct vec found;
        memset(&found, 0, sizeof found);
        stack_address(c, e, &found);
        for (uint i = 0; i < found.n; i += 2) {
            struct pos *at = found.v[i];
            const char *what = found.v[i + 1];
            push(c, diag_warning(*at, fmt("address of stack memory associated with %s returned", what),
                                 "return-stack-address"));
        }
        heap_free(found.v);
    }
    full_expression(c, e);
    bounds(c, e);
}

static void assignment_condition(struct fnc *c, struct expr *e);
static void bool_like(struct fnc *c, struct expr *e);

/* The condition of a statement at `at`, which must be a scalar. */
static int condition(struct fnc *c, struct expr *e, struct pos at) {
    if (check_expr(c, e)) {
        return 1;
    }
    if (!ty_is_scalar(ty_decay(e->ty))) {
        return fail(c, at, fmt("statement requires expression of scalar type ('%s' invalid)", ty_str(e->ty)));
    }
    /* clang's CheckBooleanCondition, then the full expression's. */
    assignment_condition(c, e);
    bool_like(c, e);
    full_expression(c, e);
    bounds(c, e);
    return 0;
}

/* Whether the value of e is all it gives (clang's isUnusedResultAWarning):
 * then where clang reports it, the expression it is of, and whether it
 * reads a volatile object. */
static int unused_result(struct fnc *c, struct expr *e, struct pos *at, uint64_t *of, int *vol) {
    switch (e->kind) {
    case E_ASSIGN:
    case E_INCDEC:
    case E_CALL:
    case E_VA_START:
    case E_VA_ARG:
    case E_VA_END:
    case E_VA_COPY:
        return 0;
    case E_CAST:
        if (!e->implicit && ty_is_void(e->ty)) {
            return 0;
        }
        if (e->implicit) {
            return unused_result(c, e->x, at, of, vol);
        }
        break;
    case E_COMMA:
        if (e->y->kind == E_NUM && i128_is_zero(e->y->value)) {
            return 0;
        }
        return unused_result(c, e->y, at, of, vol);
    case E_COND:
        /* Both operands, or it may be control flow; at the second's. */
        if (!unused_result(c, e->y, at, of, vol)) {
            return 0;
        }
        return unused_result(c, e->z, at, of, vol);
    case E_BINARY:
        if (e->op == OP_LOGAND || e->op == OP_LOGOR) {
            if (!unused_result(c, e->y, at, of, vol)) {
                return 0;
            }
            *at = e->pos;
            *of = e->id;
            *vol = 0;
            return 1;
        }
        break;
    default:
        break;
    }
    /* Reading a volatile object is what it does; of volatile void there is
     * nothing read. */
    if (is_lvalue(e) && (ty_quals(e->ty) & Q_VOLATILE) && !ty_is_void(e->ty)) {
        return 0;
    }
    if (is_lvalue(e) && (ty_quals(e->ty) & Q_VOLATILE)) {
        *at = expr_loc(e);
        *of = e->id;
        *vol = 1;
        return 1;
    }
    /* a[i], at its `]`. */
    if (e->kind == E_DEREF && pos_eq(e->x->pos, e->pos)) {
        struct pos *close = posmap_get(&c->c->written.brackets, e->pos);
        *at = close != NULL ? *close : e->pos;
        *of = e->id;
        *vol = 0;
        return 1;
    }
    *at = expr_loc(e);
    *of = e->id;
    *vol = 0;
    return 1;
}

/* clang's warnings of an expression statement e whose value is all it
 * gives (DiagnoseUnusedExprResult): a comparison's at once, any other's
 * when the function ends, if it can run. */
static void unused(struct fnc *c, struct expr *e) {
    if (e->kind == E_BINARY && binop_is_comparison(e->op)) {
        const char *what = "relational";
        const char *note = NULL;
        if (e->op == OP_EQ) {
            what = "equality";
            note = "use '=' to turn this equality comparison into an assignment";
        } else if (e->op == OP_NE) {
            what = "inequality";
            note = "use '|=' to turn this inequality comparison into an or-assignment";
        }
        struct diag *w = diag_warning(e->pos, fmt("%s comparison result unused", what), "unused-comparison");
        if (note != NULL) {
            diag_note(w, e->pos, note);
        }
        push(c, w);
        return;
    }
    struct pos at;
    uint64_t of;
    int vol;
    if (!unused_result(c, e, &at, &of, &vol)) {
        return;
    }
    if (vol) {
        push(c, diag_warning(at, "expression result unused; assign into a variable to force a volatile load",
                             "unused-volatile-lvalue"));
    } else {
        runtime(c, diag_warning(at, "expression result unused", "unused-value"), of);
    }
}

/* clang's -Wparentheses of a condition that is an assignment written
 * without parentheses (`if (a = b)`). */
static void assignment_condition(struct fnc *c, struct expr *e) {
    if (e->kind == E_ASSIGN && e->op == OP_NONE && !posmap_has(&c->c->written.parens, e->pos)) {
        struct diag *w = diag_warning(e->pos, "using the result of an assignment as a condition without parentheses",
                                      "parentheses");
        diag_note(w, e->pos, "place parentheses around the assignment to silence this warning");
        diag_note(w, e->pos, "use '==' to turn this assignment into an equality comparison");
        push(c, w);
    }
}

static void int_in_bool(struct fnc *c, struct expr *e);

/* clang's -Wpointer-bool-conversion of e taken as true or false: an address
 * that is never null. */
static void bool_like(struct fnc *c, struct expr *e) {
    while (e->kind == E_CAST && e->implicit) {
        e = e->x;
    }
    struct diag *w = never_null(c, e, -1);
    if (w != NULL) {
        push(c, w);
    }
    int_in_bool(c, e);
}

/* An integer literal as written, under its implicit conversions. */
static int written_literal(struct fnc *c, struct expr *e, i128 *v) {
    while (e->kind == E_CAST && e->implicit) {
        e = e->x;
    }
    if (e->kind == E_NUM && posmap_has(&c->c->written.int_literals, e->pos)) {
        *v = e->value;
        return 1;
    }
    return 0;
}

/* clang's DiagnoseIntInBoolContext, of e taken as true or false: a << of an
 * integer literal; a ?: of two integer literals, neither 0 nor 1. */
static void int_in_bool(struct fnc *c, struct expr *e) {
    i128 x;
    i128 y;
    if (e->kind == E_BINARY && e->op == OP_SHL) {
        int known = 0;
        int always = 0;
        if (written_literal(c, e->x, &x)) {
            if (i128_is_zero(x)) {
                known = 1;
                always = 0;
            } else if (consteval_int(e, &y)) {
                known = 1;
                always = !i128_is_zero(y);
            }
        }
        if (known) {
            push(c, diag_warning(e->pos,
                                 fmt("converting the result of '<<' to a boolean always evaluates to %s",
                                     always ? "true" : "false"),
                                 "tautological-constant-compare"));
        }
    } else if (e->kind == E_COND) {
        if (!written_literal(c, e->y, &x) || !written_literal(c, e->z, &y)) {
            return;
        }
        int x01 = i128_is_zero(x) || i128_eq(x, i128_of(1));
        int y01 = i128_is_zero(y) || i128_eq(y, i128_of(1));
        if (!(x01 && y01) && !i128_is_zero(x) && !i128_is_zero(y)) {
            push(c, diag_warning(begin_of(e),
                                 "converting the result of '?:' with integer constants to a boolean always "
                                 "evaluates to 'true'",
                                 "tautological-constant-compare"));
        }
    }
}

/* What clang checks of a full expression e once it is built
 * (AnalyzeImplicitConversions), in its order: it takes the nodes off a
 * stack, an operator's operands pushed left first, so the right one comes
 * first; a comparison's, an assignment's and a ?:'s it walks in order. */
static void full(struct fnc *c, struct expr *e) {
    struct vec work;
    memset(&work, 0, sizeof work);
    vec_push(&work, e);
    while (work.n > 0) {
        struct expr *x = vec_pop(&work);
        if (x->kind == E_BINARY && binop_is_comparison(x->op)) {
            out_of_range(c, x->op, x->pos, x->id, x->x, x->y);
            full(c, x->x);
            full(c, x->y);
        } else if (x->kind == E_ASSIGN) {
            full(c, x->x);
            full(c, x->y);
        } else if (x->kind == E_COND) {
            full(c, x->x);
            full(c, x->y);
            full(c, x->z);
        } else if (x->kind == E_SIZEOF_EXPR || x->kind == E_SIZEOF_TYPE) {
            /* Not evaluated. */
        } else {
            struct expr **k;
            uint n = children(x, &k);
            for (uint i = 0; i < n; i++) {
                vec_push(&work, k[i]);
            }
            if (x->kind == E_BINARY && (x->op == OP_LOGAND || x->op == OP_LOGOR)) {
                /* `p && "message"` is an assertion's idiom. */
                struct expr *ops[2];
                ops[0] = x->x;
                ops[1] = x->y;
                for (uint i = 0; i < 2; i++) {
                    int string = ops[i]->kind == E_STR;
                    if (!(string && x->op == OP_LOGAND)) {
                        bool_like(c, ops[i]);
                    }
                }
            } else if (x->kind == E_UNARY && x->op == UOP_NOT) {
                bool_like(c, x->x);
            }
        }
    }
    heap_free(work.v);
}

/* The first operand of ?:, which must be a scalar. */
static int cond_operand(struct fnc *c, struct expr *e) {
    if (check_expr(c, e)) {
        return 1;
    }
    struct type *t = ty_decay(e->ty);
    if (!ty_is_scalar(t)) {
        return fail(c, begin_written(c, e),
                    fmt("used type '%s' where arithmetic or pointer type is required", ty_str(t)));
    }
    bool_like(c, e);
    return 0;
}

/* ---- Local declarations ---- */

static int same_scope_conflict(struct fnc *c, struct declarator *d);
static int linkage_redeclared(struct fnc *c, struct declarator *d);
static int init_object(struct fnc *c, struct type **ty, struct init **init, int constant, struct pos at);

/* A declarator in a block: what the parser evaluated away of it before its
 * name is declared, of its initializer after. */
static void local_var(struct fnc *c, struct declarator *d);

static void local_decl(struct fnc *c, struct declarator *d) {
    unevaluated_list(c, &d->sized);
    local_var(c, d);
    unevaluated_list(c, &d->unevaluated);
}

static struct binding *binding_new(struct var var, struct type *ty, struct pos pos) {
    struct binding *b = arena_alloc(sizeof *b);
    b->var = var;
    b->ty = ty;
    b->pos = pos;
    return b;
}

static void local_var(struct fnc *c, struct declarator *d) {
    posmap_put(&c->spellings, d->pos, d->spelled);
    posmap_put(&c->begins, d->pos, pos_box(d->begin));
    /* A redeclaration the block's earlier declaration of the name does not
     * allow: the name stays the earlier's. */
    if (same_scope_conflict(c, d)) {
        return;
    }
    /* A block's extern declaration, or a function's: in the block the name
     * is the object or function outside it, which it defines nothing of. */
    if (d->storage == ST_EXTERN || ty_is_func(d->ty)) {
        /* clang declares it all the same, invalid if it conflicts with a
         * declaration around the block; one that conflicts with the block's
         * own leaves the name that one's. */
        if (linkage_redeclared(c, d)) {
            if (map_get(vec_last(&c->scopes), d->name) != NULL) {
                return;
            }
            posmap_put(&c->invalid, d->pos, NULL);
        }
        struct var v;
        v.kind = VAR_GLOBAL;
        v.local = 0;
        v.sym = d->name;
        map_put(vec_last(&c->scopes), d->name, binding_new(v, d->ty, d->pos));
        struct linked *l = arena_alloc(sizeof *l);
        l->name = d->name;
        l->ty = d->ty;
        l->pos = d->pos;
        l->spelled = d->spelled;
        vec_push(&c->linked, l);
        d->var = v;
        if (d->init != NULL) {
            fail(c, d->pos, "declaration of block scope identifier with linkage cannot have an initializer");
        }
        return;
    }
    if (ty_is_void(d->ty)) {
        fail(c, d->pos, "variable has incomplete type 'void'");
        return;
    }
    if (d->storage == ST_STATIC) {
        /* A static local is a global of its own, visible only in this
         * block, named function.name as clang names it; the key sets apart
         * several of the same name in one function. */
        const char *base = fmt("%s.%s", c->func, d->name);
        const char *label = base;
        uint n = 0;
        while (map_get(&c->static_labels, label) != NULL) {
            n++;
            label = symbol_key(base, n);
        }
        map_put(&c->static_labels, label, (void *)label);
        /* As in C, the name is in scope inside its own initializer. */
        struct var v;
        v.kind = VAR_GLOBAL;
        v.local = 0;
        v.sym = label;
        bind_local(c, d->name, v, d->ty, d->pos, "");
        if (d->init != NULL) {
            c->init_spelled = d->spelled;
            if (init_object(c, &d->ty, &d->init, 1, d->pos)) {
                return;
            }
            /* The initializer may have completed an array's size. */
            map_put(vec_last(&c->scopes), d->name, binding_new(v, d->ty, d->pos));
        }
        d->var = v;
        d->define = 1;
    } else {
        /* As in C, the name is in scope inside its own initializer. */
        uint id = declare(c, d->name, d->ty, d->pos, "");
        if (d->storage == ST_REGISTER) {
            intmap_put(&c->registers, id, NULL);
        }
        d->var.kind = VAR_LOCAL;
        d->var.local = id;
        if (d->init != NULL) {
            c->init_spelled = d->spelled;
            if (init_object(c, &d->ty, &d->init, 0, d->pos)) {
                return;
            }
            /* The initializer may have completed an array's size. */
            ((struct local *)c->locals.v[id])->ty = d->ty;
            map_put(vec_last(&c->scopes), d->name, binding_new(d->var, d->ty, d->pos));
        }
    }
    if (ty_size(d->ty) < 0) {
        incomplete_variable(c, d->pos, d->ty);
    }
}

static int init_by_literal(struct fnc *c, struct type **ty, struct init **init, int constant, struct pos at,
                           int *failed);

/* init_check of a declared object (declared at `at`), which a compound
 * literal of its type may initialize as its list would. */
static int init_object(struct fnc *c, struct type **ty, struct init **init, int constant, struct pos at) {
    int failed;
    if (!init_by_literal(c, ty, init, constant, at, &failed)) {
        if ((*init)->kind == I_EXPR) {
            failed = init_expr(c, ty, (*init)->e, constant, 0, 1, at);
        } else {
            /* A list is one full expression of unsequenced items. */
            failed = init_check(c, ty, init, constant, 1, at);
            if (!failed && c->unevaluated == 0) {
                struct finding *f;
                uint n = sequence_check_init(*init, &f);
                for (uint i = 0; i < n; i++) {
                    runtime(c, diag_warning(f[i].at, f[i].msg, "unsequenced"), f[i].of);
                }
            }
        }
    }
    /* The declarator's spelling is its own initializer's only. */
    c->init_spelled = NULL;
    return failed;
}

static int literal_value(struct fnc *c, struct expr *e, int constant, struct type **lt, struct init **list,
                         int *found);

/* An array or struct initialized by a compound literal of its type, or by a
 * member of one: it is initialized by the literal's list, as clang has it
 * (merlin's init_by_literal). 0 if init is no such literal; else 1, with
 * *failed, the list replacing it, checked. */
static int init_by_literal(struct fnc *c, struct type **ty, struct init **init, int constant, struct pos at,
                           int *failed) {
    if ((*init)->kind != I_EXPR) {
        return 0;
    }
    struct expr *e = (*init)->e;
    int applies = 0;
    if ((*ty)->kind == TY_ARRAY) {
        applies = e->kind == E_COMPOUND && ty_is_array(e->ty);
    } else if ((*ty)->kind == TY_RECORD) {
        struct type *lt = literal_type_of(e);
        applies = constant && lt != NULL && ty_record(lt) != NULL;
    }
    if (!applies) {
        return 0;
    }
    struct type *lt;
    struct init *list;
    int found;
    if (literal_value(c, e, constant, &lt, &list, &found)) {
        *failed = 1;
        return 1;
    }
    if (!found) {
        return 0;
    }
    int fits;
    if ((*ty)->kind == TY_ARRAY && lt->kind == TY_ARRAY) {
        fits = ty_eq((*ty)->to, lt->to) &&
               (!(*ty)->has_len || ((*ty)->has_len == lt->has_len && (*ty)->len == lt->len));
    } else {
        fits = ty_eq(ty_unqual(*ty), ty_unqual(lt));
    }
    if (!fits) {
        const char *msg = ty_is_array(*ty) ? fmt("cannot initialize array of type '%s' with array of type '%s'",
                                                 ty_str(*ty), ty_str(lt))
                                           : fmt("initializing '%s' with an expression of incompatible type '%s'",
                                                 ty_str(*ty), ty_str(lt));
        *failed = fail(c, at, msg);
        return 1;
    }
    if (ty_is_array(*ty)) {
        *ty = lt;
    }
    *init = list;
    *failed = 0;
    return 1;
}

/* The type and the checked, fully braced list of a compound literal e, or of
 * a member of one, where a constant is needed (merlin's literal_value):
 * *found 0 for what is no such thing, or a member of a union other than the
 * one its list gives. e is left as it is. Nonzero if an error was reported. */
static int literal_value(struct fnc *c, struct expr *e, int constant, struct type **lt, struct init **list,
                         int *found) {
    *found = 0;
    if (e->kind == E_COMPOUND) {
        struct type *ty = e->ty;
        if (literal_type(c, ty, e->pos)) {
            return 1;
        }
        struct init *l = init_clone(e->init);
        if (init_check(c, &ty, &l, constant, 1, e->pos)) {
            return 1;
        }
        *lt = ty;
        *list = l;
        *found = 1;
        return 0;
    }
    if (e->kind == E_MEMBER && !e->flag && constant) {
        struct type *bt;
        struct init *bl;
        int f;
        if (literal_value(c, e->x, constant, &bt, &bl, &f)) {
            return 1;
        }
        if (!f) {
            return 0;
        }
        struct pos pos = e->pos;
        struct record *record = ty_record(bt);
        if (record == NULL) {
            return 0;
        }
        if (record->body == NULL) {
            return fail(c, pos, fmt("incomplete definition of type '%s'", record_name(record)));
        }
        int idx = -1;
        for (uint i = 0; i < record->body->nmembers; i++) {
            if (strcmp(record->body->members[i].name, e->name) == 0) {
                idx = (int)i;
                break;
            }
        }
        if (idx < 0) {
            return fail(c, pos, fmt("no member named '%s' in '%s'", e->name, record_name(record)));
        }
        struct type *mty = ty_qualified(record->body->members[idx].ty, ty_quals(bt));
        /* The fully braced list has an item per member up to the last one
         * given: the rest are zero. A union's list ends with the one member
         * it holds. */
        uint items = bl->items.n;
        if (record->is_union && (uint)idx + 1 != (items > 1 ? items : 1)) {
            return 0;
        }
        *lt = mty;
        *list = (uint)idx < items ? bl->items.v[idx] : zero_init(mty, bl->pos);
        *found = 1;
        return 0;
    }
    return 0;
}

/* A compound literal's value where a constant is needed and its object is
 * not: the value its list gives, which e becomes; *folded 0 if e is no such
 * literal of a scalar type. Nonzero if an error was reported. */
static int folded_literal(struct fnc *c, struct expr *e, int *folded) {
    *folded = 0;
    struct type *lt = literal_type_of(e);
    if (!c->constant || lt == NULL || !ty_is_scalar(lt)) {
        return 0;
    }
    struct type *t;
    struct init *l;
    int found;
    if (literal_value(c, e, 1, &t, &l, &found)) {
        return 1;
    }
    if (found) {
        /* A scalar's initializer is an expression. */
        *e = *l->e;
        *folded = 1;
    }
    return 0;
}

static int init_items(struct fnc *c, struct type **ty, struct init **init, int constant, int has_decl,
                      struct pos decl);

static int init_list(struct fnc *c, struct type **ty, struct init **init, int constant, int has_decl,
                     struct pos decl) {
    int list = (*init)->kind == I_LIST;
    c->list_depth += (uint)list;
    int failed = init_items(c, ty, init, constant, has_decl, decl);
    c->list_depth -= (uint)list;
    return failed;
}

/* Checks an initializer against the type it initializes, completing an
 * array of unknown size, and rewrites a list into its fully braced form
 * (merlin's init). constant: every element must be a constant expression.
 * decl is where the object initialized is declared, for clang's errors of
 * its whole initializer; a list in a list has none. */
static int init_check(struct fnc *c, struct type **ty, struct init **init, int constant, int has_decl,
                      struct pos decl) {
    /* A declaration's list: the warnings of its items' conversions come
     * once it is checked, last item first. */
    if ((*init)->kind == I_LIST && has_decl) {
        c->item_spelled = c->init_spelled;
        c->init_spelled = NULL;
    }
    if ((*init)->kind == I_LIST && has_decl && !c->has_conversions) {
        c->has_conversions = 1;
        memset(&c->conversions, 0, sizeof c->conversions);
        int failed = init_list(c, ty, init, constant, has_decl, decl);
        struct vec deferred = vec_take(&c->conversions);
        c->has_conversions = 0;
        for (uint i = deferred.n; i > 0; i--) {
            struct deferred *d = deferred.v[i - 1];
            if (d->has_of) {
                struct pending *p = arena_alloc(sizeof *p);
                p->d = d->d;
                p->of = d->of;
                vec_push(&c->pending, p);
            } else {
                push(c, d->d);
            }
        }
        heap_free(deferred.v);
        return failed;
    }
    return init_list(c, ty, init, constant, has_decl, decl);
}

/* Items of a braced list past what it initializes: clang's warning at the
 * first, and they are left out, after their expressions are checked. */
static void excess(struct fnc *c, struct init **rest, uint n, const char *what) {
    if (n == 0) {
        return;
    }
    struct pos at = init_pos(rest[0]);
    for (uint i = 0; i < n; i++) {
        if (rest[i]->kind == I_EXPR) {
            value(c, rest[i]->e, 0);
        }
    }
    push(c, diag_warning(at, fmt("excess elements in %s initializer", what), "excess-initializers"));
}

/* How fill fills an aggregate (merlin's Fill). */
enum { FILL_BRACED, FILL_ELIDED, FILL_DESIGNATED };

/* An element of an initializer list not yet matched to a subobject, and
 * whether its expression has been checked already. */
struct pend {
    struct init *init;
    int checked;
};

/* A queue of them (merlin's VecDeque): the items from `head` on. */
struct queue {
    struct pend *v;
    uint head;
    uint n;
    uint cap;
};

static int queue_empty(struct queue *q) {
    return q->head == q->n;
}

static struct pend *queue_front(struct queue *q) {
    return &q->v[q->head];
}

static struct pend queue_pop(struct queue *q) {
    return q->v[q->head++];
}

static void queue_push_front(struct queue *q, struct init *init) {
    if (q->head > 0) {
        q->head--;
        q->v[q->head].init = init;
        q->v[q->head].checked = 0;
        return;
    }
    q->v = grow(q->v, &q->cap, q->n + 1, sizeof(struct pend));
    for (uint i = q->n; i > 0; i--) {
        q->v[i] = q->v[i - 1];
    }
    q->n++;
    q->v[0].init = init;
    q->v[0].checked = 0;
}

static int fill(struct fnc *c, struct type **ty, struct queue *q, int constant, struct pos pos, int how,
                struct designator **rest, uint nrest, struct init *old, struct init **out);

static int is_char_array(struct type *t) {
    return t->kind == TY_ARRAY && is_char_array_elem(t->to);
}

/* char, signed char or unsigned char (qualified or not): the element types a
 * string literal may initialize. */
static int is_char_array_elem(struct type *t) {
    int k = ty_int_kind(t);
    return k == IK_CHAR || k == IK_SCHAR || k == IK_UCHAR;
}

/* merlin's init_items: init_list of the items. */
static int init_items(struct fnc *c, struct type **ty, struct init **init, int constant, int has_decl,
                      struct pos decl) {
    if ((*init)->kind != I_LIST) {
        return init_expr(c, ty, (*init)->e, constant, 0, 0, decl);
    }
    struct init *list = *init;
    struct pos pos = list->pos;
    struct type *t = *ty;
    int aggregate = ty_is_array(t) || ty_record(t) != NULL;
    /* {} for a scalar (C23's, which clang takes in C99 too) means 0. */
    if (list->items.n == 0 && !aggregate) {
        *init = init_expr_new(expr_num(i128_of(0), ty_int(IK_INT), pos));
        return init_expr(c, ty, (*init)->e, constant, 0, 0, decl);
    }
    struct init **items = (struct init **)list->items.v;
    /* { x } for a scalar, or { "text" } for a char array, means x. */
    int unwrap = 0;
    if (list->items.n == 1 && items[0]->kind == I_EXPR) {
        if (!aggregate) {
            unwrap = 1;
        } else if (items[0]->e->kind == E_STR) {
            unwrap = is_char_array(t);
        }
    }
    if (unwrap) {
        *init = items[0];
        return init_expr(c, ty, (*init)->e, constant, 0, 0, decl);
    }
    /* { "ab", 1 } for a char array: the string initializes it, and what
     * follows is past it, as clang has it. */
    if (is_char_array(t) && list->items.n > 0 && items[0]->kind == I_EXPR && items[0]->e->kind == E_STR) {
        *init = items[0];
        int failed = init_expr(c, ty, (*init)->e, constant, 0, 0, decl);
        excess(c, items + 1, list->items.n - 1, "char array");
        return failed;
    }
    if (!aggregate) {
        for (uint i = 0; i < list->items.n; i++) {
            if (items[i]->kind == I_DESIGNATED) {
                return fail(c, has_decl ? decl : pos,
                            fmt("initialization of non-aggregate type '%s' with a designated initializer list",
                                ty_str(t)));
            }
        }
        /* {{1}} for a scalar is 1, {1, 2} is 1: clang warns. */
        if (items[0]->kind == I_LIST) {
            push(c, diag_warning(items[0]->pos, "too many braces around scalar initializer",
                                 "many-braces-around-scalar-init"));
            *init = items[0];
            return init_check(c, ty, init, constant, 0, decl);
        }
        excess(c, items + 1, list->items.n - 1, "scalar");
        *init = items[0];
        return init_check(c, ty, init, constant, 0, decl);
    }
    struct queue q;
    memset(&q, 0, sizeof q);
    for (uint i = 0; i < list->items.n; i++) {
        q.v = grow(q.v, &q.cap, q.n + 1, sizeof(struct pend));
        q.v[q.n].init = items[i];
        q.v[q.n].checked = 0;
        q.n++;
    }
    list->items.n = 0;
    struct init *filled;
    if (fill(c, ty, &q, constant, pos, FILL_BRACED, NULL, 0, NULL, &filled)) {
        heap_free(q.v);
        return 1;
    }
    if (!queue_empty(&q)) {
        struct record *r = ty_record(*ty);
        const char *what = r != NULL ? record_keyword(r) : "array";
        struct init **rest = heap_alloc((q.n - q.head + 1) * sizeof(struct init *));
        uint nrest = 0;
        for (uint i = q.head; i < q.n; i++) {
            rest[nrest++] = q.v[i].init;
        }
        excess(c, rest, nrest, what);
        heap_free(rest);
    }
    heap_free(q.v);
    *init = filled;
    return 0;
}

/* Where an initializer begins as clang has it. */
static struct pos init_begin(struct fnc *c, struct init *i) {
    if (i->kind == I_EXPR) {
        return begin_written(c, i->e);
    }
    if (i->kind == I_LIST) {
        return i->pos;
    }
    return i->ds[0]->pos;
}

/* Where what initialized a subobject so far begins, if anything did (an
 * empty list is nothing). */
static int initialized(struct fnc *c, struct init *slot, struct pos *at) {
    if (slot == NULL || (slot->kind == I_LIST && slot->items.n == 0)) {
        return 0;
    }
    *at = init_begin(c, slot);
    return 1;
}

/* clang's warning of a subobject initialized again at `at`. */
static void overrides(struct fnc *c, struct pos at, struct pos prev) {
    struct diag *w =
        diag_warning(at, "initializer overrides prior initialization of this subobject", "initializer-overrides");
    push(c, diag_note(w, prev, "previous initialization is here"));
}

/* Array designators go up to this (exclusive). */
#define MAX_DESIGNATED (1 << 20)

/* Which subobject of ty a designator names: an element index, or a
 * member's; in *at. Nonzero if an error was reported. */
static int designator(struct fnc *c, struct type *ty, struct designator *d, uint *at) {
    if (!d->member) {
        struct pos pos = d->pos;
        struct type *u = ty_unqual(ty);
        if (u->kind != TY_ARRAY) {
            return fail(c, pos, fmt("array designator cannot initialize non-array type '%s'", ty_str(ty)));
        }
        struct expr *e = d->index;
        if (check_expr(c, e)) {
            return 1;
        }
        i128 i;
        if (!(consteval_int(e, &i) && ty_is_integer(e->ty))) {
            return fail(c, e->pos, "expression is not an integer constant expression");
        }
        if (i128_neg_p(i)) {
            return fail(c, e->pos, fmt("array designator value '%s' is negative", i128_str(i)));
        }
        if (u->has_len && i128_le(i128_of(u->len), i)) {
            return fail(c, e->pos,
                        fmt("array designator index (%s) exceeds array bounds (%u)", i128_str(i), u->len));
        }
        /* Each element up to the designated one gets an item, so the index
         * must be reasonable even when the array is big. */
        int64_t es = ty_size(u->to);
        i128 size = i128_of(es < 1 ? 1 : es);
        if (i128_le(i128_of(MAX_DESIGNATED), i) || i128_lt(i128_of(2147483647), i128_mul(i128_add(i, i128_of(1)), size))) {
            return fail(c, e->pos, fmt("array designator index (%s) is too large: merlin supports up to %u",
                                       i128_str(i), MAX_DESIGNATED - 1));
        }
        *at = (uint)i.lo;
        return 0;
    }
    struct record *r = ty_record(ty);
    if (r == NULL) {
        return fail(c, d->pos,
                    fmt("field designator cannot initialize a non-struct, non-union type '%s'", ty_str(ty)));
    }
    for (uint i = 0; i < r->body->nmembers; i++) {
        if (strcmp(r->body->members[i].name, d->name) == 0) {
            *at = i;
            return 0;
        }
    }
    return fail(c, d->at,
                fmt("field designator '%s' does not refer to any field in type '%s'", d->name, ty_str(ty)));
}

static int subobject(struct fnc *c, struct type **ty, struct queue *q, int constant, struct init **out);

/* Initializes the aggregate ty from the front of the queue: subobject after
 * subobject, as long as there are items and subobjects (merlin's fill). A
 * designator moves to the subobject it names, and initialization goes on
 * from the one after it. how is FILL_DESIGNATED with the rest of a
 * designation that ran into it and what it had been initialized with
 * (old, or NULL). The fully braced list in *out. */
static int fill(struct fnc *c, struct type **ty, struct queue *q, int constant, struct pos pos, int how,
                struct designator **rest, uint nrest, struct init *old, struct init **out) {
    struct record *record = ty_record(*ty);
    if (record != NULL && record->body == NULL) {
        return fail(c, pos, fmt("initializer for incomplete type '%s'", ty_str(*ty)));
    }
    int braced = how == FILL_BRACED;
    /* The designators still to go, from ds[di]. */
    struct designator **ds = rest;
    uint nds = how == FILL_DESIGNATED ? nrest : 0;
    uint di = 0;
    if (how != FILL_DESIGNATED) {
        old = NULL;
    }
    /* The characters a string gave, which a designation changes with
     * clang's warning of the string, not of each character. */
    int exploded = old != NULL && old->kind == I_EXPR && old->e->kind == E_STR;
    /* The initializer of each subobject so far (NULL: zero). */
    struct vec slots;
    memset(&slots, 0, sizeof slots);
    if (old != NULL && old->kind == I_LIST) {
        for (uint i = 0; i < old->items.n; i++) {
            vec_push(&slots, old->items.v[i]);
        }
    } else if (exploded) {
        /* { "ab", [0][3] = 'z' }: the string becomes its characters, one
         * item each, so the designation changes only the one it names. */
        struct expr *s = old->e;
        struct pos at = s->pos;
        struct type *elem = (*ty)->to;
        struct pos here = di < nds ? ds[di]->pos : pos;
        struct diag *w = diag_warning(here, "initializer partially overrides prior initialization of this subobject",
                                      "initializer-overrides");
        push(c, diag_note(w, at, "previous initialization is here"));
        for (uint i = 0; i < s->len; i++) {
            struct expr *ch = expr_new(E_NUM, at);
            ch->value = i128_of(s->bytes[i]);
            coerce(ch, elem);
            vec_push(&slots, init_expr_new(ch));
        }
    } else if (old != NULL && old->kind == I_EXPR) {
        /* { pv, .p.y = 9 } keeps pv's other members in C, which merlin does
         * not do: an error rather than a different meaning. */
        struct pos at = di < nds ? ds[di]->pos : pos;
        return fail(c, at,
                    fmt("designator partially overrides an initializer of type '%s' given by an expression; "
                        "merlin does not support this",
                        ty_str(old->e->ty)));
    }
    uint next = 0;
    for (;;) {
        if (di == nds) {
            if (queue_empty(q)) {
                break;
            }
            struct pend *front = queue_front(q);
            if (front->init->kind == I_DESIGNATED) {
                /* A designator belongs to the innermost braced list: an
                 * aggregate filled by brace elision ends there. */
                if (!braced) {
                    break;
                }
                struct pend p = queue_pop(q);
                ds = p.init->ds;
                nds = p.init->nds;
                di = 0;
                queue_push_front(q, p.init->inner);
            }
        }
        int designated = di < nds;
        int has_named = 0;
        struct pos named_at = pos;
        if (designated) {
            struct designator *first = ds[di++];
            has_named = 1;
            named_at = first->pos;
            if (designator(c, *ty, first, &next)) {
                return 1;
            }
        } else {
            int done;
            if ((*ty)->kind == TY_ARRAY) {
                done = (*ty)->has_len && next >= (*ty)->len;
            } else if (record->is_union) {
                done = next >= 1;
            } else {
                done = next >= record->body->nmembers;
            }
            if (done) {
                break;
            }
        }
        /* Inner dimensions are always known: the parser requires them. */
        struct type *sub = (*ty)->kind == TY_ARRAY ? (*ty)->to : record->body->members[next].ty;
        /* A member is as its declaration spelled it; an element as the array
         * is. */
        struct spelling *outer_spelled = c->item_spelled;
        if (record != NULL) {
            c->item_spelled = record->body->members[next].spelled;
        }
        while (slots.n <= next) {
            vec_push(&slots, NULL);
        }
        /* A subobject initialized again: clang's warning, before what
         * initializes it now is checked, with where it was before. Of a
         * union's other member, at the designator naming this one. */
        int is_union = record != NULL && record->is_union;
        if (is_union && has_named) {
            for (uint j = 0; j < slots.n; j++) {
                struct pos prev;
                if (j != next && initialized(c, slots.v[j], &prev)) {
                    overrides(c, named_at, prev);
                    break;
                }
            }
        }
        if (di == nds && !exploded) {
            struct pos prev;
            if (initialized(c, slots.v[next], &prev)) {
                overrides(c, init_begin(c, queue_front(q)->init), prev);
            }
        }
        struct init *init;
        if (di == nds) {
            if (subobject(c, &sub, q, constant, &init)) {
                return 1;
            }
        } else {
            /* .a.b = 1: a from .b = 1, then what follows. The list it makes
             * of a begins at .b, as clang's does, or where the one it
             * changes began. */
            struct designator **more = ds + di;
            uint nmore = nds - di;
            di = nds;
            struct pos at;
            if (!initialized(c, slots.v[next], &at)) {
                at = more[0]->pos;
            }
            struct init *prev = slots.v[next];
            slots.v[next] = NULL;
            if (fill(c, &sub, q, constant, at, FILL_DESIGNATED, more, nmore, prev, &init)) {
                return 1;
            }
        }
        c->item_spelled = outer_spelled;
        /* A union holds one member: the last one initialized. */
        if (is_union) {
            for (uint j = 0; j < slots.n; j++) {
                slots.v[j] = NULL;
            }
        }
        slots.v[next] = init;
        next++;
    }
    if ((*ty)->kind == TY_ARRAY && !(*ty)->has_len) {
        *ty = ty_array((*ty)->to, 1, slots.n);
    }
    /* A union's earlier member, dropped for a later one. */
    while (slots.n > 0 && vec_last(&slots) == NULL) {
        slots.n--;
    }
    struct init *list = init_list_new(pos);
    for (uint i = 0; i < slots.n; i++) {
        vec_push(&list->items, slots.v[i] != NULL ? slots.v[i] : init_list_new(pos));
    }
    heap_free(slots.v);
    *out = list;
    return 0;
}

/* The initializer of one subobject of type ty, from the front of the queue:
 * a braced list or an expression for the whole subobject, or, when it is
 * an aggregate the next expression does not initialize as a whole, as many
 * items as it takes (C's brace elision). */
static int subobject(struct fnc *c, struct type **ty, struct queue *q, int constant, struct init **out) {
    struct pend *front = queue_front(q);
    /* A struct initialized by a struct literal's list, where a constant is
     * needed. */
    if (!front->checked && ty_record(*ty) != NULL) {
        struct pos at = init_pos(front->init);
        int failed;
        if (init_by_literal(c, ty, &front->init, constant, at, &failed)) {
            if (failed) {
                return 1;
            }
            *out = queue_pop(q).init;
            return 0;
        }
    }
    int elide = 0;
    if (front->init->kind == I_EXPR) {
        struct expr *e = front->init->e;
        if (ty_is_array(*ty)) {
            elide = !(is_char_array(*ty) && e->kind == E_STR);
        } else if (ty_record(*ty) != NULL) {
            /* A struct is initialized by a struct of its type, or member by
             * member. */
            if (!front->checked) {
                front->checked = 1;
                if (value(c, e, constant)) {
                    return 1;
                }
            }
            elide = !ty_eq(ty_unqual(e->ty), ty_unqual(*ty));
        }
    }
    if (elide) {
        /* The list it makes begins where its first item does. */
        struct pos pos = init_begin(c, queue_front(q)->init);
        return fill(c, ty, q, constant, pos, FILL_ELIDED, NULL, 0, NULL, out);
    }
    struct pend p = queue_pop(q);
    struct init *init = p.init;
    if (init->kind == I_EXPR) {
        if (init_expr(c, ty, init->e, constant, p.checked, 0, init->e->pos)) {
            return 1;
        }
    } else if (init_check(c, ty, &init, constant, 0, init->pos)) {
        return 1;
    }
    *out = init;
    return 0;
}

/* An object of type ty initialized by the expression e (already checked,
 * if checked): a string literal for a char array, otherwise as if by
 * assignment. has_decl when e is a declarator's initializer (at decl), not
 * an item of a list: clang reports a conversion there, and an item where it
 * begins. */
static int init_expr(struct fnc *c, struct type **ty, struct expr *e, int constant, int checked, int has_decl,
                     struct pos decl) {
    struct pos at = has_decl ? decl : begin_written(c, e);
    struct type *t = *ty;
    if (t->kind == TY_ARRAY && is_char_array_elem(t->to)) {
        if (e->kind != E_STR) {
            return fail(c, at, "array initializer must be an initializer list or string literal");
        }
        /* The terminating NUL may be left out when the array is exactly as
         * long as the text; a longer text is cut to the array's size, with
         * clang's warning. */
        if (!t->has_len) {
            *ty = ty_array(t->to, 1, e->len + 1);
        } else if (e->len > t->len) {
            push(c, diag_warning(e->pos, "initializer-string for char array is too long", "excess-initializers"));
            e->len = t->len;
        }
        e->ty = ty_array(ty_int(IK_CHAR), 1, e->len + 1);
        return 0;
    }
    if (t->kind == TY_ARRAY) {
        /* clang's words for an array that is no char array: int's is C99's
         * wchar_t on RISC-V, a wide string's element type. */
        int wide = ty_eq(t->to, ty_int(IK_INT));
        const char *msg;
        if (e->kind == E_STR && wide) {
            msg = "initializing wide char array with non-wide string literal";
        } else if (wide) {
            msg = "array initializer must be an initializer list or wide string literal";
        } else {
            msg = "array initializer must be an initializer list";
        }
        return fail(c, at, msg);
    }
    if (!checked && value(c, e, constant)) {
        return 1;
    }
    /* How the object initialized was spelled: the declaration's, or of an
     * item of a list, what it is an element or member of. */
    struct spelling *spelled;
    if (has_decl) {
        spelled = c->init_spelled;
        c->init_spelled = NULL;
    } else {
        spelled = c->item_spelled;
    }
    if (convert(c, e, t, spelled, CONV_INIT, at)) {
        return 1;
    }
    coerce(e, t);
    full_expression(c, e);
    /* A declaration's own initializer, not an item of a list. */
    if (has_decl) {
        bounds(c, e);
    }
    struct cval v;
    if (constant && !consteval_eval(e, &v)) {
        return fail(c, expr_loc(e), "initializer element is not a compile-time constant");
    }
    return 0;
}

/* ---- Conversions ---- */

static void enum_to_enum(struct fnc *c, struct expr *e, struct type *to, struct spelling *to_spelled);
static int convert_value(struct fnc *c, struct expr *e, struct type *from, const char *named,
                         struct spelling *from_spelled, int null, struct type *to, struct spelling *to_spelled,
                         int how, struct pos at);

/* Checks whether e's value may be converted to `to` as if by assignment,
 * with clang's diagnostics at `at`. */
static int convert(struct fnc *c, struct expr *e, struct type *to, struct spelling *to_spelled, int how,
                   struct pos at) {
    int null = is_null_pointer(e);
    struct type *from = e->ty;
    struct spelling *from_spelled = spelled_of(c, e);
    char *named = ty_named(from, from_spelled);
    if (convert_value(c, e, from, named, from_spelled, null, to, to_spelled, how, at)) {
        return 1;
    }
    uint errors = c->errors->n;
    uint pending = c->pending.n;
    constant_conversion(c, e, to);
    /* An argument's, after the note on its parameter. */
    if (how != CONV_ARG) {
        enum_to_enum(c, e, to, to_spelled);
    }
    /* Of an item of a list, kept to come with the list's. */
    if (c->has_conversions) {
        for (uint i = c->errors->n; i > errors; i--) {
            struct deferred *d = arena_alloc(sizeof *d);
            d->d = c->errors->v[i - 1];
            vec_push(&c->conversions, d);
        }
        c->errors->n = errors;
        for (uint i = c->pending.n; i > pending; i--) {
            struct pending *p = c->pending.v[i - 1];
            struct deferred *d = arena_alloc(sizeof *d);
            d->d = p->d;
            d->has_of = 1;
            d->of = p->of;
            vec_push(&c->conversions, d);
        }
        c->pending.n = pending;
    }
    /* Converted to _Bool, it is taken as true or false. */
    if (ty_eq(ty_unqual(to), ty_int(IK_BOOL)) && how != CONV_ARG) {
        bool_like(c, e);
    }
    return 0;
}

/* One enum's value converted to another enum: clang's warning. */
static void enum_to_enum(struct fnc *c, struct expr *e, struct type *to, struct spelling *to_spelled) {
    struct expr *v = strip_implicit(e);
    struct spelling *spelled = spelled_of(c, v);
    struct enum_type *source;
    if (v->kind == E_ENUMERATOR && v->spelled != NULL && v->spelled->kind == SP_ENUM) {
        source = v->spelled->en;
    } else {
        source = enum_of(spelled, v->ty);
    }
    struct enum_type *dest = enum_of(to_spelled, to);
    if (source != NULL && dest != NULL && source != dest) {
        push(c, diag_warning(expr_loc(v),
                             fmt("implicit conversion from enumeration type '%s' to different enumeration type %s",
                                 enum_name(source), ty_named(to, to_spelled)),
                             "implicit-enum-enum-cast"));
    }
}

/* An argument converted to its parameter's type: at the argument, with
 * clang's note on the parameter (param, or NULL) of a function called by
 * name. */
static int convert_arg(struct fnc *c, struct expr *e, struct type *to, struct spelling *to_spelled,
                       struct sig_param *param) {
    uint mark = c->errors->n;
    int failed = convert(c, e, to, to_spelled, CONV_ARG, begin_written(c, e));
    if (param != NULL && c->errors->n > mark) {
        const char *what = param->name[0] == 0 ? "passing argument to parameter here"
                                               : fmt("passing argument to parameter '%s' here", param->name);
        diag_note(vec_last(c->errors), param->pos, what);
    }
    if (!failed) {
        enum_to_enum(c, e, to, to_spelled);
        if (ty_eq(ty_unqual(to), ty_int(IK_BOOL))) {
            bool_like(c, e);
        }
    }
    return failed;
}

/* Checks whether a value of type from (named in messages, quotes
 * included), the value of e (a null pointer constant if null), may be
 * converted to `to` as if by assignment (C99 6.5.16.1), reporting what
 * clang reports at `at` (merlin's convert_value, whose comment lists the
 * rules). */
static int convert_value(struct fnc *c, struct expr *e, struct type *from, const char *named,
                         struct spelling *from_spelled, int null, struct type *to, struct spelling *to_spelled,
                         int how, struct pos at) {
    to = ty_unqual(to);
    char *to_named = ty_named(to, to_spelled);
    struct type *val = ty_decay(from);
    const char *problem = "";
    const char *flag = NULL;
    int tk = to->kind;
    int vk = val->kind;
    if (tk == TY_INT && vk == TY_INT) {
        return 0;
    }
    if (tk == TY_INT && to->ik == IK_BOOL && vk == TY_PTR) {
        return 0;
    }
    if (tk == TY_RECORD && vk == TY_RECORD && to->rec == val->rec) {
        if (ty_size(to) < 0) {
            return fail(c, at, fmt("incomplete type '%s' cannot be copied", ty_str(to)));
        }
        return 0;
    }
    if (tk == TY_PTR && (vk == TY_PTR || vk == TY_INT) && null) {
        return 0;
    }
    if (tk == TY_PTR && vk == TY_PTR) {
        struct type *a = to->to;
        struct type *b = val->to;
        /* What they point to without the qualifiers, an array's elements'
         * too, which only the qualifiers' check is of. */
        struct type *au = ty_without_quals(a);
        struct type *bu = ty_without_quals(b);
        int plain;
        if (ty_is_func(au) && ty_is_func(bu)) {
            if (agree(au, to_spelled, bu, from_spelled)) {
                return 0;
            }
            problem = "incompatible function pointer types";
            flag = "incompatible-function-pointer-types";
        } else if (enums_differ(au, to_spelled, bu, from_spelled) || distinct_enums(au, to_spelled, bu, from_spelled)) {
            problem = "incompatible pointer types";
            flag = "incompatible-pointer-types";
        } else if (ty_compatible(au, bu) || ty_is_void(au) || ty_is_void(bu)) {
            uint aq = ty_quals(a);
            uint bq = ty_quals(b);
            if ((aq & bq) != bq) {
                push(c, diag_warning(at, fmt("%s discards qualifiers", conv_phrase(how, to_named, named)),
                                     "incompatible-pointer-types-discards-qualifiers"));
            }
            return 0;
        } else if ((plain = sign_pair(au, bu)) >= 0) {
            /* Pointers to the signed and unsigned of one integer type:
             * clang's warning, and the conversion. */
            const char *why = plain ? "where one is of the unique plain 'char' type and the other is not"
                                    : "with different sign";
            push(c, diag_warning(at,
                                 fmt("%s converts between pointers to integer types %s",
                                     conv_phrase(how, to_named, named), why),
                                 "pointer-sign"));
            return 0;
        } else if (ty_similar(au, bu)) {
            /* const char ** from char **: the same but for qualifiers
             * further in, which clang only warns of. */
            push(c, diag_warning(at,
                                 fmt("%s discards qualifiers in nested pointer types",
                                     conv_phrase(how, to_named, named)),
                                 "incompatible-pointer-types-discards-qualifiers"));
            return 0;
        } else {
            problem = "incompatible pointer types";
            flag = "incompatible-pointer-types";
        }
    } else if (tk == TY_PTR && vk == TY_INT) {
        problem = "incompatible integer to pointer conversion";
        flag = "int-conversion";
    } else if (tk == TY_INT && vk == TY_PTR) {
        problem = "incompatible pointer to integer conversion";
        flag = "int-conversion";
    }
    const char *hint = conversion_hint(e, from, to);
    const char *msg = problem[0] == 0 ? fmt("%s%s", conv_incompatible(how, to_named, named), hint)
                                      : fmt("%s %s%s", problem, conv_phrase(how, to_named, named), hint);
    struct diag *d = diag_error(at, msg);
    /* One of the warnings clang makes errors: it converts the value all
     * the same, and goes on with it. */
    if (flag != NULL) {
        d->flag = flag;
        push(c, d);
        return 0;
    }
    push(c, d);
    return 1;
}

/* ---- Assignability ---- */

static struct diag *const_assignment(struct fnc *c, struct expr *target, struct pos at);

/* The target of =, op=, ++ and -- (whose operator is at `at`): a modifiable
 * lvalue. */
static int assignable(struct fnc *c, struct expr *target, struct pos at) {
    struct type *ty = target->ty;
    struct diag *e;
    if (!is_lvalue(target)) {
        e = diag_error(at, "expression is not assignable");
    } else if (ty_is_func(ty)) {
        e = diag_error(at, fmt("non-object type '%s' is not assignable", ty_str(ty)));
    } else if (ty_is_array(ty)) {
        e = diag_error(at, fmt("array type '%s' is not assignable", ty_str(ty)));
    } else if (ty_quals(ty) & Q_CONST) {
        e = const_assignment(c, target, at);
    } else if ((e = const_fields(target, at)) == NULL) {
        return 0;
    }
    push(c, e);
    return 1;
}

static int declared(struct fnc *c, const char *name, struct type **ty, struct pos *pos);

/* Whether a type, once through a -> if deref, is not const. */
static int modifiable(struct type *t, int deref) {
    struct type *u = ty_unqual(t);
    if (deref && u->kind == TY_PTR) {
        return !(ty_quals(u->to) & Q_CONST);
    }
    return !(ty_quals(t) & Q_CONST);
}

/* The member a member access names, or NULL. */
static struct member *member_of(struct expr *e) {
    struct type *outer = e->flag ? ty_pointee(ty_decay(e->x->ty)) : e->x->ty;
    struct record *r = outer != NULL ? ty_record(outer) : NULL;
    if (r == NULL || r->body == NULL) {
        return NULL;
    }
    return record_member(r->body, e->name);
}

/* clang's error for assigning to a const lvalue target, as its
 * DiagnoseConstAssignment finds it: it walks from the lvalue to the variable
 * it is part of, through members, and names the first thing whose type is
 * const, with a note on each such member or variable; anything else is a
 * read-only variable. */
static struct diag *const_assignment(struct fnc *c, struct expr *target, struct pos at) {
    struct expr *e = target;
    int deref = 0;
    struct diag *found = NULL;
    struct vec notes;
    memset(&notes, 0, sizeof notes);
    for (;;) {
        if (e->kind == E_MEMBER) {
            struct member *m = member_of(e);
            if (m != NULL && !modifiable(m->ty, deref)) {
                if (found == NULL) {
                    found = diag_error(at, fmt("cannot assign to non-static data member '%s' with const-qualified "
                                               "type '%s'",
                                               e->name, ty_str(m->ty)));
                }
                vec_push(&notes, pos_box(m->pos));
                vec_push(&notes, fmt("non-static data member '%s' declared const here", e->name));
            }
            deref = e->flag;
            e = e->x;
            continue;
        }
        if (e->kind == E_VAR) {
            struct type *ty;
            struct pos pos;
            if (declared(c, e->name, &ty, &pos) && !modifiable(ty, deref)) {
                if (found == NULL) {
                    found = diag_error(at, fmt("cannot assign to variable '%s' with const-qualified type '%s'",
                                               e->name, ty_str(ty)));
                }
                vec_push(&notes, pos_box(pos));
                vec_push(&notes, fmt("variable '%s' declared const here", e->name));
            }
        }
        break;
    }
    if (found == NULL) {
        heap_free(notes.v);
        return diag_error(at, "read-only variable is not assignable");
    }
    for (uint i = 0; i < notes.n; i += 2) {
        diag_note(found, *(struct pos *)notes.v[i], notes.v[i + 1]);
    }
    heap_free(notes.v);
    return found;
}

/* Whether the lvalue e is (a member of) a register object. */
static int in_register(struct fnc *c, struct expr *e) {
    if (e->kind == E_VAR && e->var.kind == VAR_LOCAL) {
        return intmap_has(&c->registers, e->var.local);
    }
    if (e->kind == E_MEMBER && !e->flag) {
        return in_register(c, e->x);
    }
    return 0;
}

/* How the type of e was spelled, where its declaration says (merlin's
 * spelled_of). */
static struct spelling *spelled_of(struct fnc *c, struct expr *e) {
    switch (e->kind) {
    case E_VAR: {
        struct binding *b = lookup(c, e->name);
        if (b != NULL) {
            return posmap_get(&c->spellings, b->pos);
        }
        struct global *g = visible_global(c, e->name);
        return g != NULL ? g->spelled : NULL;
    }
    case E_ADDR:
    case E_DEREF:
    case E_CALL:
    case E_ASSIGN:
        return spelled_of(c, e->x);
    case E_COMMA:
        return spelled_of(c, e->y);
    case E_BINARY:
        if ((e->op == OP_ADD || e->op == OP_SUB) && ty_is_ptr(e->ty)) {
            return spelled_of(c, ty_is_ptr(ty_decay(e->x->ty)) ? e->x : e->y);
        }
        return NULL;
    case E_CAST:
        /* An array or a function as a pointer is still as spelled; any
         * other implicit conversion makes a type of its own. */
        if (e->implicit) {
            int k = ty_unqual(e->x->ty)->kind;
            return k == TY_ARRAY || k == TY_FUNC ? spelled_of(c, e->x) : NULL;
        }
        return e->spelled;
    case E_MEMBER: {
        struct member *m = member_of(e);
        return m != NULL ? m->spelled : NULL;
    }
    default:
        return NULL;
    }
}

/* How the declaration at pos spelled its type. */
static struct spelling *spelling_at(struct fnc *c, struct pos pos) {
    return posmap_get(&c->spellings, pos);
}

/* Whether d redeclares a name its block declared already in a way C does
 * not allow, said as clang's MergeVarDecl says it (merlin's
 * same_scope_conflict). */
static int same_scope_conflict(struct fnc *c, struct declarator *d) {
    const char *name = d->name;
    struct binding *b = map_get(vec_last(&c->scopes), name);
    if (b == NULL) {
        return 0;
    }
    struct pos prev = b->pos;
    /* A declaration with linkage is bound to the name as its symbol; a
     * static local's symbol is its own. */
    int linked = b->var.kind == VAR_GLOBAL && strcmp(b->var.sym, name) == 0;
    int static_local = b->var.kind == VAR_GLOBAL && !linked;
    int external = d->storage == ST_EXTERN || ty_is_func(d->ty);
    const char *msg;
    const char *note;
    if (ty_is_func(b->ty) != ty_is_func(d->ty)) {
        msg = fmt("redefinition of '%s' as different kind of symbol", name);
        note = "previous definition is here";
    } else if (ty_is_func(b->ty)) {
        return 0;
    } else if (!agree(b->ty, spelling_at(c, prev), d->ty, d->spelled)) {
        msg = fmt("%s of '%s' with a different type: %s vs %s", external ? "redeclaration" : "redefinition", name,
                  ty_named(d->ty, d->spelled), ty_named(b->ty, spelling_at(c, prev)));
        note = previous_what(!linked);
    } else if (external && linked) {
        return 0;
    } else if (static_local && d->storage != ST_STATIC) {
        msg = fmt("non-static declaration of '%s' follows static declaration", name);
        note = "previous definition is here";
    } else if (external) {
        msg = fmt("extern declaration of '%s' follows non-extern declaration", name);
        note = "previous definition is here";
    } else if (linked) {
        msg = fmt("non-extern declaration of '%s' follows extern declaration", name);
        note = "previous declaration is here";
    } else {
        return 0;
    }
    push(c, diag_note(diag_error(d->pos, msg), prev, note));
    return 1;
}

/* A global's type and where its latest declaration is, and whether that is
 * a definition (merlin's Global::declaration). */
static struct type *global_declaration(struct global *g, struct pos *pos, int *definition) {
    *pos = g->pos;
    if (g->is_func) {
        *definition = g->has_def && pos_eq(g->def, g->pos);
    } else {
        *definition = g->defines;
    }
    return g->ty;
}

/* clang's error for a block's declaration with linkage d that does not
 * agree with an earlier one of its name: one with linkage, in a block of
 * this function or of an earlier one, or at file scope. Whether it had
 * one. Otherwise d gets the composite type with the visible earlier one. */
static int linkage_redeclared(struct fnc *c, struct declarator *d) {
    const char *name = d->name;
    struct type *ty = NULL;
    struct pos pos;
    int definition = 0;
    struct spelling *spelled = NULL;
    for (uint i = c->linked.n; i > 0; i--) {
        struct linked *l = c->linked.v[i - 1];
        if (strcmp(l->name, name) == 0) {
            ty = l->ty;
            pos = l->pos;
            spelled = l->spelled;
            break;
        }
    }
    if (ty == NULL) {
        struct global *g = global_of(c, name);
        if (g == NULL) {
            return 0;
        }
        ty = global_declaration(g, &pos, &definition);
        spelled = g->spelled;
    }
    /* C23's conflict of one without a prototype and one with, before
     * whatever else is said of them. */
    if (ty_is_func(ty) && ty_is_func(d->ty)) {
        struct diag *w = deprecated_conflict(ty->fn, pos, d->ty->fn, d->pos, 0);
        if (w != NULL) {
            push(c, w);
        }
    }
    int agrees = agree(ty, spelled, d->ty, d->spelled);
    const char *msg;
    if (ty_is_func(ty) && ty_is_func(d->ty) && !agrees) {
        msg = fmt("conflicting types for '%s'", name);
    } else if (!ty_is_func(ty) && !ty_is_func(d->ty) && !agrees) {
        msg = fmt("redeclaration of '%s' with a different type: %s vs %s", name, ty_named(d->ty, d->spelled),
                  ty_named(ty, spelled));
    } else if (ty_is_func(ty) != ty_is_func(d->ty)) {
        struct diag *e = diag_error(d->pos, fmt("redefinition of '%s' as different kind of symbol", name));
        push(c, diag_note(e, pos, "previous definition is here"));
        return 1;
    } else {
        /* The name has the type it makes together with the earlier
         * declaration that is visible, if one is (C99 6.2.7p4). */
        struct type *visible = NULL;
        struct binding *b = lookup(c, name);
        if (b != NULL) {
            if (b->var.kind == VAR_GLOBAL && strcmp(b->var.sym, name) == 0) {
                visible = b->ty;
            }
        } else {
            struct global *g = visible_global(c, name);
            if (g != NULL) {
                struct pos p2;
                int d2;
                visible = global_declaration(g, &p2, &d2);
            }
        }
        if (visible != NULL) {
            d->ty = ty_composite(visible, d->ty);
        }
        return 0;
    }
    push(c, diag_note(diag_error(d->pos, msg), pos, previous_what(definition)));
    return 1;
}

/* The type and the position of the declaration a name in scope refers to:
 * a local's, or a global's latest. */
static int declared(struct fnc *c, const char *name, struct type **ty, struct pos *pos) {
    struct binding *b = lookup(c, name);
    if (b != NULL) {
        *ty = b->ty;
        *pos = b->pos;
        return 1;
    }
    struct global *g = visible_global(c, name);
    if (g == NULL) {
        return 0;
    }
    *ty = g->ty;
    *pos = g->pos;
    return 1;
}

/* ---- Expressions ---- */

static int incomplete_arithmetic(struct fnc *c, struct pos pos, struct type *to);

/* clang's error for operands l and r that the binary operator op at pos
 * does not take. */
static int invalid_operands(struct fnc *c, int op, struct pos pos, struct type *l, struct type *r) {
    struct type *lv = ty_decay(l);
    struct type *rv = ty_decay(r);
    struct type *lu = ty_unqual(lv);
    struct type *ru = ty_unqual(rv);
    /* The difference of pointers to different types. */
    if (op == OP_SUB && lu->kind == TY_PTR && ru->kind == TY_PTR && ty_step(lv) >= 0 && ty_step(rv) >= 0) {
        return fail(c, pos, fmt("'%s' and '%s' are not pointers to compatible types", ty_str(lv), ty_str(rv)));
    }
    /* Arithmetic on a pointer to an incomplete type. */
    struct type *to = NULL;
    if ((op == OP_ADD || op == OP_SUB) && lu->kind == TY_PTR && ru->kind == TY_INT) {
        to = lu->to;
    } else if (op == OP_ADD && lu->kind == TY_INT && ru->kind == TY_PTR) {
        to = ru->to;
    } else if (op == OP_SUB && lu->kind == TY_PTR && ru->kind == TY_PTR) {
        to = lu->to;
    }
    if (to != NULL && (ty_step(lv) < 0 || ty_step(rv) < 0)) {
        return incomplete_arithmetic(c, pos, to);
    }
    return fail(c, pos, fmt("invalid operands to binary expression ('%s' and '%s')", ty_str(l), ty_str(r)));
}

/* clang's error for a variable (or a compound literal) of the incomplete
 * type ty declared at pos. */
static void incomplete_variable(struct fnc *c, struct pos pos, struct type *ty) {
    if (ty->kind == TY_ARRAY && !ty->has_len) {
        fail(c, pos, "definition of variable with array type needs an explicit size or an initializer");
        return;
    }
    struct diag *e = diag_error(pos, fmt("variable has incomplete type '%s'", ty_str(ty)));
    struct record *r = ty_record(ty);
    if (r != NULL) {
        diag_note(e, r->pos, fmt("forward declaration of '%s'", record_name(r)));
    }
    push(c, e);
}

/* clang's error for arithmetic on a pointer to the incomplete type to. */
static int incomplete_arithmetic(struct fnc *c, struct pos pos, struct type *to) {
    struct diag *e = diag_error(pos, fmt("arithmetic on a pointer to an incomplete type '%s'", ty_str(to)));
    struct record *r = ty_record(to);
    if (r != NULL) {
        diag_note(e, r->pos, fmt("forward declaration of '%s'", record_name(r)));
    }
    push(c, e);
    return 1;
}

/* check_expr of an initializer's expression, constant if constant. */
static int value(struct fnc *c, struct expr *e, int constant) {
    int outer = c->constant;
    c->constant = constant;
    int failed = check_expr(c, e);
    c->constant = outer;
    return failed;
}

static int va_end_or_copy(struct fnc *c, struct expr *e);
static void precedence(struct fnc *c, int op, struct pos pos, struct expr *l, struct expr *r);
static int binary(struct fnc *c, int op, struct expr *l, struct expr *r, struct pos pos, uint64_t id,
                  struct type **out);
static void bad_shift_or_divisor(struct fnc *c, int op, struct pos pos, uint64_t id, struct type *t,
                                 struct expr *value);
static int sizeof_value(struct fnc *c, struct expr *e, struct type *t, struct pos at);
static int cond_type(struct fnc *c, struct expr *a, struct expr *b, struct pos pos, struct type **out);
static int variadic_args(struct fnc *c, struct expr **args, int *copies, uint n);
static int va_start(struct fnc *c, struct expr *ap, struct expr *last, struct pos pos);
static int va_arg(struct fnc *c, struct expr *ap, struct type *t, struct pos at, uint64_t id);
static char *printed_init(struct fnc *c, struct init *init);
static int static_literal(struct fnc *c, struct type **ty, struct init *init, struct pos pos, const char **name);
static int compound_literal(struct fnc *c, struct type **ty, struct init **init, struct pos pos, uint *local);

static struct pos begin_written_cb(struct fnc *c, struct expr *e) {
    return begin_written(c, e);
}

/* The checks of a call (merlin's expr, of ExprKind::Call): the type in *out. */
static int call(struct fnc *c, struct expr *e, struct type **out) {
    struct pos pos = e->pos;
    struct expr *callee = e->x;
    if (callee->kind == E_VAR && lookup(c, callee->name) == NULL && visible_global(c, callee->name) == NULL) {
        struct diag *d = diag_error(pos, fmt("call to undeclared function '%s'; ISO C99 and later do not support "
                                             "implicit function declarations",
                                             callee->name));
        d->flag = "implicit-function-declaration";
        /* clang's typo correction, in notes. */
        struct correction *cr = posmap_get(&c->c->written.corrections, callee->pos);
        if (cr != NULL) {
            diag_note(d, callee->pos, fmt("did you mean '%s'?", cr->name));
            diag_note(d, cr->pos, fmt("'%s' declared here", cr->name));
        }
        push(c, d);
        return 1;
    }
    int callee_failed = check_expr(c, callee);
    int ok = 1;
    for (uint i = 0; i < e->nargs; i++) {
        if (check_expr(c, e->args[i])) {
            ok = 0;
        }
    }
    if (callee_failed) {
        return 1;
    }
    /* A function name decays to a pointer to the function, so f(x), fp(x)
     * and (*fp)(x) all call through a pointer. */
    struct type *ct = ty_decay(callee->ty);
    if (!(ct->kind == TY_PTR && ty_is_func(ct->to))) {
        return fail(c, e->at, fmt("called object type '%s' is not a function or function pointer", ty_str(ct)));
    }
    struct functype *ft = ct->to->fn;
    /* The function called by name, whose declaration clang's notes point at
     * and its messages name the parameters of. A function declared in a
     * block is the declaration a call names. */
    const char *direct = direct_callee(callee);
    struct global *sig = NULL;
    if (direct != NULL) {
        struct binding *b = lookup(c, direct);
        if (b != NULL && ty_is_func(b->ty)) {
            /* Named and placed by the block's declaration, defined as the
             * function is. */
            sig = arena_alloc(sizeof *sig);
            sig->is_func = 1;
            sig->ty = b->ty;
            sig->pos = b->pos;
            sig->begin = b->pos;
            sig->spelled = spelling_at(c, b->pos);
            struct global *g = global_of(c, direct);
            if (g != NULL && g->is_func) {
                sig->has_def = g->has_def;
                sig->def = g->def;
                sig->defined = g->defined;
                sig->bare_def = g->bare_def;
            }
        } else {
            struct global *g = global_of(c, direct);
            if (g != NULL && g->is_func) {
                sig = g;
            }
        }
    }
    if (unprototyped(ft)) {
        /* No prototype: the arguments are promoted, as `...`'s are, and
         * passed as named ones. To a definition with (), which takes none,
         * clang warns of them; and of any to what has no prototype. */
        if (e->nargs > 0 && sig != NULL && sig->bare_def != NULL) {
            push(c, diag_warning_unflagged(e->at2, fmt("too many arguments in call to '%s'", direct)));
        }
        if (e->nargs > 0) {
            const char *to = sig != NULL ? fmt("'%s'", direct) : "a function";
            push(c, diag_warning(e->at,
                                 fmt("passing arguments to %s without a prototype is deprecated in all versions of "
                                     "C and is not supported in C23",
                                     to),
                                 "deprecated-non-prototype"));
        }
    } else {
        struct diag *d = arity_error(c, ft, e->args, e->nargs, direct, sig, e->at2, begin_written_cb);
        if (d != NULL) {
            push(c, d);
            return 1;
        }
    }
    if (!ok) {
        return 1;
    }
    e->narg_copies = e->nargs;
    e->arg_copies = arena_alloc(e->nargs * sizeof(int) + 1);
    for (uint i = 0; i < e->nargs; i++) {
        e->arg_copies[i] = -1;
    }
    uint arity = ft->nparams;
    for (uint i = 0; i < e->nargs && i < arity; i++) {
        struct expr *a = e->args[i];
        struct type *p = ft->params[i];
        /* Each argument converts to its parameter's type as if assigned,
         * with clang's note on the parameter. */
        struct sig_param *param = sig != NULL && i < sig->nparams ? &sig->params[i] : NULL;
        /* The parameter's spelling: its declaration's, else the function
         * type's (a pointer's). */
        struct spelling *spelled = param != NULL ? param->spelled : i < ft->nspelled ? ft->spelled[i].spelled : NULL;
        if (convert_arg(c, a, p, spelled, param)) {
            ok = 0;
            continue;
        }
        bounds(c, a);
        coerce(a, p);
        /* A struct too big for registers is passed as the address of a
         * copy. */
        if (ty_record(p) != NULL && abi_classify(p) == PASS_REF) {
            e->arg_copies[i] = (int)temp(c, p, "argument copy", a->pos);
        }
    }
    /* The rest: the arguments `...` takes (the arity errors above leave
     * none too few). */
    if (!variadic_args(c, e->args + arity, e->arg_copies + arity, e->nargs - arity)) {
        ok = 0;
    }
    if (!ok) {
        return 1;
    }
    if (ty_record(ft->ret) != NULL) {
        if (ty_size(ft->ret) < 0) {
            struct diag *d;
            if (sig != NULL) {
                d = diag_error(pos, fmt("calling '%s' with incomplete return type '%s'", direct, ty_str(ft->ret)));
                diag_note(d, sig->pos, fmt("'%s' declared here", direct));
            } else {
                d = diag_error(pos, fmt("calling function with incomplete return type '%s'", ty_str(ft->ret)));
            }
            struct record *r = ty_record(ft->ret);
            diag_note(d, r->pos, fmt("forward declaration of '%s'", record_name(r)));
            push(c, d);
            return 1;
        }
        /* A struct result is kept in a hidden local of the caller. */
        e->result = (int)temp(c, ty_unqual(ft->ret), "call result", pos);
    }
    /* A value, of the result type without its qualifiers. */
    *out = ty_unqual(ft->ret);
    return 0;
}

/* The checks of a member access (`.`, `->`): the type in *out. */
static int member(struct fnc *c, struct expr *e, int addressed, struct type **out) {
    struct pos pos = e->pos;
    /* &s.m takes the address of a part of s. */
    c->address_taken = addressed && !e->flag;
    if (check_expr(c, e->x)) {
        return 1;
    }
    /* p->m reaches through a pointer; s.m names a part of s. clang reports
     * what is wrong with the base at the `.` or `->`, a member it cannot
     * find at the member's name. */
    struct type *bt = ty_decay(e->x->ty);
    struct type *outer;
    uint quals;
    if (e->flag && bt->kind == TY_PTR) {
        outer = bt->to;
        quals = ty_quals(bt->to);
    } else if (e->flag) {
        int rec = ty_record(bt) != NULL;
        const char *msg =
            fmt("member reference type '%s' is not a pointer%s", ty_str(bt), rec ? "; did you mean to use '.'?" : "");
        /* At the -> with the hint, else at the member's name. */
        return fail(c, rec ? pos : e->at, msg);
    } else if (bt->kind == TY_PTR && ty_record(bt->to) != NULL) {
        return fail(c, pos, fmt("member reference type '%s' is a pointer; did you mean to use '->'?", ty_str(bt)));
    } else {
        outer = e->x->ty;
        quals = ty_quals(e->x->ty);
    }
    struct record *record = ty_record(outer);
    if (record == NULL) {
        return fail(c, pos, fmt("member reference base type '%s' is not a structure or union", ty_str(outer)));
    }
    if (record->body == NULL) {
        struct diag *d = diag_error(pos, fmt("incomplete definition of type '%s'", record_name(record)));
        push(c, diag_note(d, record->pos, fmt("forward declaration of '%s'", record_name(record))));
        return 1;
    }
    struct member *m = record_member(record->body, e->name);
    if (m == NULL) {
        return fail(c, e->at, fmt("no member named '%s' in '%s'", e->name, record_name(record)));
    }
    e->offset = m->offset;
    /* A member of a const struct is const too. */
    *out = ty_qualified(m->ty, quals);
    return 0;
}

/* The checks of an explicit conversion (C99 6.5.4): to void, or between
 * scalars, where clang reports a type that is no scalar, the target at the
 * cast's `(`, the operand where it begins. */
static int cast(struct fnc *c, struct expr *e) {
    struct pos pos = e->pos;
    struct expr *x = e->x;
    if (check_expr(c, x)) {
        return 1;
    }
    struct type *to = e->ty;
    struct type *from = ty_decay(x->ty);
    if (ty_is_void(to)) {
        return 0;
    }
    if (!ty_is_scalar(to)) {
        return fail(c, pos, fmt("used type '%s' where arithmetic or pointer type is required", ty_str(to)));
    }
    if (!ty_is_scalar(from)) {
        return fail(c, begin_written(c, x),
                    fmt("operand of type '%s' where arithmetic or pointer type is required", ty_str(from)));
    }
    struct type *tu = ty_unqual(to);
    /* A pointer to an integer narrower than it loses the address. */
    if (tu->kind == TY_INT && from->kind == TY_PTR) {
        int k = tu->ik;
        int64_t fs = ty_size(from);
        if (k != IK_BOOL && (int64_t)ik_size(k) < (fs < 0 ? 0 : fs)) {
            struct type *pt = ty_pointee(from);
            const char *flag = pt != NULL && ty_is_void(pt) ? "void-pointer-to-int-cast" : "pointer-to-int-cast";
            push(c, diag_warning(pos, fmt("cast to smaller integer type '%s' from '%s'", ty_str(to), ty_str(from)),
                                 flag));
        }
    }
    /* An integer narrower than a pointer made one, unless it is a constant,
     * a _Bool or an enum (clang's checkIntToPointerCast). */
    if (tu->kind == TY_PTR && from->kind == TY_INT) {
        struct spelling *spelled = spelled_of(c, x);
        int narrower = ik_size(from->ik) < PTR_SIZE;
        i128 v;
        int constant = consteval_int(x, &v);
        if (narrower && from->ik != IK_BOOL && !constant && enum_of(spelled, from) == NULL) {
            const char *flag = ty_is_void(tu->to) ? "int-to-void-pointer-cast" : "int-to-pointer-cast";
            push(c, diag_warning(pos,
                                 fmt("cast to '%s' from smaller integer type %s", ty_str(to), ty_named(from, spelled)),
                                 flag));
        }
    }
    return 0;
}

/* The checks of sizeof expr. */
static int sizeof_expr(struct fnc *c, struct expr *e) {
    struct expr *x = e->x;
    struct pos at = e->at;
    /* The operand is typed (and checked) but never evaluated, so a
     * file-scope compound literal in it is no object. */
    uint mark = c->literals.n;
    c->unevaluated++;
    /* The operand itself is not converted: sizeof a of an array is its
     * size, not a pointer's. */
    c->address_taken = 1;
    int failed = check_expr(c, x);
    c->unevaluated--;
    vec_truncate(&c->literals, mark);
    if (failed) {
        return 1;
    }
    /* sizeof a of a parameter declared int a[4] is a pointer's size. */
    if (x->kind == E_VAR) {
        struct binding *b = lookup(c, x->name);
        if (b != NULL) {
            struct type *array = posmap_get(&c->array_params, b->pos);
            if (array != NULL) {
                struct diag *w = diag_warning(loc_written(c, x),
                                              fmt("sizeof on array function parameter will return size of '%s' "
                                                  "instead of '%s'",
                                                  ty_str(b->ty), ty_str(array)),
                                              "sizeof-array-argument");
                push(c, diag_note(w, b->pos, "declared here"));
            }
        }
    }
    if (side_effects(x)) {
        struct pos here = posmap_has(&c->c->written.parens, x->pos) ? at : expr_loc(x);
        push(c, diag_warning(here, "expression with side effects has no effect in an unevaluated context",
                             "unevaluated-expression"));
    }
    return sizeof_value(c, e, x->ty, at);
}

/* The checks of `*x`, a[i] among them: the type in *out. */
static int deref(struct fnc *c, struct expr *e, int addressed, struct type **out) {
    struct pos pos = e->pos;
    struct expr *x = e->x;
    /* The parser writes a[i] as *(a + i), both at the `[`. */
    int subscript = x->kind == E_BINARY && x->op == OP_ADD && pos_eq(x->pos, pos);
    c->subscript = subscript;
    if (check_expr(c, x)) {
        return 1;
    }
    struct type *t = ty_decay(x->ty);
    if (t->kind != TY_PTR) {
        const char *msg = subscript ? "subscripted value is not an array, pointer, or vector"
                                    : fmt("indirection requires pointer operand ('%s' invalid)", ty_str(t));
        return fail(c, pos, msg);
    }
    struct type *to = t->to;
    /* *p of a void * is an expression of type void, which clang takes with a
     * warning. */
    if (ty_is_void(to) && !subscript && c->unevaluated == 0) {
        push(c, diag_warning(pos, fmt("ISO C does not allow indirection on operand of type '%s'", ty_str(x->ty)),
                             "void-ptr-dereference"));
        *out = to;
        return 0;
    }
    /* *(int *)0 read or stored: clang's warning that the optimizer deletes
     * it (CheckForNullPointerDereference). */
    if (!subscript && !addressed && ty_is_scalar(to) && !(ty_quals(to) & Q_VOLATILE)) {
        struct expr *inner = x;
        while (inner->kind == E_CAST) {
            inner = inner->x;
        }
        i128 v;
        if (ty_is_integer(inner->ty) && consteval_int(inner, &v) && i128_is_zero(v)) {
            struct diag *w =
                diag_warning(pos, "indirection of non-volatile null pointer will be deleted, not trap", "null-dereference");
            runtime(c, diag_note(w, pos, "consider using __builtin_trap() or qualifying pointer with 'volatile'"), e->id);
        }
    }
    *out = to;
    return 0;
}

/* The checks of an assignment: the type in *out. clang reports what is
 * wrong with it at its operator. */
static int assign(struct fnc *c, struct expr *e, struct type **out) {
    struct pos pos = e->pos;
    struct expr *target = e->x;
    struct expr *val = e->y;
    int a = check_expr(c, target);
    int b = check_expr(c, val);
    if (a || b) {
        return 1;
    }
    if (assignable(c, target, pos)) {
        return 1;
    }
    if (e->op == OP_NONE) {
        struct spelling *spelled = spelled_of(c, target);
        if (convert(c, val, target->ty, spelled, CONV_ASSIGN, pos)) {
            return 1;
        }
        coerce(val, target->ty);
    } else {
        /* t op= v computes t op v, then stores it as t = ... would: clang
         * checks that conversion, of the value v with the type of t op v. */
        struct type *t = ty_decay(target->ty);
        struct type *v = ty_decay(val->ty);
        struct type *r = arith_type(e->op, t, v);
        if (r == NULL) {
            return invalid_operands(c, e->op, pos, target->ty, val->ty);
        }
        /* A pointer difference is clang's __ptrdiff_t, as its messages
         * name it. */
        const char *named = ty_is_ptr(t) && ty_is_ptr(v) ? fmt("'__ptrdiff_t' (aka '%s')", ty_str(r))
                                                         : fmt("'%s'", ty_str(r));
        struct spelling *spelled = spelled_of(c, target);
        if (convert_value(c, val, r, named, NULL, 0, target->ty, spelled, CONV_ASSIGN, pos)) {
            return 1;
        }
        bad_shift_or_divisor(c, e->op, pos, e->id, t, val);
    }
    bounds(c, target);
    bounds(c, val);
    /* The value of an assignment is what was stored: no qualifiers. */
    *out = ty_unqual(target->ty);
    return 0;
}

/* Types e and everything in it (merlin's expr): nonzero if it reported an
 * error. */
static int check_expr(struct fnc *c, struct expr *e) {
    struct pos pos = e->pos;
    uint64_t id = e->id;
    /* Only e itself is the operand of &, not what is inside it. */
    int addressed = c->address_taken;
    c->address_taken = 0;
    if (!addressed) {
        int folded;
        if (folded_literal(c, e, &folded)) {
            return 1;
        }
        if (folded) {
            return 0;
        }
    }
    if (e->kind == E_VA_END || e->kind == E_VA_COPY) {
        return va_end_or_copy(c, e);
    }
    struct type *ty;
    switch (e->kind) {
    case E_NUM:
    case E_ENUMERATOR:
    case E_STR:
        return 0;
    case E_VAR: {
        struct binding *b = lookup(c, e->name);
        if (b != NULL) {
            /* A register array used as a value is its first element's
             * address, which C does not give (6.3.2.1p3). */
            if (b->var.kind == VAR_LOCAL && ty_is_array(b->ty) && intmap_has(&c->registers, b->var.local) &&
                !addressed) {
                return fail(c, pos, "address of register variable requested");
            }
            /* An invalid declaration's name: what it is in is invalid too,
             * of nothing more to say. */
            if (posmap_has(&c->invalid, b->pos)) {
                return 1;
            }
            e->var = b->var;
            ty = b->ty;
            break;
        }
        struct global *g = visible_global(c, e->name);
        if (g != NULL) {
            /* A function's name designates the function. */
            e->var.kind = VAR_GLOBAL;
            e->var.sym = e->name;
            ty = g->ty;
            break;
        }
        /* clang's typo correction: the name taken for one in scope, which
         * the expression then is. */
        struct correction *cr = posmap_get(&c->c->written.corrections, pos);
        if (cr == NULL) {
            return fail(c, pos, fmt("use of undeclared identifier '%s'", e->name));
        }
        struct diag *d = diag_error(pos, fmt("use of undeclared identifier '%s'; did you mean '%s'?", e->name, cr->name));
        push(c, diag_note(d, cr->pos, fmt("'%s' declared here", cr->name)));
        if (cr->has_enumerator) {
            e->kind = E_ENUMERATOR;
            e->name = cr->name;
            e->value = cr->value;
            e->spelled = cr->of;
        } else {
            e->kind = E_VAR;
            e->name = cr->name;
            e->var.kind = VAR_NONE;
        }
        e->ty = ty_int(IK_INT);
        return check_expr(c, e);
    }
    case E_ASSIGN:
        if (assign(c, e, &ty)) {
            return 1;
        }
        break;
    case E_INCDEC: {
        struct expr *target = e->x;
        if (check_expr(c, target)) {
            return 1;
        }
        struct type *u = ty_unqual(target->ty);
        if (u->kind == TY_PTR && ty_step(target->ty) < 0) {
            return incomplete_arithmetic(c, pos, u->to);
        }
        if (u->kind != TY_INT && u->kind != TY_PTR) {
            return fail(c, pos, fmt("cannot %s value of type '%s'", e->flag ? "increment" : "decrement",
                                    ty_str(target->ty)));
        }
        if (assignable(c, target, pos)) {
            return 1;
        }
        bounds(c, target);
        ty = ty_unqual(target->ty);
        break;
    }
    case E_BINARY: {
        /* A subscript's + is no operator to clang: its base and index are
         * not checked as operands are. */
        int subscript = c->subscript;
        c->subscript = 0;
        int a = check_expr(c, e->x);
        int b = check_expr(c, e->y);
        if (a || b) {
            return 1;
        }
        precedence(c, e->op, pos, e->x, e->y);
        c->subscript_add = subscript;
        int failed = binary(c, e->op, e->x, e->y, pos, id, &ty);
        c->subscript_add = 0;
        if (failed) {
            return 1;
        }
        if (!subscript) {
            bounds(c, e->x);
            bounds(c, e->y);
        }
        break;
    }
    case E_UNARY: {
        if (check_expr(c, e->x)) {
            return 1;
        }
        struct type *t = ty_decay(e->x->ty);
        if (e->op == UOP_NOT && ty_is_scalar(t)) {
            ty = ty_int(IK_INT);
        } else if (e->op != UOP_NOT && t->kind == TY_INT) {
            ty = ty_int(ik_promoted(t->ik));
        } else {
            return fail(c, pos, fmt("invalid argument type '%s' to unary expression", ty_str(t)));
        }
        bounds(c, e->x);
        break;
    }
    case E_ADDR:
        c->address_taken = 1;
        if (check_expr(c, e->x)) {
            return 1;
        }
        if (!is_lvalue(e->x)) {
            return fail(c, pos, fmt("cannot take the address of an rvalue of type '%s'", ty_str(e->x->ty)));
        }
        if (in_register(c, e->x)) {
            return fail(c, pos, "address of register variable requested");
        }
        ty = ty_ptr(e->x->ty);
        break;
    case E_DEREF:
        if (deref(c, e, addressed, &ty)) {
            return 1;
        }
        break;
    case E_CAST:
        return cast(c, e);
    case E_SIZEOF_TYPE:
        return sizeof_value(c, e, e->of, pos);
    case E_SIZEOF_EXPR:
        return sizeof_expr(c, e);
    case E_COND: {
        int r1 = cond_operand(c, e->x);
        int r2 = check_expr(c, e->y);
        int r3 = check_expr(c, e->z);
        if (r1 || r2 || r3) {
            return 1;
        }
        if (cond_type(c, e->y, e->z, pos, &ty)) {
            return 1;
        }
        break;
    }
    case E_COMMA: {
        int r1 = check_expr(c, e->x);
        int r2 = check_expr(c, e->y);
        if (r1 || r2) {
            return 1;
        }
        /* clang's DiagnoseCommaOperator: a left operand whose value is all
         * it does. */
        struct pos at;
        uint64_t of;
        int vol;
        if (unused_result(c, e->x, &at, &of, &vol)) {
            runtime(c, diag_warning(at, "left operand of comma operator has no effect", "unused-value"), of);
        }
        bounds(c, e->x);
        bounds(c, e->y);
        ty = ty_decay(e->y->ty);
        coerce(e->y, ty);
        break;
    }
    case E_MEMBER:
        if (member(c, e, addressed, &ty)) {
            return 1;
        }
        break;
    case E_CALL:
        if (call(c, e, &ty)) {
            return 1;
        }
        break;
    case E_VA_START:
        if (va_start(c, e->x, e->y, pos)) {
            return 1;
        }
        ty = TY_VOID_T;
        break;
    case E_VA_ARG:
        ty = e->ty;
        if (va_arg(c, e->x, ty, e->at, id)) {
            return 1;
        }
        break;
    default: {
        /* A compound literal. */
        ty = e->ty;
        /* Its list as written, before sema braces it fully, for clang's
         * messages that name it. */
        char *text = printed_init(c, e->init);
        if (text != NULL) {
            intmap_put(&c->literal_texts, e->id, text);
        }
        if (c->func[0] == 0) {
            struct init *init = e->init;
            e->init = init_list_new(pos);
            const char *name;
            if (static_literal(c, &ty, init, pos, &name)) {
                return 1;
            }
            e->kind = E_VAR;
            e->name = name;
            e->var.kind = VAR_GLOBAL;
            e->var.sym = name;
            e->init = NULL;
        } else {
            uint local;
            if (compound_literal(c, &ty, &e->init, pos, &local)) {
                return 1;
            }
            e->local = (int)local;
        }
        break;
    }
    }
    e->ty = ty;
    return 0;
}

/* Checks a compound literal's type: any complete object type, or an array
 * of unknown size (which the list completes). */
static int literal_type(struct fnc *c, struct type *ty, struct pos pos) {
    if (ty_is_func(ty) || ty_is_void(ty) || (ty_size(ty) < 0 && !(ty->kind == TY_ARRAY && !ty->has_len))) {
        incomplete_variable(c, pos, ty);
        return 1;
    }
    return 0;
}

/* (type){ ... } in a function: checks the list against the type
 * (completing an array's size) and gives the object a hidden local. */
static int compound_literal(struct fnc *c, struct type **ty, struct init **init, struct pos pos, uint *local) {
    if (literal_type(c, *ty, pos)) {
        return 1;
    }
    if (init_check(c, ty, init, 0, 1, pos)) {
        return 1;
    }
    *local = temp(c, *ty, "compound literal", pos);
    return 0;
}

/* (type){ ... } at file scope: an object with static storage, so its list
 * must be constant, like a global's. It becomes a static variable of its
 * own, whose key this gives. */
static int static_literal(struct fnc *c, struct type **ty, struct init *init, struct pos pos, const char **name) {
    if (literal_type(c, *ty, pos)) {
        return 1;
    }
    if (init_check(c, ty, &init, 1, 1, pos)) {
        return 1;
    }
    /* Numbered after any literal nested in this one's list. */
    const char *key = symbol_key(LITERAL, c->literal_base + c->literals.n);
    struct declarator *d = arena_alloc(sizeof *d);
    d->name = LITERAL;
    d->ty = *ty;
    d->storage = ST_STATIC;
    d->init = init;
    d->var.kind = VAR_GLOBAL;
    d->var.sym = key;
    d->define = 1;
    d->pos = pos;
    d->begin = pos;
    vec_push(&c->literals, d);
    *name = key;
    return 0;
}

/* The arguments a call's `...` takes get the default argument promotions:
 * integers narrower than int become int, arrays and functions pointers;
 * structs go as they are (a big one as the address of a copy, recorded in
 * copies). 0 if one is incomplete. */
static int variadic_args(struct fnc *c, struct expr **args, int *copies, uint n) {
    int ok = 1;
    for (uint i = 0; i < n; i++) {
        struct expr *a = args[i];
        struct type *t = ty_decay(a->ty);
        if (ty_is_void(t) || (ty_record(t) != NULL && ty_size(t) < 0)) {
            struct diag *e = diag_error(a->pos, fmt("argument type '%s' is incomplete", ty_str(t)));
            struct record *r = ty_record(t);
            if (r != NULL) {
                diag_note(e, r->pos, fmt("forward declaration of '%s'", record_name(r)));
            }
            push(c, e);
            ok = 0;
            continue;
        }
        struct type *promoted = t->kind == TY_INT ? ty_int(ik_promoted(t->ik)) : t;
        coerce(a, promoted);
        if (ty_record(promoted) != NULL && abi_classify(promoted) == PASS_REF) {
            copies[i] = (int)temp(c, promoted, "argument copy", a->pos);
        }
    }
    return ok;
}

/* Checks that ap, the first argument of a builtin, is a va_list (an
 * object, which the builtin changes, if target). */
static int va_list_arg(struct fnc *c, struct expr *ap, int target) {
    if (check_expr(c, ap)) {
        return 1;
    }
    if (!ty_eq(ty_decay(ap->ty), ty_ptr(TY_VOID_T))) {
        return fail(c, begin_written(c, ap),
                    fmt("non-const lvalue reference to type '__builtin_va_list' cannot bind to a value of unrelated "
                        "type '%s'",
                        ty_str(ap->ty)));
    }
    if (target && assignable(c, ap, ap->pos)) {
        return 1;
    }
    return 0;
}

/* va_start(ap, last): only in a variadic function, naming its last named
 * parameter (as C99 asks; merlin does not need it). */
static int va_start(struct fnc *c, struct expr *ap, struct expr *last, struct pos pos) {
    const char *param = c->last_param;
    if (param == NULL) {
        return fail(c, pos, "'va_start' used in function with fixed args");
    }
    if (va_list_arg(c, ap, 1)) {
        return 1;
    }
    if (check_expr(c, last)) {
        return 1;
    }
    /* merlin does not need it, and clang only warns. */
    if (!(last->kind == E_VAR && strcmp(last->name, param) == 0)) {
        push(c, diag_warning(begin_written(c, last), "second argument to 'va_start' is not the last non-variadic parameter",
                             "varargs"));
        return 0;
    }
    struct binding *b = lookup(c, param);
    if (b != NULL) {
        /* A parameter the default argument promotions change is not where
         * the caller put it. */
        int k = ty_int_kind(b->ty);
        if (k >= 0 && ik_promoted(k) != k) {
            struct diag *e = diag_warning(begin_written(c, last),
                                          "passing an object that undergoes default argument promotion to 'va_start' "
                                          "has undefined behavior",
                                          "varargs");
            push(c, diag_note(e, b->pos, fmt("parameter of type '%s' is declared here", ty_str(b->ty))));
        }
    }
    return 0;
}

/* va_arg(ap, t), with t written at `at`: t must be a complete object type.
 * One the default argument promotions change, or an array, makes it
 * undefined, which clang warns of (when it runs). */
static int va_arg(struct fnc *c, struct expr *ap, struct type *t, struct pos at, uint64_t id) {
    if (check_expr(c, ap)) {
        return 1;
    }
    if (!ty_eq(ty_decay(ap->ty), ty_ptr(TY_VOID_T))) {
        return fail(c, begin_written(c, ap),
                    fmt("first argument to 'va_arg' is of type '%s' and not 'va_list'", ty_str(ap->ty)));
    }
    if (ty_is_void(t) || ty_is_func(t) || ty_size(t) < 0) {
        struct diag *e = diag_error(at, fmt("second argument to 'va_arg' is of incomplete type '%s'", ty_str(t)));
        struct record *r = ty_record(t);
        if (r != NULL) {
            diag_note(e, r->pos, fmt("forward declaration of '%s'", record_name(r)));
        }
        push(c, e);
        return 1;
    }
    const char *undefined = NULL;
    int k = ty_int_kind(t);
    if (ty_is_array(t)) {
        undefined = fmt("second argument to 'va_arg' is of array type '%s'; this va_arg has undefined behavior "
                        "because arguments will never be compatible with array type",
                        ty_str(t));
    } else if (k >= 0 && ik_promoted(k) != k) {
        undefined = fmt("second argument to 'va_arg' is of promotable type '%s'; this va_arg has undefined "
                        "behavior because arguments will be promoted to 'int'",
                        ty_str(t));
    }
    if (undefined != NULL) {
        runtime(c, diag_warning(at, undefined, "varargs"), id);
    }
    return assignable(c, ap, ap->pos);
}

/* va_end(ap) does nothing: it becomes (void)ap. va_copy(dst, src) copies a
 * pointer: (void)(dst = src). */
static int va_end_or_copy(struct fnc *c, struct expr *e) {
    struct pos pos = e->pos;
    struct expr *x;
    if (e->kind == E_VA_END) {
        x = e->x;
        if (va_list_arg(c, x, 0)) {
            return 1;
        }
    } else {
        struct expr *dst = e->x;
        struct expr *src = e->y;
        int a = va_list_arg(c, dst, 1);
        int b = va_list_arg(c, src, 0);
        if (a || b) {
            return 1;
        }
        coerce(src, dst->ty);
        x = expr_new(E_ASSIGN, pos);
        x->op = OP_NONE;
        x->x = dst;
        x->y = src;
        x->ty = ty_unqual(dst->ty);
    }
    e->kind = E_CAST;
    e->implicit = 0;
    e->spelled = NULL;
    e->x = x;
    e->y = NULL;
    e->ty = TY_VOID_T;
    return 0;
}

/* What a sizeof sema made a constant was of, for clang's warnings of a
 * division of two of them (merlin's SizeofOf): the type, how it is
 * written, the variable it names (or NULL) and where it is declared, and
 * whether it is of an expression. */
struct sizeof_of {
    struct type *ty;
    const char *text;
    const char *var;
    struct pos var_at;
    int expr;
};

static char *printed(struct fnc *c, struct expr *e);
static int decl_of(struct fnc *c, const char *name, struct pos *at);

/* Replaces a sizeof with its value, a size_t. void and a function are 1
 * byte, a GNU extension clang takes without a word. An incomplete type is
 * reported at `at`. */
static int sizeof_value(struct fnc *c, struct expr *e, struct type *t, struct pos at) {
    /* What it is of, as written, before it becomes its value. */
    struct sizeof_of *of = NULL;
    if (e->kind == E_SIZEOF_EXPR) {
        struct expr *x = e->x;
        /* The operand is not converted: a variable is itself. */
        char *text = printed(c, x);
        if (text != NULL) {
            of = arena_alloc(sizeof *of);
            of->ty = x->ty;
            of->text = fmt("sizeof %s", text);
            of->expr = 1;
            struct pos vat;
            if (x->kind == E_VAR && decl_of(c, x->name, &vat)) {
                of->var = x->name;
                of->var_at = vat;
            }
        }
    } else {
        of = arena_alloc(sizeof *of);
        of->ty = t;
        of->text = fmt("sizeof(%s)", ty_declare(t, ""));
    }
    if (of != NULL) {
        intmap_put(&c->sizeofs, e->id, of);
    }
    int64_t size = ty_is_void(t) || ty_is_func(t) ? 1 : ty_size(t);
    if (size < 0) {
        struct diag *d = diag_error(at, fmt("invalid application of 'sizeof' to an incomplete type '%s'", ty_str(t)));
        struct record *r = ty_record(t);
        if (r != NULL) {
            diag_note(d, r->pos, fmt("forward declaration of '%s'", record_name(r)));
        }
        push(c, d);
        return 1;
    }
    e->kind = E_NUM;
    e->value = i128_of(size);
    e->x = NULL;
    e->ty = ty_int(ik_size_t());
    return 0;
}


static void logical_not_on_left(struct fnc *c, int op, struct expr *l, struct expr *r);
static void string_plus(struct fnc *c, struct pos pos, struct expr *l, struct expr *r);
static void sizeof_division(struct fnc *c, struct pos pos, struct expr *l, struct expr *r);
static void constant_logical_operand(struct fnc *c, int op, struct pos pos, struct type *lt, struct expr *r);
static void shift_of_constant(struct fnc *c, struct pos pos, uint64_t id, struct type *lt, struct expr *l,
                              struct expr *r);

static int is_str_literal(struct expr *e) {
    return strip_implicit(e)->kind == E_STR;
}

/* The type of l op r (merlin's binary), with clang's warnings of it. */
static int binary(struct fnc *c, int op, struct expr *l, struct expr *r, struct pos pos, uint64_t id,
                  struct type **out) {
    struct type *lt = ty_decay(l->ty);
    struct type *rt = ty_decay(r->ty);
    if (op == OP_LOGAND || op == OP_LOGOR) {
        if (!ty_is_scalar(lt) || !ty_is_scalar(rt)) {
            return invalid_operands(c, op, pos, l->ty, r->ty);
        }
        constant_logical_operand(c, op, pos, lt, r);
        *out = ty_int(IK_INT);
        return 0;
    }
    if (binop_is_comparison(op)) {
        /* clang's warnings come first, of what the operands are, then
         * whether they can be compared at all. */
        logical_not_on_left(c, op, l, r);
        /* Not of a macro's (#define SAME(x) (x == x)). */
        if (same_operand(l, r) && !posmap_has(&c->c->written.macros, pos)) {
            int always = op == OP_EQ || op == OP_LE || op == OP_GE;
            push(c, diag_warning(pos, fmt("self-comparison always evaluates to %s", always ? "true" : "false"),
                                 "tautological-compare"));
        } else if ((is_str_literal(l) && !is_null_pointer(r)) || (is_str_literal(r) && !is_null_pointer(l))) {
            /* A string literal compared with anything but a null pointer
             * constant compares its address. */
            push(c, diag_warning(pos,
                                 "result of comparison against a string literal is unspecified (use an explicit "
                                 "string comparison function instead)",
                                 "string-compare"));
        }
        int lnull = is_null_pointer(l);
        int rnull = is_null_pointer(r);
        int equality = op == OP_EQ || op == OP_NE;
        if (equality && (lnull || rnull)) {
            struct expr *other = rnull ? l : r;
            struct diag *w = never_null(c, other, op == OP_NE);
            if (w != NULL) {
                push(c, w);
            }
        }
        struct spelling *ls = spelled_of(c, l);
        struct spelling *rs = spelled_of(c, r);
        const char *pair = fmt("(%s and %s)", ty_named(lt, ls), ty_named(rt, rs));
        if (lt->kind == TY_INT && rt->kind == TY_INT) {
            /* Fine. */
        } else if (lt->kind == TY_PTR && rt->kind == TY_PTR) {
            /* Pointers to different types compare as addresses, with
             * clang's warning; void * and an object pointer, or any pointer
             * and a null pointer constant, without; void * and a function
             * pointer, for equality only. */
            struct type *au = ty_without_quals(lt->to);
            struct type *bu = ty_without_quals(rt->to);
            int is_void = equality && (ty_is_void(au) || ty_is_void(bu));
            int distinct = !ty_compatible(au, bu) || enums_differ(au, ls, bu, rs) || distinct_enums(au, ls, bu, rs);
            if (distinct && !is_void) {
                push(c, diag_warning(pos, fmt("comparison of distinct pointer types %s", pair),
                                     "compare-distinct-pointer-types"));
            }
        } else if ((lt->kind == TY_PTR && rt->kind == TY_INT) || (lt->kind == TY_INT && rt->kind == TY_PTR)) {
            /* A pointer and an integer that is no null pointer constant
             * compare as addresses too, with clang's warning. */
            if (!lnull && !rnull) {
                if (equality) {
                    push(c, diag_warning(pos, fmt("comparison between pointer and integer %s", pair),
                                         "pointer-integer-compare"));
                } else {
                    push(c, diag_warning_unflagged(pos, fmt("ordered comparison between pointer and integer %s", pair)));
                }
                /* The integer becomes a pointer, as clang converts it. */
                struct type *to = ty_is_ptr(lt) ? lt : rt;
                coerce(l, to);
                coerce(r, to);
            }
        } else {
            return invalid_operands(c, op, pos, l->ty, r->ty);
        }
        *out = ty_int(IK_INT);
        return 0;
    }
    /* Named as written, before arrays decay, as clang names them. */
    if (op == OP_BITAND) {
        logical_not_on_left(c, op, l, r);
    }
    if (op == OP_ADD && !c->subscript_add) {
        string_plus(c, pos, l, r);
    }
    if (op == OP_DIV) {
        sizeof_division(c, pos, l, r);
    }
    /* Pointers to two enums, which merlin's types leave out. */
    if (op == OP_SUB && lt->kind == TY_PTR && rt->kind == TY_PTR) {
        struct spelling *ls = spelled_of(c, l);
        struct spelling *rs = spelled_of(c, r);
        if (distinct_enums(lt->to, ls, rt->to, rs)) {
            return fail(c, pos, fmt("%s and %s are not pointers to compatible types", ty_named(lt, ls), ty_named(rt, rs)));
        }
    }
    struct type *ty = arith_type(op, lt, rt);
    if (ty == NULL) {
        return invalid_operands(c, op, pos, l->ty, r->ty);
    }
    bad_shift_or_divisor(c, op, pos, id, lt, r);
    if (op == OP_SHL) {
        shift_of_constant(c, pos, id, lt, l, r);
    }
    *out = ty;
    return 0;
}

/* An operator not in parentheses, or -1. */
static int bare_op(struct fnc *c, struct expr *e) {
    if (e->kind == E_BINARY && !posmap_has(&c->c->written.parens, e->pos)) {
        return e->op;
    }
    return -1;
}

static int is_bitwise(int o) {
    return o == OP_BITAND || o == OP_BITXOR || o == OP_BITOR;
}

static int is_relational(int o) {
    return o == OP_LT || o == OP_GT || o == OP_LE || o == OP_GE;
}

/* clang's -Wparentheses warnings of an operator op at pos whose operand,
 * written without parentheses, binds more tightly than it looks to: a
 * comparison in a bitwise operator, an addition in a shift
 * (DiagnoseBitwisePrecedence, DiagnoseAdditionInShift); and a chained
 * comparison, clang 23's error. */
static void precedence(struct fnc *c, int op, struct pos pos, struct expr *l, struct expr *r) {
    int lo = bare_op(c, l);
    int ro = bare_op(c, r);
    if (is_bitwise(op)) {
        int lc = lo >= 0 && binop_is_comparison(lo);
        int rc = ro >= 0 && binop_is_comparison(ro);
        if (lc != rc && !(lo >= 0 && is_bitwise(lo)) && !(ro >= 0 && is_bitwise(ro))) {
            const char *inner = binop_symbol(lc ? lo : ro);
            const char *outer = binop_symbol(op);
            struct diag *e = diag_warning(
                pos, fmt("%s has lower precedence than %s; %s will be evaluated first", outer, inner, inner),
                "parentheses");
            diag_note(e, pos, fmt("place parentheses around the '%s' expression to silence this warning", inner));
            diag_note(e, pos, fmt("place parentheses around the %s expression to evaluate it first", outer));
            push(c, e);
        }
    }
    /* a < b < c is (a < b) < c, which clang 23 makes an error. */
    if (is_relational(op) && lo >= 0 && is_relational(lo)) {
        struct diag *e = diag_error(pos, fmt("chained comparison 'X %s Y %s Z' does not behave the same as a "
                                             "mathematical expression",
                                             binop_symbol(lo), binop_symbol(op)));
        e->flag = "parentheses";
        push(c, e);
    }
    if (op == OP_SHL || op == OP_SHR) {
        struct expr *sides[2];
        int ops[2];
        sides[0] = l;
        sides[1] = r;
        ops[0] = lo;
        ops[1] = ro;
        for (uint i = 0; i < 2; i++) {
            if (ops[i] == OP_ADD || ops[i] == OP_SUB) {
                const char *shift = binop_symbol(op);
                const char *add = binop_symbol(ops[i]);
                struct diag *e = diag_warning(sides[i]->pos,
                                              fmt("operator '%s' has lower precedence than '%s'; '%s' will be "
                                                  "evaluated first",
                                                  shift, add, add),
                                              "shift-op-parentheses");
                push(c, diag_note(e, sides[i]->pos,
                                  fmt("place parentheses around the '%s' expression to silence this warning", add)));
            }
        }
    }
}

/* clang's -Wlogical-not-parentheses: !a == b (or !a & b) is (!a) == b,
 * likely meant as !(a == b). */
static void logical_not_on_left(struct fnc *c, int op, struct expr *l, struct expr *r) {
    struct expr *s = strip_implicit(l);
    if (!(s->kind == E_UNARY && s->op == UOP_NOT)) {
        return;
    }
    struct pos not = s->pos;
    if (posmap_has(&c->c->written.parens, not) || known_boolean(r) || known_boolean(strip_implicit(s->x))) {
        return;
    }
    const char *what = op == OP_BITAND ? "bitwise operator" : "comparison";
    struct diag *w = diag_warning(not, fmt("logical not is only applied to the left hand side of this %s", what),
                                  "logical-not-parentheses");
    diag_note(w, not, fmt("add parentheses after the '!' to evaluate the %s first", what));
    diag_note(w, not, "add parentheses around left hand side expression to silence this warning");
    push(c, w);
}

/* clang's warnings of sizeof x / sizeof y that does not count the elements
 * of an array (DiagnoseDivisionSizeofPointerOrArray). */
static void sizeof_division(struct fnc *c, struct pos pos, struct expr *l, struct expr *r) {
    struct sizeof_of *lo = intmap_get(&c->sizeofs, strip_implicit(l)->id);
    struct sizeof_of *ro = intmap_get(&c->sizeofs, strip_implicit(r)->id);
    if (lo == NULL || ro == NULL || !lo->expr) {
        return;
    }
    struct type *lt = ty_unqual(lo->ty);
    struct type *rt = ty_unqual(ro->ty);
    struct diag *w;
    if (lt->kind == TY_PTR && !ty_is_ptr(rt)) {
        if (!ty_eq(ty_unqual(lt->to), rt)) {
            return;
        }
        w = diag_warning(pos, fmt("'%s' will return the size of the pointer, not the array itself", lo->text),
                         "sizeof-pointer-div");
        if (lo->var != NULL) {
            diag_note(w, lo->var_at, fmt("pointer '%s' declared here", lo->var));
        }
    } else if (lt->kind == TY_ARRAY) {
        struct type *elem = ty_unqual(lt->to);
        int k = ty_int_kind(elem);
        int ch = k == IK_CHAR || k == IK_SCHAR || k == IK_UCHAR;
        if (ty_is_array(elem) || ch || ty_eq(elem, rt)) {
            return;
        }
        w = diag_warning(pos,
                         fmt("expression does not compute the number of elements in this array; element type is "
                             "'%s', not '%s'",
                             ty_str(elem), ty_str(rt)),
                         "sizeof-array-div");
        if (lo->var != NULL) {
            diag_note(w, lo->var_at, fmt("array '%s' declared here", lo->var));
        }
        diag_note(w, pos, fmt("place parentheses around the '%s' expression to silence this warning", ro->text));
    } else {
        return;
    }
    push(c, w);
}

/* Where the declaration of the variable name this checker sees begins. */
static int decl_begin_of(struct fnc *c, const char *name, struct pos *at) {
    struct binding *b = lookup(c, name);
    if (b != NULL) {
        struct pos *begin = posmap_get(&c->begins, b->pos);
        *at = begin != NULL ? *begin : b->pos;
        return 1;
    }
    struct global *g = visible_global(c, name);
    if (g == NULL) {
        return 0;
    }
    *at = g->is_func ? g->pos : g->begin;
    return 1;
}

static void stack_object(struct fnc *c, struct expr *e, struct vec *out);

/* The automatic objects whose addresses the value e can be, as clang's
 * lifetime analysis follows it: where each is named, and what it is (pairs
 * of struct pos * and char * in out). */
static void stack_address(struct fnc *c, struct expr *e, struct vec *out) {
    switch (e->kind) {
    case E_CAST:
        if (ty_is_array(e->x->ty)) {
            stack_object(c, e->x, out);
        } else {
            stack_address(c, e->x, out);
        }
        return;
    case E_COND:
        stack_address(c, e->y, out);
        stack_address(c, e->z, out);
        return;
    case E_COMMA:
        stack_address(c, e->y, out);
        return;
    case E_BINARY:
        if ((e->op == OP_ADD || e->op == OP_SUB) && ty_is_ptr(e->ty)) {
            stack_address(c, ty_is_ptr(ty_decay(e->x->ty)) ? e->x : e->y, out);
            return;
        }
        break;
    case E_ADDR:
        stack_object(c, e->x, out);
        return;
    default:
        break;
    }
    if (ty_is_array(e->ty)) {
        stack_object(c, e, out);
    }
}

/* stack_address of the object e designates. */
static void stack_object(struct fnc *c, struct expr *e, struct vec *out) {
    switch (e->kind) {
    case E_VAR:
        if (e->var.kind == VAR_LOCAL) {
            struct binding *b = lookup(c, e->name);
            const char *what = b != NULL && posmap_has(&c->param_pos, b->pos) ? "parameter" : "local variable";
            vec_push(out, pos_box(e->pos));
            vec_push(out, fmt("%s '%s'", what, e->name));
        }
        break;
    case E_MEMBER:
        if (!e->flag) {
            stack_object(c, e->x, out);
        }
        break;
    case E_DEREF:
        if (pos_eq(e->x->pos, e->pos)) {
            if (e->x->kind == E_BINARY) {
                struct expr *a = e->x->x;
                struct expr *i = e->x->y;
                struct expr *base = strip_implicit(ty_is_ptr(ty_decay(a->ty)) ? a : i);
                if (ty_is_array(base->ty)) {
                    stack_object(c, base, out);
                }
            }
        } else {
            stack_address(c, e->x, out);
        }
        break;
    case E_COMPOUND:
        if (e->local >= 0) {
            char *text = intmap_get(&c->literal_texts, e->id);
            if (text != NULL) {
                vec_push(out, pos_box(e->pos));
                vec_push(out, fmt("compound literal '%s'", text));
            }
        }
        break;
    case E_CAST:
        if (e->implicit) {
            stack_object(c, e->x, out);
        }
        break;
    default:
        break;
    }
}

/* An initializer list as clang prints it ({.x = 1, 2}), as far as printed
 * prints its expressions; NULL where it does not. */
static char *printed_init(struct fnc *c, struct init *i) {
    if (i->kind == I_EXPR) {
        return printed(c, i->e);
    }
    struct buf b;
    memset(&b, 0, sizeof b);
    if (i->kind == I_LIST) {
        buf_byte(&b, '{');
        for (uint k = 0; k < i->items.n; k++) {
            char *t = printed_init(c, i->items.v[k]);
            if (t == NULL) {
                buf_free(&b);
                return NULL;
            }
            if (k > 0) {
                buf_str(&b, ", ");
            }
            buf_str(&b, t);
        }
        buf_byte(&b, '}');
    } else {
        for (uint k = 0; k < i->nds; k++) {
            struct designator *d = i->ds[k];
            if (d->member) {
                buf_fmt(&b, ".%s", d->name);
            } else {
                char *t = printed(c, d->index);
                if (t == NULL) {
                    buf_free(&b);
                    return NULL;
                }
                buf_fmt(&b, "[%s]", t);
            }
        }
        char *t = printed_init(c, i->inner);
        if (t == NULL) {
            buf_free(&b);
            return NULL;
        }
        buf_fmt(&b, " = %s", t);
    }
    char *r = buf_arena(&b);
    buf_free(&b);
    return r;
}

/* Where the variable name this checker sees is declared. */
static int decl_of(struct fnc *c, const char *name, struct pos *at) {
    struct binding *b = lookup(c, name);
    if (b != NULL) {
        *at = b->pos;
        return 1;
    }
    struct global *g = visible_global(c, name);
    if (g == NULL) {
        return 0;
    }
    *at = g->pos;
    return 1;
}

/* e as clang prints an expression in a message, as far as merlin can:
 * names, members, *, &, subscripts, integer literals and the binary
 * operators, with the parentheses written; NULL for anything else. */
static char *printed(struct fnc *c, struct expr *e) {
    char *text;
    char *a;
    char *b;
    switch (e->kind) {
    case E_CAST:
        if (e->implicit) {
            return printed(c, e->x);
        }
        return NULL;
    case E_VAR:
        text = (char *)e->name;
        break;
    case E_NUM:
        if (!posmap_has(&c->c->written.int_literals, e->pos)) {
            return NULL;
        }
        text = i128_str(e->value);
        break;
    case E_MEMBER:
        a = printed(c, e->x);
        if (a == NULL) {
            return NULL;
        }
        text = fmt("%s%s%s", a, e->flag ? "->" : ".", e->name);
        break;
    case E_DEREF:
        if (pos_eq(e->x->pos, e->pos)) {
            if (!(e->x->kind == E_BINARY && e->x->op == OP_ADD)) {
                return NULL;
            }
            a = printed(c, e->x->x);
            b = a != NULL ? printed(c, e->x->y) : NULL;
            if (b == NULL) {
                return NULL;
            }
            text = fmt("%s[%s]", a, b);
        } else {
            a = printed(c, e->x);
            if (a == NULL) {
                return NULL;
            }
            text = fmt("*%s", a);
        }
        break;
    case E_ADDR:
        a = printed(c, e->x);
        if (a == NULL) {
            return NULL;
        }
        text = fmt("&%s", a);
        break;
    case E_BINARY:
        a = printed(c, e->x);
        b = a != NULL ? printed(c, e->y) : NULL;
        if (b == NULL) {
            return NULL;
        }
        text = fmt("%s %s %s", a, binop_symbol(e->op), b);
        break;
    default:
        return NULL;
    }
    return posmap_has(&c->c->written.parens, e->pos) ? fmt("(%s)", text) : text;
}

static int is_char_kind(int k) {
    return k == IK_CHAR || k == IK_SCHAR || k == IK_UCHAR;
}

/* A pointer to a character type. */
static int string_pointer(struct expr *e) {
    struct type *p = ty_pointee(ty_decay(e->ty));
    return p != NULL && is_char_kind(ty_int_kind(p));
}

/* clang's warnings of a string literal and an integer added: -Wstring-
 * plus-int of any integer, and -Wstring-plus-char of a character constant
 * and a pointer to a character type. */
static void string_plus(struct fnc *c, struct pos pos, struct expr *l, struct expr *r) {
    l = strip_implicit(l);
    r = strip_implicit(r);
    /* Not through parentheses: ("abc") + i is let be. */
    int ll = l->kind == E_STR && !posmap_has(&c->c->written.parens, l->pos);
    int rl = r->kind == E_STR && !posmap_has(&c->c->written.parens, r->pos);
    struct expr *index = ll ? r : rl ? l : NULL;
    if (index != NULL && ty_is_integer(index->ty)) {
        struct diag *w = diag_warning(pos, fmt("adding '%s' to a string does not append to the string", ty_str(index->ty)),
                                      "string-plus-int");
        push(c, diag_note(w, pos, "use array indexing to silence this warning"));
    }
    int lc = l->kind == E_NUM && posmap_has(&c->c->written.char_literals, l->pos);
    int rc = r->kind == E_NUM && posmap_has(&c->c->written.char_literals, r->pos);
    if ((rc && string_pointer(l)) || (lc && string_pointer(r))) {
        struct diag *w =
            diag_warning(pos, "adding 'char' to a string pointer does not append to the string", "string-plus-char");
        push(c, diag_note(w, pos, "use array indexing to silence this warning"));
    }
}

/* An i128 in upper-case hex, as Rust's {:X} writes a positive one. */
static char *hex_upper(i128 v) {
    static const char digits[] = "0123456789ABCDEF";
    char tmp[40];
    uint n = 0;
    do {
        tmp[n++] = digits[v.lo & 15];
        v = i128_shr(v, 4);
    } while (!i128_is_zero(v));
    char *r = arena_alloc(n + 1);
    for (uint i = 0; i < n; i++) {
        r[i] = tmp[n - 1 - i];
    }
    return r;
}

/* clang's warnings of l << r of constants (DiagnoseBadShiftValues, the
 * count in range): of a negative left operand, when it runs; of a signed
 * result that needs more bits than its type has, but for its sign bit. */
static void shift_of_constant(struct fnc *c, struct pos pos, uint64_t of, struct type *lt, struct expr *l,
                              struct expr *r) {
    int k = ty_int_kind(lt);
    if (k < 0) {
        return;
    }
    k = ik_promoted(k);
    if (!ik_signed(k)) {
        return;
    }
    i128 width = i128_of(ik_size(k) * 8);
    i128 a;
    i128 n;
    if (!consteval_int(l, &a) || !consteval_int(r, &n)) {
        return;
    }
    if (i128_neg_p(n) || i128_le(width, n)) {
        return;
    }
    if (i128_neg_p(a)) {
        runtime(c, diag_warning(pos, "shifting a negative signed value is undefined", "shift-negative-value"), of);
        return;
    }
    /* The bits of the value, its sign bit among them, and of the result. */
    i128 bits = i128_of(128 - (int64_t)i128_leading_zeros(a) + 1);
    i128 needed = i128_add(n, bits);
    if (i128_le(needed, width) || i128_eq(i128_sub(needed, i128_of(1)), width)) {
        return;
    }
    i128 result = i128_shl(a, (uint)n.lo);
    push(c, diag_warning(pos,
                         fmt("signed shift result (0x%s) requires %s bits to represent, but '%s' only has %s bits",
                             hex_upper(result), i128_str(needed), ty_str(ty_int(k)), i128_str(width)),
                         "shift-overflow"));
}

/* clang's -Wconstant-logical-operand: x && 4, an integer and a constant
 * other than 0 and 1, was likely meant as x & 4. */
static void constant_logical_operand(struct fnc *c, int op, struct pos pos, struct type *lt, struct expr *r) {
    int lk = ty_int_kind(lt);
    int rk = ty_int_kind(ty_decay(r->ty));
    if (lk < 0 || lk == IK_BOOL || rk < 0 || rk == IK_BOOL) {
        return;
    }
    i128 v;
    if (!consteval_int(r, &v) || i128_is_zero(v) || i128_eq(v, i128_of(1))) {
        return;
    }
    /* Not of an enumerator, nor of a value its evaluator does not give. */
    struct expr *inner = r;
    while (inner->kind == E_CAST && inner->implicit) {
        inner = inner->x;
    }
    if (inner->kind == E_ENUMERATOR || overflows(inner)) {
        return;
    }
    const char *logical = op == OP_LOGAND ? "&&" : "||";
    const char *bitwise = op == OP_LOGAND ? "&" : "|";
    struct diag *e = diag_warning(pos, fmt("use of logical '%s' with constant operand", logical), "constant-logical-operand");
    diag_note(e, pos, fmt("use '%s' for a bitwise operation", bitwise));
    if (op == OP_LOGAND) {
        diag_note(e, pos, "remove constant to silence this warning");
    }
    push(c, e);
}

/* clang's warnings of a shift by a constant count that is negative or not
 * less than the width of the left operand's promoted type, and of dividing
 * by a constant zero: what the code does when it runs. */
static void bad_shift_or_divisor(struct fnc *c, int op, struct pos pos, uint64_t of, struct type *lt, struct expr *r) {
    i128 v;
    if (!ty_is_integer(r->ty) || !consteval_int(r, &v)) {
        return;
    }
    const char *msg = NULL;
    const char *flag = NULL;
    if (op == OP_SHL || op == OP_SHR) {
        int k = ty_int_kind(lt);
        i128 bits = i128_of(k < 0 ? 0 : ik_size(ik_promoted(k)) * 8);
        if (i128_neg_p(v)) {
            msg = "shift count is negative";
            flag = "shift-count-negative";
        } else if (i128_le(bits, v)) {
            msg = "shift count >= width of type";
            flag = "shift-count-overflow";
        }
    } else if (op == OP_DIV && i128_is_zero(v)) {
        msg = "division by zero is undefined";
        flag = "division-by-zero";
    } else if (op == OP_REM && i128_is_zero(v)) {
        msg = "remainder by zero is undefined";
        flag = "division-by-zero";
    }
    if (msg != NULL) {
        runtime(c, diag_warning(pos, msg, flag), of);
    }
}

/* overflow of the operator e itself, at it. */
static void overflowed(struct fnc *c, struct expr *e) {
    i128 v;
    int k;
    if (consteval_overflow_at(e, 1, 0, &v, &k)) {
        push(c, diag_warning(e->pos,
                             fmt("overflow in expression; result is %s with type '%s'", digit_groups(v),
                                 ty_str(ty_int(k))),
                             "integer-overflow"));
    }
}

/* clang's -Winteger-overflow in the full expression e, which it evaluates
 * once it is built (EvaluateForOverflow), as it runs: the operands a
 * constant ?:, && or || passes by are not. */
static void overflow(struct fnc *c, struct expr *e) {
    if (c->unevaluated > 0) {
        return;
    }
    i128 v;
    switch (e->kind) {
    case E_COND:
        overflow(c, e->x);
        if (consteval_int(e->x, &v)) {
            overflow(c, i128_is_zero(v) ? e->z : e->y);
        } else {
            overflow(c, e->y);
            overflow(c, e->z);
        }
        return;
    case E_BINARY:
        if (e->op == OP_LOGAND || e->op == OP_LOGOR) {
            overflow(c, e->x);
            int passed = consteval_int(e->x, &v) && (!i128_is_zero(v)) == (e->op == OP_LOGOR);
            if (!passed) {
                overflow(c, e->y);
            }
            return;
        }
        overflow(c, e->x);
        overflow(c, e->y);
        overflowed(c, e);
        return;
    case E_UNARY:
        if (e->op == UOP_NEG) {
            overflow(c, e->x);
            overflowed(c, e);
            return;
        }
        break;
    case E_SIZEOF_EXPR:
    case E_SIZEOF_TYPE:
        return;
    default:
        break;
    }
    struct vec k = child_vec(e);
    for (uint i = 0; i < k.n; i++) {
        overflow(c, k.v[i]);
    }
    heap_free(k.v);
}

/* What clang checks of a full expression once it is built: its conversions
 * and comparisons, then its overflows. */
static void full_expression(struct fnc *c, struct expr *e) {
    full(c, e);
    /* An item of a list is no full expression of its own. */
    if (c->unevaluated == 0 && c->list_depth == 0) {
        struct finding *f;
        uint n = sequence_check(e, &f);
        for (uint i = 0; i < n; i++) {
            runtime(c, diag_warning(f[i].at, f[i].msg, "unsequenced"), f[i].of);
        }
    }
    overflow(c, e);
}

static void bound(struct fnc *c, struct expr *base, struct expr *index, int one_past);

static void bounds_chain(struct fnc *c, struct expr *e, int allow) {
    e = strip_implicit(e);
    for (;;) {
        if (e->kind == E_DEREF && pos_eq(e->x->pos, e->pos)) {
            if (e->x->kind != E_BINARY) {
                return;
            }
            struct expr *a = e->x->x;
            struct expr *i = e->x->y;
            int ap = ty_is_ptr(ty_decay(a->ty));
            struct expr *base = ap ? a : i;
            struct expr *index = ap ? i : a;
            bound(c, base, index, allow > 0);
            e = strip_implicit(base);
        } else if (e->kind == E_MEMBER) {
            e = strip_implicit(e->x);
        } else if (e->kind == E_ADDR) {
            allow++;
            e = strip_implicit(e->x);
        } else if (e->kind == E_DEREF) {
            allow--;
            e = strip_implicit(e->x);
        } else if (e->kind == E_COND) {
            bounds_chain(c, e->y, allow);
            bounds_chain(c, e->z, allow);
            return;
        } else {
            return;
        }
    }
}

/* clang's -Warray-bounds of e, as its CheckArrayAccess checks it where it
 * is called (merlin's bounds, whose comment says where). */
static void bounds(struct fnc *c, struct expr *e) {
    bounds_chain(c, e, 0);
}

/* The subscript of base by index: clang's warning if base is an array of
 * known size and index a constant out of it (one just past the end may be
 * if one_past), with where the array is declared. */
static void bound(struct fnc *c, struct expr *base, struct expr *index, int one_past) {
    struct expr *array = strip_implicit(base);
    struct type *at = ty_unqual(array->ty);
    if (!(at->kind == TY_ARRAY && at->has_len)) {
        return;
    }
    i128 n = i128_of(at->len);
    i128 v;
    if (!consteval_int(index, &v)) {
        return;
    }
    const char *msg;
    if (i128_neg_p(v)) {
        msg = fmt("array index %s is before the beginning of the array", i128_str(v));
    } else if (i128_lt(n, v) || (i128_eq(v, n) && !one_past)) {
        msg = fmt("array index %s is past the end of the array (that has type '%s')", i128_str(v), ty_str(at));
    } else {
        return;
    }
    /* Where the base begins, inside parentheses around it. */
    struct diag *w = diag_warning(begin_inside(c, array), msg, "array-bounds");
    /* The array it is, through the subscripts it is of. */
    struct expr *named = array;
    while (named->kind == E_DEREF) {
        struct expr *x = named->x;
        if (x->kind != E_BINARY || !pos_eq(x->pos, named->pos)) {
            break;
        }
        named = strip_implicit(ty_is_ptr(ty_decay(x->x->ty)) ? x->x : x->y);
    }
    struct pos decl;
    if (named->kind == E_VAR && decl_begin_of(c, named->name, &decl)) {
        diag_note(w, decl, fmt("array '%s' declared here", named->name));
    } else if (named->kind == E_MEMBER) {
        struct member *m = member_of(named);
        if (m != NULL) {
            diag_note(w, m->begin, fmt("array '%s' declared here", named->name));
        }
    }
    runtime(c, w, index->id);
}

/* clang's -Wtautological-constant-out-of-range-compare: a constant compared
 * with an expression of a type whose every value is on one side of it, so
 * the result is always the same. */
static void out_of_range(struct fnc *c, int op, struct pos pos, uint64_t of, struct expr *l, struct expr *r) {
    int lk = ty_int_kind(ty_decay(l->ty));
    int rk = ty_int_kind(ty_decay(r->ty));
    if (lk < 0 || rk < 0) {
        return;
    }
    int common = ik_common(lk, rk);
    for (int side = 0; side < 2; side++) {
        struct expr *cst = side == 0 ? l : r;
        struct expr *other = side == 0 ? r : l;
        int left = side == 0;
        i128 v;
        i128 w;
        if (!consteval_int(cst, &v) || consteval_int(other, &w)) {
            continue;
        }
        struct expr *inner = other;
        while (inner->kind == E_CAST && inner->implicit) {
            inner = inner->x;
        }
        int t = ty_int_kind(ty_unqual(inner->ty));
        if (t < 0) {
            continue;
        }
        /* Only where every value of t is the same in the common type. */
        if (i128_lt(ik_min(t), ik_min(common)) || i128_lt(ik_max(common), ik_max(t))) {
            continue;
        }
        v = ik_wrap(common, v);
        int below = i128_lt(v, ik_min(t));
        if (!below && i128_le(v, ik_max(t))) {
            /* A _Bool compared with one of its own two values, where that
             * decides it: clang's warning of the limits of a truth value. */
            if (t != IK_BOOL) {
                continue;
            }
            /* As other op v. */
            int o = op;
            if (left) {
                o = op == OP_LT ? OP_GT : op == OP_GT ? OP_LT : op == OP_LE ? OP_GE : op == OP_GE ? OP_LE : op;
            }
            int one = i128_eq(v, i128_of(1));
            int zero = i128_is_zero(v);
            int always;
            if ((o == OP_GT && one) || (o == OP_LT && zero)) {
                always = 0;
            } else if ((o == OP_LE && one) || (o == OP_GE && zero)) {
                always = 1;
            } else {
                continue;
            }
            runtime(c,
                    diag_warning(pos,
                                 fmt("result of comparison of constant %s with expression of type '%s' is always %s",
                                     i128_str(v), ty_str(ty_unqual(inner->ty)), always ? "true" : "false"),
                                 "tautological-constant-compare"),
                    of);
            continue;
        }
        /* v op other, or other op v, for every value of t. */
        int less = below == left;
        int always;
        if (op == OP_LT || op == OP_LE) {
            always = less;
        } else if (op == OP_GT || op == OP_GE) {
            always = !less;
        } else {
            always = op != OP_EQ;
        }
        runtime(c,
                diag_warning(pos,
                             fmt("result of comparison of constant %s with expression of type '%s' is always %s",
                                 i128_str(v), ty_str(ty_unqual(inner->ty)), always ? "true" : "false"),
                             "tautological-constant-out-of-range-compare"),
                of);
    }
}

/* clang's -Wconstant-conversion of the constant e converted to the integer
 * type to, if the value changes: of a value too wide for it, when the code
 * runs; of one exactly as wide that becomes negative, at once. */
static void constant_conversion(struct fnc *c, struct expr *e, struct type *to) {
    int from = ty_int_kind(ty_decay(e->ty));
    int k = ty_int_kind(ty_unqual(to));
    if (from < 0 || k < 0) {
        return;
    }
    i128 v;
    if (!consteval_int(e, &v) || k == IK_BOOL) {
        /* Each operand a ?: may give, the right one of a comma. */
        if (e->kind == E_COND) {
            constant_conversion(c, e->y, to);
            constant_conversion(c, e->z, to);
        } else if (e->kind == E_COMMA) {
            constant_conversion(c, e->y, to);
        } else if (e->kind == E_CAST && e->implicit) {
            constant_conversion(c, e->x, to);
        }
        return;
    }
    /* The bits the value needs (a sign bit with them for a negative one),
     * and the type's (clang's IntRange). */
    uint width = !i128_neg_p(v) ? 128 - i128_leading_zeros(v) : 129 - i128_leading_zeros(i128_not(v));
    uint bits = ik_size(k) * 8;
    i128 changed = ik_wrap(k, v);
    struct diag *warning =
        diag_warning(expr_loc(e),
                     fmt("implicit conversion from '%s' to '%s' changes value from %s to %s", ty_str(ty_decay(e->ty)),
                         ty_str(ty_unqual(to)), i128_str(v), i128_str(changed)),
                     "constant-conversion");
    if (width > bits) {
        runtime(c, warning, e->id);
    } else if (width == bits && ik_signed(k) && !i128_neg_p(v) && ik_signed(from) &&
               !posmap_has(&c->c->written.bit_patterns, begin_of(e))) {
        /* Not of a literal written in hex or octal, a bit pattern, as
         * clang's isSameWidthConstantConversion has it. */
        push(c, warning);
    }
}

/* An integer type as an operand promoted has it; any other type as it is. */
static struct type *promoted(struct type *t) {
    struct type *u = ty_unqual(t);
    return u->kind == TY_INT ? ty_int(ik_promoted(u->ik)) : t;
}

/* The type of c ? a : b (C99 6.5.15) at pos, converting both branches to
 * it. What C99 has no type for, clang gives one, with a warning: pointers
 * to different types make a void *, a pointer and an integer the pointer;
 * void and anything, void. */
static int cond_type(struct fnc *c, struct expr *a, struct expr *b, struct pos pos, struct type **out) {
    struct type *at = ty_decay(a->ty);
    struct type *bt = ty_decay(b->ty);
    struct spelling *sa = spelled_of(c, a);
    struct spelling *sb = spelled_of(c, b);
    struct type *ty;
    if (at->kind == TY_INT && bt->kind == TY_INT) {
        ty = ty_int(ik_common(at->ik, bt->ik));
    } else if (at->kind == TY_VOID || bt->kind == TY_VOID) {
        ty = TY_VOID_T;
    } else if (at->kind == TY_RECORD && bt->kind == TY_RECORD && at->rec == bt->rec) {
        ty = at;
    } else if (at->kind == TY_PTR && bt->kind == TY_PTR && is_null_pointer(a)) {
        /* A null pointer constant takes the other pointer's type, even when
         * it is (void *)0 (C99 6.5.15p6). */
        ty = bt;
    } else if (at->kind == TY_PTR && bt->kind == TY_PTR && is_null_pointer(b)) {
        ty = at;
    } else if (at->kind == TY_PTR && bt->kind == TY_PTR) {
        /* Otherwise both point to the same type, but for the qualifiers of
         * what they point to, or one to void and the other to an object,
         * and the result points to it with every qualifier of either side.
         * Of others clang warns, and makes the result a void *. */
        struct type *x = at->to;
        struct type *y = bt->to;
        uint q = ty_quals(x) | ty_quals(y);
        struct type *xu = ty_unqual(x);
        struct type *yu = ty_unqual(y);
        if (ty_compatible(xu, yu) && !distinct_enums(xu, sa, yu, sb)) {
            ty = ty_ptr(ty_qualified(ty_composite(xu, yu), q));
        } else if ((ty_is_void(x) && !ty_is_func(y)) || (ty_is_void(y) && !ty_is_func(x))) {
            ty = ty_ptr(ty_qualified(TY_VOID_T, q));
        } else {
            push(c, diag_warning(pos, fmt("pointer type mismatch (%s and %s)", ty_named(at, sa), ty_named(bt, sb)),
                                 "pointer-type-mismatch"));
            ty = ty_ptr(TY_VOID_T);
        }
    } else if ((at->kind == TY_PTR && bt->kind == TY_INT) || (at->kind == TY_INT && bt->kind == TY_PTR)) {
        /* Only an integer that is a null pointer constant takes the
         * pointer's type silently. */
        struct expr *integer = ty_is_ptr(at) ? b : a;
        if (!is_null_pointer(integer)) {
            /* The operands as clang names them: promoted. */
            push(c, diag_warning(pos,
                                 fmt("pointer/integer type mismatch in conditional expression ('%s' and '%s')",
                                     ty_str(promoted(at)), ty_str(promoted(bt))),
                                 "conditional-type-mismatch"));
        }
        ty = ty_is_ptr(at) ? at : bt;
    } else {
        return fail(c, pos, fmt("incompatible operand types ('%s' and '%s')", ty_str(promoted(at)), ty_str(promoted(bt))));
    }
    coerce(a, ty);
    coerce(b, ty);
    *out = ty;
    return 0;
}

/* e without the conversions sema added. */
static struct expr *strip_implicit(struct expr *e) {
    while (e->kind == E_CAST && e->implicit) {
        e = e->x;
    }
    return e;
}

/* The bases of two members same_operand compares: the same variable, or
 * the same member of one. */
static int member_base(struct expr *a, struct expr *b) {
    if (a->kind == E_VAR && b->kind == E_VAR) {
        return a->var.kind != VAR_NONE && b->var.kind != VAR_NONE && var_eq(a->var, b->var);
    }
    if (a->kind == E_MEMBER && b->kind == E_MEMBER) {
        return strcmp(a->name, b->name) == 0 && a->flag == b->flag && member_base(a->x, b->x);
    }
    return 0;
}

/* Whether two operands of a comparison are the same thing, as clang's
 * isSameComparisonOperand sees it (merlin's same_operand, which flow.c
 * uses too). */
int same_operand(struct expr *a, struct expr *b) {
    if (a->kind == E_VAR && b->kind == E_VAR) {
        return a->var.kind != VAR_NONE && b->var.kind != VAR_NONE && var_eq(a->var, b->var);
    }
    if (a->kind == E_MEMBER && b->kind == E_MEMBER) {
        return strcmp(a->name, b->name) == 0 && a->flag == b->flag && member_base(a->x, b->x);
    }
    /* a[i] is *(a + i), at the `[`. */
    if (a->kind == E_DEREF && b->kind == E_DEREF && pos_eq(a->x->pos, a->pos) && pos_eq(b->x->pos, b->pos)) {
        struct expr *x = a->x;
        struct expr *y = b->x;
        if (x->kind == E_BINARY && x->op == OP_ADD && y->kind == E_BINARY && y->op == OP_ADD) {
            /* An index may be the same number too. */
            int index;
            if (x->y->kind == E_NUM && y->y->kind == E_NUM) {
                index = i128_eq(x->y->value, y->y->value);
            } else {
                index = same_operand(x->y, y->y);
            }
            return same_operand(x->x, y->x) && index;
        }
    }
    return 0;
}

/* clang's warning for a pointer e that can never be null, used as a truth
 * value (compared -1) or compared with a null pointer constant (compared:
 * whether the comparison is !=): the address of an object, an array or a
 * function named (DiagnoseAlwaysNonNullPointer); at the name. */
static struct diag *never_null(struct fnc *c, struct expr *e, int compared) {
    const char *name;
    struct pos pos;
    const char *what;
    int function = 0;
    if (e->kind == E_ADDR && e->x->kind == E_VAR) {
        name = e->x->name;
        pos = e->x->pos;
        what = "address of";
    } else if (e->kind == E_VAR && ty_is_func(e->ty)) {
        name = e->name;
        pos = e->pos;
        what = "function";
        function = 1;
    } else if (e->kind == E_VAR && ty_is_array(e->ty)) {
        name = e->name;
        pos = e->pos;
        what = "array";
    } else {
        return NULL;
    }
    const char *named;
    if (strcmp(what, "address of") == 0) {
        named = fmt("address of '%s'", name);
    } else if (compared < 0) {
        named = fmt("address of %s '%s'", what, name);
    } else {
        named = fmt("%s '%s'", what, name);
    }
    struct diag *d;
    if (compared < 0) {
        d = diag_warning(pos, fmt("%s will always evaluate to 'true'", named), "pointer-bool-conversion");
    } else {
        d = diag_warning(pos,
                         fmt("comparison of %s %s to a null pointer is always %s", named, compared ? "not equal" : "equal",
                             compared ? "true" : "false"),
                         "tautological-pointer-compare");
    }
    if (function) {
        diag_note(d, pos, "prefix with the address-of operator to silence this warning");
        if (compared >= 0) {
            diag_note(d, pos, "suffix with parentheses to turn this into a function call");
        }
    }
    return d;
}

/* ---- What the parser evaluated away ---- */

/* The expressions the parser evaluated away at one place, in order
 * (merlin's FnChecker::unevaluated): each checked as the operand of sizeof
 * is, never evaluated, with the parameters before it in scope if it was in
 * a parameter list. One the parser could not type made its constant an
 * error, reported here if the expression has none of its own. A
 * declarator's warning is given if none of its operands has an error. */
static void unevaluated_list(struct fnc *c, struct vec *us) {
    uchar *clean = heap_alloc(us->n + 1);
    uint nclean = 0;
    for (uint i = 0; i < us->n; i++) {
        struct unevaluated *u = us->v[i];
        uint literals = c->literals.n;
        uint locals = c->locals.n;
        int params = u->nparams > 0;
        if (params) {
            /* C's prototype scope: what the parameter list declared. */
            push_scope(c);
            for (uint k = 0; k < u->nparams; k++) {
                declare(c, u->params[k].name, u->params[k].ty, u->params[k].pos, "parameter ");
            }
        }
        c->unevaluated++;
        /* A sizeof's operand is not converted, as in sizeof itself. */
        c->address_taken = !u->constant && !u->folded && !u->has_untyped;
        int failed = check_expr(c, u->expr);
        c->unevaluated--;
        if (u->constant && !failed) {
            /* An integer constant expression is evaluated, as clang
             * evaluates it: its overflows are warned of. */
            uint outer = c->unevaluated;
            c->unevaluated = 0;
            overflow(c, u->expr);
            c->unevaluated = outer;
        }
        if (params) {
            pop_scope(c);
        }
        vec_truncate(&c->literals, literals);
        vec_truncate(&c->locals, locals);
        int ok = !failed && !u->has_untyped;
        if (!failed && u->has_untyped) {
            struct type *t = ty_decay(u->expr->ty);
            if (u->has_non_integer && !ty_is_integer(t)) {
                fail(c, u->non_integer_pos, fmt("%s '%s'", u->non_integer_msg, ty_str(t)));
            } else {
                struct diag *e = diag_error(u->untyped_pos, u->untyped_msg);
                struct pos at;
                const char *note;
                if (consteval_why_not(u->expr, &at, &note)) {
                    diag_note(e, at, note);
                }
                push(c, e);
            }
        }
        clean[nclean++] = (uchar)ok;
        if (u->diagnostic != NULL) {
            uchar *own = clean + nclean - 1 - u->with;
            uint nown = u->with + 1;
            int all = 1;
            for (uint k = 0; k < nown; k++) {
                all &= own[k];
            }
            if (all) {
                push(c, u->diagnostic);
            } else if (u->vla != NULL) {
                /* A size with an error clang takes as a constant, and folds
                 * none of the others: those are of variable length, which
                 * file scope has not. */
                struct file_vla *v = u->vla;
                int top = 0;
                if (v->has_top) {
                    top = 1;
                    for (uint k = v->top.start; k < v->top.end; k++) {
                        top &= own[k];
                    }
                }
                int inner = 0;
                for (uint r = 0; r < v->ninner && !inner; r++) {
                    int all_ok = 1;
                    for (uint k = v->inner[r].start; k < v->inner[r].end; k++) {
                        all_ok &= own[k];
                    }
                    inner = all_ok;
                }
                if (top) {
                    fail(c, v->at, "variable length array declaration not allowed at file scope");
                } else if (inner) {
                    fail(c, v->at, "variably modified type declaration not allowed at file scope");
                }
            }
        }
    }
    heap_free(clean);
}
