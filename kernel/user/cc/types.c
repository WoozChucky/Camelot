/* cc1: C types as merlin knows them, with their sizes on Excalibur, RISC-V's
 * lp64 (follows merlin's types.rs, whose comments say more). char is 1
 * byte, short 2, int 4, and long, long long and pointers 8, each aligned to
 * its size. Plain char is unsigned, as on every RISC-V target.
 *
 * Integer promotion turns _Bool, char and short into int; the usual
 * arithmetic conversions bring two operands to a common type by rank and
 * range (ik_common). Structs and unions are laid out as clang lays them
 * out: each member at the next offset aligned for its type, the whole
 * aligned to its most aligned member. An enum type is an integer type
 * (unsigned int, or int when a constant is negative).
 *
 * const and volatile wrap a type in TY_QUAL. Qualifiers of an array apply
 * to its elements, so a TY_QUAL never wraps an array, nor another TY_QUAL:
 * ty_qualified keeps that form. */
#include "cc.h"

/* ---- Integer types ---- */

uint ik_size(int k) {
    switch (k) {
    case IK_BOOL:
    case IK_CHAR:
    case IK_SCHAR:
    case IK_UCHAR:
        return 1;
    case IK_SHORT:
    case IK_USHORT:
        return 2;
    case IK_INT:
    case IK_UINT:
        return 4;
    default:
        return 8;
    }
}

/* C's integer conversion rank (C99 6.3.1.1). */
static uint ik_rank(int k) {
    switch (k) {
    case IK_BOOL:
        return 0;
    case IK_CHAR:
    case IK_SCHAR:
    case IK_UCHAR:
        return 1;
    case IK_SHORT:
    case IK_USHORT:
        return 2;
    case IK_INT:
    case IK_UINT:
        return 3;
    case IK_LONG:
    case IK_ULONG:
        return 4;
    default:
        return 5;
    }
}

/* size_t, what sizeof gives: unsigned long (clang's __SIZE_TYPE__). */
int ik_size_t(void) {
    return IK_ULONG;
}

/* ptrdiff_t, the difference of two pointers: long. */
int ik_ptrdiff_t(void) {
    return IK_LONG;
}

static int ik_to_unsigned(int k) {
    switch (k) {
    case IK_INT:
        return IK_UINT;
    case IK_LONG:
        return IK_ULONG;
    case IK_LLONG:
        return IK_ULLONG;
    default:
        return k;
    }
}

int ik_signed(int k) {
    return k == IK_SCHAR || k == IK_SHORT || k == IK_INT || k == IK_LONG || k == IK_LLONG;
}

const char *ik_name(int k) {
    static const char *const names[] = {"_Bool",
                                        "char",
                                        "signed char",
                                        "unsigned char",
                                        "short",
                                        "unsigned short",
                                        "int",
                                        "unsigned int",
                                        "long",
                                        "unsigned long",
                                        "long long",
                                        "unsigned long long"};
    return names[k];
}

/* Anything of lower rank than int becomes int. */
int ik_promoted(int k) {
    return ik_rank(k) < ik_rank(IK_INT) ? IK_INT : k;
}

i128 ik_min(int k) {
    if (k == IK_BOOL || !ik_signed(k)) {
        return i128_of(0);
    }
    return i128_neg(i128_shl(i128_of(1), ik_size(k) * 8 - 1));
}

i128 ik_max(int k) {
    if (k == IK_BOOL) {
        return i128_of(1);
    }
    uint bits = ik_size(k) * 8;
    if (ik_signed(k)) {
        bits--;
    }
    return i128_sub(i128_shl(i128_of(1), bits), i128_of(1));
}

/* v converted to the type: truncated to its width, then sign-extended if
 * it is signed (two's complement, as clang does); for _Bool, v != 0. */
i128 ik_wrap(int k, i128 v) {
    if (k == IK_BOOL) {
        return i128_of(!i128_is_zero(v));
    }
    uint bits = ik_size(k) * 8;
    i128 one = i128_of(1);
    i128 w = i128_and(v, i128_sub(i128_shl(one, bits), one));
    if (ik_signed(k) && !i128_is_zero(i128_shr(w, bits - 1))) {
        return i128_sub(w, i128_shl(one, bits));
    }
    return w;
}

