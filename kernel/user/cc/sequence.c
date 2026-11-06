/* cc1: clang's -Wunsequenced: two modifications of one variable, or a
 * modification and a read of it, that C leaves unsequenced in one full
 * expression (i = i++ + 1, a[i] = i++), which is undefined behaviour (C99
 * 6.5p2). Follows merlin's sequence.rs, after clang's SequenceChecker
 * (SemaChecking.cpp) as it runs on C:
 *
 * The expression is walked in order, each read of a variable and each
 * modification (=, op=, ++, --) noted with the region it is in. Regions
 * form a tree: an operator that orders its operands (`,`, &&, ||, ?:) gives
 * each a region of its own under the current one, and the regions of what
 * it ordered are merged into it once it is done. Two usages are
 * unsequenced when the older one's region (or what it was merged into) is
 * the newer one's or one of its ancestors. A modification is a value as
 * soon as it happens or a side effect that only comes after (x++, x = 1 in
 * C): a read conflicts with a modification that is a value before it is
 * made and with a side effect after. A sequenced subexpression (the left
 * operand of `,`, &&, ||, a ?:'s condition, a call's operands) makes its
 * side effects values once it is done. Each variable is reported once, at
 * the modification. */
#include "cc.h"

/* How a variable is used: read, modified with the new value there at once,
 * modified with the store a side effect that comes later. */
enum { USE, MOD_AS_VALUE, MOD_AS_SIDE_EFFECT };

/* A usage: where, of what expression, in which region. */
struct usage {
    int has;
    struct pos at;
    uint64_t of;
    uint region;
};

struct usages {
    const char *name;
    struct usage uses[3];
    int diagnosed;
};

/* A side effect noted in a sequenced subexpression, with what it
 * replaced. */
struct saved {
    const char *key;
    struct usage prev;
};

struct seq {
    /* The regions: each one's parent, and whether it was merged into it. */
    uint *parent;
    uchar *merged;
    uint nregions;
    uint cap;
    uint capm;
    uint region;
    /* The variable's key ("L3", "Gname") -> struct usages *. */
    struct map usages;
    /* struct saved *, or none (has_saved 0). */
    int has_saved;
    struct vec saved;
    /* struct finding, found. */
    struct finding *found;
    uint nfound;
    uint capfound;
};

static uint allocate(struct seq *s, uint parent) {
    s->parent = grow(s->parent, &s->cap, s->nregions + 1, sizeof(uint));
    s->merged = grow(s->merged, &s->capm, s->nregions + 1, 1);
    s->parent[s->nregions] = parent;
    s->merged[s->nregions] = 0;
    return s->nregions++;
}

static void merge(struct seq *s, uint r) {
    s->merged[r] = 1;
}

/* The region r counts as: the first not merged into its parent. */
static uint representative(struct seq *s, uint r) {
    while (s->merged[r]) {
        r = s->parent[r];
    }
    return r;
}

/* Whether what is in old is unsequenced with what is in cur (the more
 * recent): old stands for cur or one of its ancestors. */
static int unsequenced(struct seq *s, uint cur, uint old) {
    uint c = representative(s, cur);
    uint target = representative(s, old);
    while (c >= target) {
        if (c == target) {
            return 1;
        }
        if (c == 0) {
            break;
        }
        c = s->parent[c];
    }
    return 0;
}

/* The variable e designates, through casts, as a modification's target
 * (clang's getObject): its key and name; NULL for none. */
static const char *object(struct expr *e, int modified, const char **name) {
    switch (e->kind) {
    case E_CAST:
        return object(e->x, modified, name);
    case E_COMMA:
        return object(e->y, modified, name);
    case E_ASSIGN:
        return modified ? object(e->x, modified, name) : NULL;
    case E_INCDEC:
        return modified && e->prefix ? object(e->x, modified, name) : NULL;
    case E_VAR:
        if (e->var.kind == VAR_NONE) {
            return NULL;
        }
        *name = e->name;
        return e->var.kind == VAR_LOCAL ? fmt("L%u", e->var.local) : fmt("G%s", e->var.sym);
    default:
        return NULL;
    }
}

static struct usages *usages_of(struct seq *s, const char *key, const char *name) {
    struct usages *u = map_get(&s->usages, key);
    if (u == NULL) {
        u = arena_alloc(sizeof *u);
        u->name = name;
        map_put(&s->usages, key, u);
    }
    return u;
}

/* clang's checkUsage: the usage cur of o against its earlier one of the
 * other kind, if they are unsequenced. */