/* The usual arithmetic conversions (C99 6.3.1.8), after promotion: two
 * signed or two unsigned types meet at the one of greater rank; otherwise
 * the unsigned type wins if its rank is not lower, else the signed type if
 * it holds every value of the unsigned one, else the signed type's
 * unsigned counterpart. */
int ik_common(int a, int b) {
    a = ik_promoted(a);
    b = ik_promoted(b);
    if (ik_signed(a) == ik_signed(b)) {
        return ik_rank(a) >= ik_rank(b) ? a : b;
    }
    int s = ik_signed(a) ? a : b;
    int u = ik_signed(a) ? b : a;
    if (ik_rank(u) >= ik_rank(s)) {
        return u;
    }
    if (i128_le(ik_max(u), ik_max(s))) {
        return s;
    }
    return ik_to_unsigned(s);
}

const char *quals_name(uint q) {
    if ((q & Q_CONST) && (q & Q_VOLATILE)) {
        return "const volatile";
    }
    if (q & Q_CONST) {
        return "const";
    }
    if (q & Q_VOLATILE) {
        return "volatile";
    }
    return "";
}

/* ---- Making types ---- */

static struct type void_type = {TY_VOID, 0, 0, 0, 0, NULL, NULL, NULL};
struct type *TY_VOID_T = &void_type;

/* One shared type per integer kind. */
static struct type int_types[12];

struct type *ty_int(int k) {
    struct type *t = &int_types[k];
    t->kind = TY_INT;
    t->ik = (uchar)k;
    return t;
}

static struct type *ty_new(int kind) {
    struct type *t = arena_alloc(sizeof *t);
    t->kind = (uchar)kind;
    return t;
}

struct type *ty_ptr(struct type *to) {
    struct type *t = ty_new(TY_PTR);
    t->to = to;
    return t;
}

struct type *ty_array(struct type *of, int has_len, uint n) {
    struct type *t = ty_new(TY_ARRAY);
    t->to = of;
    t->has_len = (uchar)has_len;
    t->len = has_len ? n : 0;
    return t;
}

struct type *ty_func(struct type *ret, struct type **params, uint n, int variadic) {
    struct type *t = ty_new(TY_FUNC);
    struct functype *f = arena_alloc(sizeof *f);
    f->ret = ret;
    f->params = arena_alloc(n * sizeof(struct type *) + 1);
    for (uint i = 0; i < n; i++) {
        f->params[i] = params[i];
    }
    f->nparams = n;
    f->variadic = (uchar)variadic;
    t->fn = f;
    return t;
}

/* The type with qualifiers q added, kept in normal form: on an array they
 * go to the elements, they merge with any already there, and on a
 * function type they mean nothing. */
struct type *ty_qualified(struct type *t, uint q) {
    if (q == 0) {
        return t;
    }
    switch (t->kind) {
    case TY_ARRAY:
        return ty_array(ty_qualified(t->to, q), t->has_len, t->len);
    case TY_QUAL: {
        if ((t->quals | q) == t->quals) {
            return t;
        }
        struct type *r = ty_new(TY_QUAL);
        r->to = t->to;
        r->quals = (uchar)(t->quals | q);
        return r;
    }
    case TY_FUNC:
        return t;
    default: {
        struct type *r = ty_new(TY_QUAL);
        r->to = t;
        r->quals = (uchar)q;
        return r;
    }
    }
}

struct type *ty_unqual(struct type *t) {
    return t->kind == TY_QUAL ? t->to : t;
}

static int fn_eq(struct functype *a, struct functype *b) {
    if (!ty_eq(a->ret, b->ret) || a->nparams != b->nparams || a->variadic != b->variadic ||
        a->unprototyped != b->unprototyped) {
        return 0;
    }
    for (uint i = 0; i < a->nparams; i++) {
        if (!ty_eq(a->params[i], b->params[i])) {
            return 0;
        }
    }
    return 1;
}