static void conflict(struct seq *s, const char *key, const char *name, struct usage cur, int other, int mod_mod) {
    struct usages *u = usages_of(s, key, name);
    if (u->diagnosed) {
        return;
    }
    struct usage prev = u->uses[other];
    if (!prev.has || !unsequenced(s, s->region, prev.region)) {
        return;
    }
    /* At the modification: the earlier usage, unless it is the read. */
    struct usage m = other == USE ? cur : prev;
    u->diagnosed = 1;
    s->found = grow(s->found, &s->capfound, s->nfound + 1, sizeof(struct finding));
    s->found[s->nfound].at = m.at;
    s->found[s->nfound].of = m.of;
    s->found[s->nfound].msg = mod_mod ? fmt("multiple unsequenced modifications to '%s'", u->name)
                                      : fmt("unsequenced modification and access to '%s'", u->name);
    s->nfound++;
}

/* clang's addUsage: notes cur as the usage of its kind, unless the one
 * there is unsequenced with it. */
static void add(struct seq *s, const char *key, const char *name, struct usage cur, int kind) {
    struct usages *u = usages_of(s, key, name);
    struct usage prev = u->uses[kind];
    if (prev.has && unsequenced(s, s->region, prev.region)) {
        return;
    }
    if (kind == MOD_AS_SIDE_EFFECT && s->has_saved) {
        struct saved *sv = arena_alloc(sizeof *sv);
        sv->key = key;
        sv->prev = prev;
        vec_push(&s->saved, sv);
    }
    u->uses[kind] = cur;
}

static struct usage usage(struct seq *s, struct expr *e, struct pos at) {
    struct usage u;
    u.has = 1;
    u.at = at;
    u.of = e->id;
    u.region = s->region;
    return u;
}

static void pre_use(struct seq *s, const char *key, const char *name, struct usage u) {
    conflict(s, key, name, u, MOD_AS_VALUE, 0);
}

static void post_use(struct seq *s, const char *key, const char *name, struct usage u) {
    conflict(s, key, name, u, MOD_AS_SIDE_EFFECT, 0);
    add(s, key, name, u, USE);
}

static void pre_mod(struct seq *s, const char *key, const char *name, struct usage u) {
    conflict(s, key, name, u, MOD_AS_VALUE, 1);
    conflict(s, key, name, u, USE, 0);
}

static void post_mod(struct seq *s, const char *key, const char *name, struct usage u, int kind) {
    conflict(s, key, name, u, MOD_AS_SIDE_EFFECT, 1);
    add(s, key, name, u, kind);
}

static void value(struct seq *s, struct expr *e);
static void object_place(struct seq *s, struct expr *e);
static void init_items(struct seq *s, struct init *init);

/* Ends a sequenced subexpression (clang's SequencedSubexpression): its side
 * effects become values. outer is what was being saved around it. */
static void end_sequenced(struct seq *s, int had, struct vec outer) {
    struct vec saved = s->saved;
    s->saved = outer;
    s->has_saved = had;
    for (uint i = saved.n; i > 0; i--) {
        struct saved *sv = saved.v[i - 1];
        struct usages *u = map_get(&s->usages, sv->key);
        struct usage side = u->uses[MOD_AS_SIDE_EFFECT];
        if (side.has) {
            add(s, sv->key, u->name, side, MOD_AS_VALUE);
        }
        u = map_get(&s->usages, sv->key);
        u->uses[MOD_AS_SIDE_EFFECT] = sv->prev;
    }
    heap_free(saved.v);
}

/* The value of e as a sequenced subexpression. */
static void sequenced_value(struct seq *s, struct expr *e) {
    int had = s->has_saved;
    struct vec outer = vec_take(&s->saved);
    s->has_saved = 1;
    value(s, e);
    end_sequenced(s, had, outer);
}

/* first, then then, each in a region of its own, first a sequenced
 * subexpression (`,`). */
static void in_order(struct seq *s, struct expr *first, struct expr *then) {
    uint before = allocate(s, s->region);
    uint after = allocate(s, s->region);
    uint outer = s->region;
    s->region = before;
    sequenced_value(s, first);
    s->region = after;
    value(s, then);
    s->region = outer;
    merge(s, before);
    merge(s, after);
}

/* e's value: what it reads and modifies. */
static void value(struct seq *s, struct expr *e) {
    const char *name = NULL;
    const char *key;
    i128 v;
    switch (e->kind) {
    case E_VAR:
        if (!ty_is_array(e->ty) && !ty_is_func(e->ty)) {
            key = object(e, 0, &name);
            if (key != NULL) {
                struct usage u = usage(s, e, e->pos);
                pre_use(s, key, name, u);
                post_use(s, key, name, u);
            }
        }
        return;
    case E_CAST:
        value(s, e->x);
        return;
    case E_ASSIGN: {
        key = object(e->x, 1, &name);
        struct usage u = usage(s, e, e->pos);
        if (key != NULL) {
            pre_mod(s, key, name, u);
        }
        object_place(s, e->x);
        /* t op= v reads t too. */
        if (key != NULL && e->op != OP_NONE) {
            post_use(s, key, name, u);
        }
        value(s, e->y);
        if (key != NULL) {
            post_mod(s, key, name, u, MOD_AS_SIDE_EFFECT);
        }
        return;
    }
    case E_INCDEC: {
        key = object(e->x, 1, &name);
        if (key == NULL) {
            object_place(s, e->x);
            return;
        }
        struct usage u = usage(s, e, e->pos);
        pre_mod(s, key, name, u);
        object_place(s, e->x);
        post_mod(s, key, name, u, MOD_AS_SIDE_EFFECT);
        return;
    }
    case E_BINARY:
        if (e->op == OP_LOGAND || e->op == OP_LOGOR) {
            uint left = allocate(s, s->region);
            uint right = allocate(s, s->region);
            uint outer = s->region;
            s->region = left;
            sequenced_value(s, e->x);
            /* The right operand, unless the left one decides it. */
            int decided = consteval_int(e->x, &v) ? !i128_is_zero(v) : -1;
            if (decided != (e->op == OP_LOGOR)) {
                s->region = right;
                value(s, e->y);
            }
            s->region = outer;
            merge(s, left);
            merge(s, right);
            return;
        }
        value(s, e->x);
        value(s, e->y);
        return;
    case E_COMMA:
        in_order(s, e->x, e->y);
        return;
    case E_COND: {
        uint outer = s->region;
        uint cond = allocate(s, outer);
        uint yes = allocate(s, outer);
        uint no = allocate(s, outer);
        s->region = cond;
        sequenced_value(s, e->x);
        int decided = consteval_int(e->x, &v) ? !i128_is_zero(v) : -1;
        if (decided != 0) {
            s->region = yes;
            value(s, e->y);
        }
        if (decided != 1) {
            s->region = no;
            value(s, e->z);
        }
        s->region = outer;
        merge(s, cond);
        merge(s, yes);
        merge(s, no);
        return;
    }
    case E_ADDR:
        object_place(s, e->x);
        return;
    case E_MEMBER:
        if (e->flag) {
            value(s, e->x);
        } else {
            object_place(s, e->x);
        }
        return;
    case E_CALL: {
        int had = s->has_saved;
        struct vec outer = vec_take(&s->saved);
        s->has_saved = 1;
        value(s, e->x);
        for (uint i = 0; i < e->nargs; i++) {
            value(s, e->args[i]);
        }
        end_sequenced(s, had, outer);
        return;
    }
    case E_SIZEOF_EXPR:
    case E_SIZEOF_TYPE:
        return;
    case E_COMPOUND:
        /* A compound literal's items, in the region it is in. */
        init_items(s, e->init);
        return;
    case E_VA_COPY:
    case E_VA_START:
        value(s, e->x);
        value(s, e->y);
        return;
    case E_UNARY:
    case E_DEREF:
    case E_VA_ARG:
    case E_VA_END:
        value(s, e->x);
        return;
    default:
        return;
    }
}

/* The items of a list, all in the current region. */
static void init_items(struct seq *s, struct init *init) {
    if (init->kind == I_EXPR) {
        value(s, init->e);
    } else if (init->kind == I_LIST) {
        for (uint i = 0; i < init->items.n; i++) {
            init_items(s, init->items.v[i]);
        }
    } else {
        for (uint i = 0; i < init->nds; i++) {
            if (!init->ds[i]->member) {
                value(s, init->ds[i]->index);
            }
        }
        init_items(s, init->inner);
    }
}

/* e as the object an assignment, ++, & or `.` is of: not read itself, what
 * designates it is. */
static void object_place(struct seq *s, struct expr *e) {
    if (e->kind == E_VAR) {
        return;
    }
    if (e->kind == E_CAST && e->implicit) {
        object_place(s, e->x);
        return;
    }
    if (e->kind == E_MEMBER && !e->flag) {
        object_place(s, e->x);
        return;
    }
    value(s, e);
}

static struct seq *seq_new(void) {
    struct seq *s = heap_calloc(1, sizeof(struct seq));
    allocate(s, 0);
    return s;
}

static uint seq_done(struct seq *s, struct finding **out) {
    uint n = s->nfound;
    *out = s->found;
    heap_free(s->parent);
    heap_free(s->merged);
    heap_free(s->usages.keys);
    heap_free(s->usages.values);
    heap_free(s);
    return n;
}

/* The -Wunsequenced warnings of the full expression e, in order. */
uint sequence_check(struct expr *e, struct finding **out) {
    struct seq *s = seq_new();
    value(s, e);
    return seq_done(s, out);
}

/* Of a declaration's list, one full expression whose items C leaves
 * unsequenced (clang's VisitInitListExpr in C): {i++, i++}. */
uint sequence_check_init(struct init *init, struct finding **out) {
    struct seq *s = seq_new();
    init_items(s, init);
    return seq_done(s, out);
}