/* Rust's derived ==: the same type, records by identity, spellings never
 * part of it. */
int ty_eq(struct type *a, struct type *b) {
    if (a == b) {
        return 1;
    }
    if (a->kind != b->kind) {
        return 0;
    }
    switch (a->kind) {
    case TY_VOID:
        return 1;
    case TY_INT:
        return a->ik == b->ik;
    case TY_PTR:
        return ty_eq(a->to, b->to);
    case TY_ARRAY:
        return a->has_len == b->has_len && a->len == b->len && ty_eq(a->to, b->to);
    case TY_RECORD:
        return a->rec == b->rec;
    case TY_FUNC:
        return fn_eq(a->fn, b->fn);
    default:
        return a->quals == b->quals && ty_eq(a->to, b->to);
    }
}

/* The same type but for qualifiers, at any level of pointers and arrays
 * (clang's hasCvrSimilarType). */
int ty_similar(struct type *a, struct type *b) {
    a = ty_unqual(a);
    b = ty_unqual(b);
    if (a->kind == TY_PTR && b->kind == TY_PTR) {
        return ty_similar(a->to, b->to);
    }
    if (a->kind == TY_ARRAY && b->kind == TY_ARRAY) {
        return a->has_len == b->has_len && a->len == b->len && ty_similar(a->to, b->to);
    }
    return ty_eq(a, b);
}

/* Without its qualifiers, an array's elements' too (getUnqualifiedType). */
struct type *ty_without_quals(struct type *t) {
    t = ty_unqual(t);
    if (t->kind == TY_ARRAY) {
        return ty_array(ty_without_quals(t->to), t->has_len, t->len);
    }
    return t;
}

/* The qualifiers of an object of this type: its own, or an array's
 * elements'. */
uint ty_quals(struct type *t) {
    if (t->kind == TY_QUAL) {
        return t->quals;
    }
    if (t->kind == TY_ARRAY) {
        return ty_quals(t->to);
    }
    return 0;
}

int ty_quals_added_to(struct type *t, struct type *of) {
    uint mine = ty_quals(t);
    uint its = ty_quals(of);
    uint added = mine & ~its;
    return ty_eq(ty_qualified(of, added), t) ? (int)added : -1;
}

int64_t ty_size(struct type *t) {
    switch (t->kind) {
    case TY_VOID:
    case TY_FUNC:
        return -1;
    case TY_INT:
        return ik_size(t->ik);
    case TY_PTR:
        return PTR_SIZE;
    case TY_ARRAY: {
        int64_t e = ty_size(t->to);
        if (e < 0 || !t->has_len) {
            return -1;
        }
        /* u32 arithmetic, as merlin's (the parser keeps sizes in range). */
        return (int64_t)(uint)((uint)e * t->len);
    }
    case TY_RECORD:
        return t->rec->body != NULL ? t->rec->body->size : -1;
    default:
        return ty_size(t->to);
    }
}

uint ty_align(struct type *t) {
    switch (t->kind) {
    case TY_VOID:
    case TY_FUNC:
        return 1;
    case TY_INT:
        return ik_size(t->ik);
    case TY_PTR:
        return PTR_SIZE;
    case TY_ARRAY:
        return ty_align(t->to);
    case TY_RECORD:
        return t->rec->body != NULL ? t->rec->body->align : 1;
    default:
        return ty_align(t->to);
    }
}

int ty_is_integer(struct type *t) {
    return ty_unqual(t)->kind == TY_INT;
}

int ty_is_ptr(struct type *t) {
    return ty_unqual(t)->kind == TY_PTR;
}

int ty_is_array(struct type *t) {
    return t->kind == TY_ARRAY;
}

int ty_is_func(struct type *t) {
    return t->kind == TY_FUNC;
}

int ty_is_void(struct type *t) {
    return ty_unqual(t)->kind == TY_VOID;
}

struct record *ty_record(struct type *t) {
    t = ty_unqual(t);
    return t->kind == TY_RECORD ? t->rec : NULL;
}

int ty_is_scalar(struct type *t) {
    int k = ty_unqual(t)->kind;
    return k == TY_INT || k == TY_PTR;
}

/* Arrays and functions (which decay to their address) and structs and
 * unions (copied from it) stand for an address, not a value. */
int ty_is_addressed(struct type *t) {
    return ty_is_array(t) || ty_is_func(t) || ty_record(t) != NULL;
}

int ty_int_kind(struct type *t) {
    t = ty_unqual(t);
    return t->kind == TY_INT ? t->ik : -1;
}

/* What an expression of this type gives as a value: an array decays to a
 * pointer to its first element, a function to a pointer to itself, and
 * top-level qualifiers go. */
struct type *ty_decay(struct type *t) {
    switch (t->kind) {
    case TY_ARRAY:
        return ty_ptr(t->to);
    case TY_FUNC:
        return ty_ptr(t);
    case TY_QUAL:
        return t->to;
    default:
        return t;
    }
}

/* What adding 1 to a pointer moves it by: 1 for void and a function (GNU's
 * extension, which clang takes without a word). */
int64_t ty_step(struct type *t) {
    struct type *to = ty_pointee(t);
    if (to == NULL) {
        return -1;
    }
    if (ty_is_void(to) || ty_is_func(to)) {
        return 1;
    }
    return ty_size(to);
}

struct type *ty_pointee(struct type *t) {
    t = ty_unqual(t);
    return t->kind == TY_PTR || t->kind == TY_ARRAY ? t->to : NULL;
}

/* Whether operations on the value, promoted, are unsigned: pointers too. */
int ty_is_unsigned(struct type *t) {
    t = ty_unqual(t);
    switch (t->kind) {
    case TY_INT:
        return !ik_signed(ik_promoted(t->ik));
    case TY_PTR:
    case TY_ARRAY:
    case TY_FUNC:
        return 1;
    default:
        return 0;
    }
}

/* ---- Function types ---- */

/* All but (), which says nothing of the parameters (C99 6.7.5.3p14). */
int fn_has_prototype(struct functype *f) {
    return !(f->unprototyped && f->nparams == 0 && !f->variadic);
}

static int promotes_to_itself(struct type *t) {
    t = ty_unqual(t);
    return t->kind != TY_INT || ik_promoted(t->ik) == t->ik;
}

struct spelling *fn_param_spelled(struct functype *f, uint i) {
    if (i < f->nspelled && f->nspelled == f->nparams) {
        return f->spelled[i].spelled;
    }
    return NULL;
}

/* Whether the spellings are there for parameter i. */
static int has_param_spelled(struct functype *f, uint i) {
    return i < f->nspelled && f->nspelled == f->nparams;
}

static int param_enums_differ(struct functype *a, struct functype *b) {
    for (uint i = 0; i < a->nparams && i < b->nparams; i++) {
        if (has_param_spelled(a, i) && has_param_spelled(b, i) &&
            distinct_enums(a->params[i], fn_param_spelled(a, i), b->params[i], fn_param_spelled(b, i))) {
            return 1;
        }
    }
    return 0;
}

/* C99 6.7.5.3p15: their results are compatible, and with prototypes both
 * so are their parameters; one without a prototype is compatible with a
 * prototype whose parameters promote to themselves, with no `...`. */
int fn_compatible(struct functype *a, struct functype *b) {
    if (!ty_compatible(a->ret, b->ret)) {
        return 0;
    }
    int pa = fn_has_prototype(a);
    int pb = fn_has_prototype(b);
    if (pa && pb) {
        if (a->variadic != b->variadic || a->nparams != b->nparams) {
            return 0;
        }
        for (uint i = 0; i < a->nparams; i++) {
            if (!ty_compatible(ty_unqual(a->params[i]), ty_unqual(b->params[i]))) {
                return 0;
            }
        }
        return !param_enums_differ(a, b);
    }
    if (!pa && !pb) {
        return 1;
    }
    struct functype *proto = pa ? a : b;
    if (proto->variadic) {
        return 0;
    }
    for (uint i = 0; i < proto->nparams; i++) {
        if (!promotes_to_itself(proto->params[i])) {
            return 0;
        }
    }
    return 1;
}

/* The enum ty is, if a declaration spelled so declared it (merlin's
 * enum_of): through its typedefs to an enum of the same type. */
struct enum_type *enum_of(struct spelling *spelled, struct type *ty) {
    struct spelling *s = spelled;
    while (s != NULL && s->kind == SP_TYPEDEF) {
        s = s->td->spelled;
    }
    if (s != NULL && s->kind == SP_ENUM && ty_eq(ty_unqual(ty), s->en->ty)) {
        return s->en;
    }
    return NULL;
}

/* Whether a and b are made of two different enums where they are alike. */
int distinct_enums(struct type *a, struct spelling *sa, struct type *b, struct spelling *sb) {
    a = ty_unqual(a);
    b = ty_unqual(b);
    if ((a->kind == TY_PTR && b->kind == TY_PTR) || (a->kind == TY_ARRAY && b->kind == TY_ARRAY)) {
        return distinct_enums(a->to, sa, b->to, sb);
    }
    if (a->kind == TY_FUNC && b->kind == TY_FUNC) {
        return distinct_enums(a->fn->ret, sa, b->fn->ret, sb) || param_enums_differ(a->fn, b->fn);
    }
    if (a->kind == TY_INT && b->kind == TY_INT) {
        struct enum_type *x = enum_of(sa, a);
        struct enum_type *y = enum_of(sb, b);
        return x != NULL && y != NULL && x != y;
    }
    return 0;
}

/* C99 6.2.7: the same, but that an array of unknown size is compatible
 * with one of a size, and function types as fn_compatible has them, at
 * any depth. */
int ty_compatible(struct type *a, struct type *b) {
    if (a->kind == TY_QUAL && b->kind == TY_QUAL) {
        return a->quals == b->quals && ty_compatible(a->to, b->to);
    }
    if (a->kind == TY_QUAL || b->kind == TY_QUAL) {
        return 0;
    }
    if (a->kind == TY_PTR && b->kind == TY_PTR) {
        return ty_compatible(a->to, b->to);
    }
    if (a->kind == TY_ARRAY && b->kind == TY_ARRAY) {
        return ty_compatible(a->to, b->to) &&
               ((a->has_len == b->has_len && a->len == b->len) || !a->has_len || !b->has_len);
    }
    if (a->kind == TY_FUNC && b->kind == TY_FUNC) {
        return fn_compatible(a->fn, b->fn);
    }
    return ty_eq(a, b);
}

static struct type *func_copy(struct functype *f) {
    struct type *t = ty_func(f->ret, f->params, f->nparams, f->variadic);
    t->fn->unprototyped = f->unprototyped;
    t->fn->spelled = f->spelled;
    t->fn->nspelled = f->nspelled;
    return t;
}

/* The composite type of two compatible types (C99 6.2.7p3). */
struct type *ty_composite(struct type *a, struct type *b) {
    if (a->kind == TY_QUAL && b->kind == TY_QUAL) {
        struct type *r = ty_new(TY_QUAL);
        r->to = ty_composite(a->to, b->to);
        r->quals = a->quals;
        return r;
    }
    if (a->kind == TY_PTR && b->kind == TY_PTR) {
        return ty_ptr(ty_composite(a->to, b->to));
    }
    if (a->kind == TY_ARRAY && b->kind == TY_ARRAY) {
        int has = a->has_len || b->has_len;
        return ty_array(ty_composite(a->to, b->to), has, a->has_len ? a->len : b->len);
    }
    if (a->kind == TY_FUNC && b->kind == TY_FUNC) {
        struct functype *f = a->fn;
        struct functype *g = b->fn;
        struct type *ret = ty_composite(f->ret, g->ret);
        struct type *c;
        int pf = fn_has_prototype(f);
        int pg = fn_has_prototype(g);
        if (!pf && pg) {
            c = func_copy(g);
        } else if (pf && pg) {
            c = func_copy(f);
            uint n = f->nparams < g->nparams ? f->nparams : g->nparams;
            for (uint i = 0; i < n; i++) {
                c->fn->params[i] = ty_composite(f->params[i], g->params[i]);
            }
            /* zip: as many as the shorter. */
            c->fn->nparams = n;
        } else {
            c = func_copy(f);
        }
        c->fn->ret = ret;
        return c;
    }
    return a;
}

/* ---- Records and enums ---- */

struct record *record_new(int is_union, const char *tag, struct pos pos) {
    struct record *r = arena_alloc(sizeof *r);
    r->is_union = (uchar)is_union;
    r->tag = tag;
    r->pos = pos;
    return r;
}

const char *record_keyword(struct record *r) {
    return r->is_union ? "union" : "struct";
}

uint64_t align_up(uint64_t n, uint align) {
    uint64_t a = align;
    return (n + a - 1) / a * a;
}

int record_complete(struct record *r, struct member_decl *members, uint n, struct pos pos) {
    if (r->body != NULL) {
        return 0;
    }
    struct record_body *b = arena_alloc(sizeof *b);
    b->members = arena_alloc(n * sizeof(struct member) + 1);
    uint64_t offset = 0;
    uint64_t size = 0;
    uint align = 1;
    for (uint i = 0; i < n; i++) {
        uint64_t msize = (uint64_t)ty_size(members[i].ty);
        uint malign = ty_align(members[i].ty);
        if (malign > align) {
            align = malign;
        }
        uint64_t at = r->is_union ? 0 : align_up(offset, malign);
        offset = at + msize;
        if (offset > size) {
            size = offset;
        }
        struct member *m = &b->members[i];
        m->name = members[i].name;
        m->ty = members[i].ty;
        m->offset = (uint)at;
        m->spelled = members[i].spelled;
        m->pos = members[i].pos;
        m->begin = members[i].begin;
    }
    b->nmembers = n;
    b->size = (uint)align_up(size, align);
    b->align = align;
    b->pos = pos;
    r->body = b;
    return 1;
}

/* How clang's messages name it: `struct node`; without a tag, the typedef
 * that names it, or where it is declared. */
char *record_name(struct record *r) {
    if (r->tag != NULL) {
        return fmt("%s %s", record_keyword(r), r->tag);
    }
    if (r->typedef_name != NULL) {
        return (char *)r->typedef_name;
    }
    return fmt("%s (unnamed at %s)", record_keyword(r), place(r->pos));
}

struct member *record_member(struct record_body *b, const char *name) {
    for (uint i = 0; i < b->nmembers; i++) {
        if (strcmp(b->members[i].name, name) == 0) {
            return &b->members[i];
        }
    }
    return NULL;
}

char *enum_name(struct enum_type *e) {
    if (e->tag != NULL) {
        return fmt("enum %s", e->tag);
    }
    if (e->typedef_name != NULL) {
        return (char *)e->typedef_name;
    }
    return fmt("enum (unnamed at %s)", place(e->pos));
}

struct type *spelling_ty(struct spelling *s) {
    return s->kind == SP_TYPEDEF ? s->td->ty : s->en->ty;
}

struct spelling *spelling_typedef(struct typedef_name *t) {
    struct spelling *s = arena_alloc(sizeof *s);
    s->kind = SP_TYPEDEF;
    s->td = t;
    return s;
}

struct spelling *spelling_enum(struct enum_type *e) {
    struct spelling *s = arena_alloc(sizeof *s);
    s->kind = SP_ENUM;
    s->en = e;
    return s;
}

/* lib.rs's place: the file a number between two FILE_MARKs. */
char *place(struct pos pos) {
    return fmt("%c%u%c%u:%u", FILE_MARK, pos.file, FILE_MARK, pos.line, pos.col);
}

/* ---- Names ---- */

/* A declarator that starts with `*` binds looser than [] and (), so it
 * needs parentheses before them. */
static char *parenthesize(char *inner) {
    return inner[0] == '*' ? fmt("(%s)", inner) : inner;
}

/* The base and the declarator, apart, but for an array's brackets with no
 * name before them, as clang writes them: int[2], int *[2]. */
static char *join(const char *base, const char *inner) {
    if (inner[0] == 0 || inner[0] == '[') {
        return fmt("%s%s", base, inner);
    }
    return fmt("%s %s", base, inner);
}

static char *spelled_as(struct type *t, const char *name, struct spelling *spelled, int sugar);

/* merlin's declare_as's go: t, around the declarator `inner`, with the part
 * that is base_ty written as base_name, and the parameters as spelled if
 * sugar is 0 or 1 (-1: plainly). */
static char *declare_go(struct type *t, char *inner, struct type *base_ty, const char *base_name, int sugar) {
    if (base_ty != NULL && ty_eq(t, base_ty)) {
        return join(base_name, inner);
    }
    switch (t->kind) {
    case TY_VOID:
        return join("void", inner);
    case TY_INT:
        return join(ik_name(t->ik), inner);
    case TY_RECORD:
        return join(record_name(t->rec), inner);
    case TY_PTR:
        return declare_go(t->to, fmt("*%s", inner), base_ty, base_name, sugar);
    case TY_QUAL:
        /* `int *const p`, but `const int x`. */
        if (t->to->kind == TY_PTR) {
            return declare_go(t->to->to, fmt("*%s", join(quals_name(t->quals), inner)), base_ty, base_name,
                              sugar);
        }
        return fmt("%s %s", quals_name(t->quals), declare_go(t->to, inner, base_ty, base_name, sugar));
    case TY_ARRAY: {
        char *n = t->has_len ? fmt("%u", t->len) : "";
        return declare_go(t->to, fmt("%s[%s]", parenthesize(inner), n), base_ty, base_name, sugar);
    }
    default: {
        struct functype *f = t->fn;
        struct buf b;
        memset(&b, 0, sizeof b);
        uint count = 0;
        for (uint i = 0; i < f->nparams; i++) {
            if (count++ > 0) {
                buf_str(&b, ", ");
            }
            if (sugar >= 0 && has_param_spelled(f, i)) {
                buf_str(&b, spelled_as(f->params[i], "", fn_param_spelled(f, i), sugar));
            } else {
                buf_str(&b, ty_declare(f->params[i], ""));
            }
        }
        if (f->variadic) {
            if (count++ > 0) {
                buf_str(&b, ", ");
            }
            buf_str(&b, "...");
        }
        char *params = count == 0 && !f->unprototyped ? "void" : buf_arena(&b);
        buf_free(&b);
        return declare_go(f->ret, fmt("%s(%s)", parenthesize(inner), params), base_ty, base_name, sugar);
    }
    }
}

char *ty_declare(struct type *t, const char *name) {
    return declare_go(t, (char *)name, NULL, NULL, -1);
}

char *ty_str(struct type *t) {
    return ty_declare(t, "");
}

/* The type as a declaration spelled it: with typedef names if sugar, else
 * only with enums' names. */
static char *spelled_as(struct type *t, const char *name, struct spelling *spelled, int sugar) {
    struct spelling *base = spelled;
    while (base != NULL && base->kind == SP_TYPEDEF && !sugar) {
        base = base->td->spelled;
    }
    struct type *bt = NULL;
    const char *bn = NULL;
    if (base != NULL && base->kind == SP_TYPEDEF) {
        bt = base->td->ty;
        bn = base->td->name;
    } else if (base != NULL) {
        bt = base->en->ty;
        bn = enum_name(base->en);
    }
    return declare_go(t, (char *)name, bt, bn, sugar);
}

/* The type as clang names it in a message, quoted: through a typedef
 * name, its name with what it stands for after it ('T *' (aka 'int *')). */
char *ty_named(struct type *t, struct spelling *spelled) {
    char *plain = spelled_as(t, "", spelled, 0);
    char *sugared = spelled_as(t, "", spelled, 1);
    if (strcmp(sugared, plain) != 0) {
        return fmt("'%s' (aka '%s')", sugared, plain);
    }
    if (spelled != NULL && spelled->kind == SP_TYPEDEF) {
        return fmt("'%s'", sugared);
    }
    return fmt("'%s'", plain);
}
