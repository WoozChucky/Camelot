/* cc1: code generation, the checked tree to Excalibur assembly for as
 * (follows merlin's codegen.rs, whose module comment explains it in full:
 * the stack frame, the calling convention, the 32-bit invariant, the order
 * clang emits functions and variables in, the symbol names, control flow,
 * unreachable code and branch relaxation).
 *
 * It is a stack machine: every expression leaves its value in a0, and a
 * value that must survive while another expression is evaluated is pushed
 * on the stack. Every variable lives in memory. An int lives in a 64-bit
 * register sign-extended from bit 31, as the psABI requires; the W
 * instructions keep it so. Structs and arrays leave their address in a0.
 *
 * Functions and variables are generated in the order clang's front end
 * emits them and written in the order its module has them, so .text,
 * .data, .bss and .rodata hold them as clang's -O0 output does. Each
 * function is generated twice: a first pass finds the locals the code
 * generated uses (as clang's frame at -O0 has room only for those), the
 * second lays out those alone.
 *
 * -g (merlin's debug.rs) is the next stage of cc1: where merlin's code
 * generator writes debug information, this one writes nothing. */
#include "cc.h"

#define TRY(x)                                                                                               \
    do {                                                                                                     \
        struct diag *try_ = (x);                                                                             \
        if (try_ != NULL) {                                                                                  \
            return try_;                                                                                     \
        }                                                                                                    \
    } while (0)

/* `ra` and the caller's s0 take the top two words of every frame: 16. */
#define SAVED_REGS (2 * WORD)
/* The largest frame: offsets from s0 and the frame size stay ints. */
#define MAX_FRAME (2147483647ll & ~15ll)

/* ---- Lines of a function ---- */

/* One line of a function's assembly (merlin's Line): `name:`, an
 * instruction (with the most bytes it can assemble to), a conditional
 * branch, whose reach is limited (relax_branches), or text (a comment, a
 * directive) of no bytes. */
enum { LINE_LABEL, LINE_INSN, LINE_BRANCH, LINE_TEXT };
struct line {
    int kind;
    /* LINE_LABEL: the name; LINE_INSN, LINE_TEXT: the text; LINE_BRANCH:
     * the target. */
    char *text;
    uint size;
    /* LINE_BRANCH: `op regs, target`. */
    const char *op;
    const char *regs;
};

/* ---- The file's definitions ---- */

/* A definition of the file: a function, or a global variable (merlin's
 * Def). */
struct def {
    struct function *f;
    struct declarator *d;
};

static const char *def_symbol(struct def *d) {
    return d->f != NULL ? d->f->name : d->d->var.sym;
}

/* Emitted only once used: a static function or global (and an inline
 * definition), as clang's MustBeEmitted says of internal linkage. */
static int def_deferrable(struct def *d) {
    if (d->f != NULL) {
        return d->f->is_static || d->f->inline_only;
    }
    return d->d->storage == ST_STATIC;
}

/* A global without an initializer: emitted at the end of the file. */
static int def_tentative(struct def *d) {
    return d->d != NULL && d->d->init == NULL;
}

/* A compound literal of the file's scope, which the initializer that names
 * it emits. */
static int def_hidden(struct def *d) {
    return d->d != NULL && strcmp(d->d->name, ".compoundliteral") == 0;
}

/* A piece of a symbol's output, in its section. */
struct piece {
    const char *section;
    char *text;
    uint len;
};

/* A string literal: its array's bytes (the NUL included) and its label. */
struct strlit {
    uchar *bytes;
    uint len;
    const char *label;
};

/* What is shared by the whole file (merlin's FileGen). */
struct filegen {
    /* The text being written: the piece of a symbol's output in section,
     * or the whole file at the end. */
    struct buf out;
    const char *section;
    /* struct piece *: the pieces of the symbol being written so far. */
    struct vec pieces;
    /* The label counter, shared by every function. */
    uint labels;
    /* struct strlit *, emitted at the end. */
    struct vec strings;
    /* The module's symbol names (merlin's SymbolTable): the names taken,
     * and the last number appended. */
    struct map taken;
    uint last_unique;
    /* key -> the symbol it is written as. */
    struct map names;
    struct def *defs;
    uint ndefs;
    /* symbol -> its definition's index + 1 (as a pointer). */
    struct map by_symbol;
    uchar *emitted;
    uchar *waiting;
    /* Indexes + 1 of what is to be emitted (clang's DeferredDeclsToEmit). */
    struct vec queue;
    /* Every symbol made so far, in the order it was made. */
    struct vec created;
    struct map created_set;
    /* symbol -> the type a reference made it with. */
    struct map referred_as;
    /* The names of the constants counted but not written. */
    struct map held;
    /* symbol -> struct vec * of its pieces. */
    struct map chunks;
    /* A function's first pass is being generated: it makes nothing. */
    int dry;
};

static void out_str(struct filegen *file, const char *s) {
    buf_str(&file->out, s);
}

/* Goes on in section, writing its directive (merlin's switch). */
static void switch_section(struct filegen *file, const char *section) {
    if (file->section != NULL && strcmp(file->section, section) == 0) {
        return;
    }
    const char *directive = strcmp(section, ".rodata") == 0 ? ".section .rodata" : section;
    /* A blank line sets the new section apart from what came before. */
    if (file->out.len > 0) {
        buf_byte(&file->out, '\n');
    }
    buf_fmt(&file->out, "    %s\n", directive);
    file->section = section;
}

/* ---- Symbol names ---- */

/* Takes the name base, or, if it is taken, base.N for the first N after the
 * last one given that makes a name not taken (makeUniqueName). */
static const char *symbols_insert(struct filegen *file, const char *base) {
    if (map_get(&file->taken, base) == NULL) {
        const char *k = arena_strdup(base);
        map_put(&file->taken, k, (void *)k);
        return k;
    }
    for (;;) {
        file->last_unique++;
        const char *name = fmt("%s.%u", base, file->last_unique);
        if (map_get(&file->taken, name) == NULL) {
            map_put(&file->taken, name, (void *)name);
            return name;
        }
    }
}

/* A second global made under the name base of one that has it, which then
 * takes the name over (takeName): the counter moves on. */
static void symbols_rename(struct filegen *file, const char *base) {
    for (;;) {
        file->last_unique++;
        if (map_get(&file->taken, fmt("%s.%u", base, file->last_unique)) == NULL) {
            return;
        }
    }
}

/* Whether symbol key key is a C name, not one sema makes up (with a `.` or
 * a `#`). */
static int is_c_name(const char *key) {
    return strchr(key, '.') == NULL && strchr(key, '#') == NULL;
}

static struct diag *emit(struct filegen *file, uint k);

/* Makes symbol sym (a key sema gave it), as clang makes a function or global
 * the first time it is defined or referred to: the output has the symbols
 * in this order, and the symbol takes its name now. A deferred definition
 * waiting for its first use is to be emitted now. */
static void create(struct filegen *file, const char *sym) {
    if (file->dry || map_get(&file->created_set, sym) != NULL) {
        return;
    }
    map_put(&file->created_set, sym, (void *)sym);
    /* A function or file-scope variable keeps its own name; the names sema
     * makes up are made unique. */
    const char *name;
    if (is_c_name(sym)) {
        map_put(&file->taken, sym, (void *)sym);
        name = sym;
    } else {
        name = symbols_insert(file, symbol_base(sym));
    }
    map_put(&file->names, sym, (void *)name);
    vec_push(&file->created, (void *)sym);
    uintptr_t k1 = (uintptr_t)map_get(&file->by_symbol, sym);
    if (k1 != 0 && file->waiting[k1 - 1]) {
        file->waiting[k1 - 1] = 0;
        vec_push(&file->queue, (void *)k1);
    }
}

/* The symbol made for key sym; a first pass writes the key. */
static const char *sym_name(struct filegen *file, const char *sym) {
    const char *n = map_get(&file->names, sym);
    return n != NULL ? n : sym;
}

/* A reference to symbol sym in the code or the data generated, of a
 * variable declared there with type ty (or NULL). A compound literal it
 * names is emitted first. */
static void reference(struct filegen *file, const char *sym, struct type *ty) {
    if (file->dry) {
        return;
    }
    uintptr_t k1 = (uintptr_t)map_get(&file->by_symbol, sym);
    if (k1 != 0 && def_hidden(&file->defs[k1 - 1]) && !file->emitted[k1 - 1]) {
        emit(file, (uint)(k1 - 1));
    }
    if (ty != NULL && map_get(&file->created_set, sym) == NULL) {
        map_put(&file->referred_as, sym, ty);
    }
    create(file, sym);
}

/* Moves symbol sym, made already, to after every symbol made so far. */
static void made_now(struct filegen *file, const char *sym) {
    uint k = 0;
    for (uint i = 0; i < file->created.n; i++) {
        if (strcmp(file->created.v[i], sym) != 0) {
            file->created.v[k++] = file->created.v[i];
        }
    }
    file->created.n = k;
    vec_push(&file->created, (void *)sym);
}

/* The definition of function or variable sym (a C name), when a constant
 * clang made, counted but not written, has its name: the definition takes
 * it over (merlin's take_over). Whether the name was the constant's. */
static int take_over(struct filegen *file, const char *sym, int variable) {
    if (file->dry || !map_remove(&file->held, sym)) {
        return 0;
    }
    if (variable) {
        symbols_rename(file, sym);
    }
    made_now(file, sym);
    return 1;
}

/* Takes the name base for a global clang makes that merlin has no object for
 * (a constant clang copies a local's initializer from): the counter goes on
 * as clang's. A first pass takes none. */
static void take_name(struct filegen *file, const char *base) {
    if (file->dry) {
        return;
    }
    const char *name = symbols_insert(file, base);
    if (is_c_name(name)) {
        map_put(&file->held, name, (void *)name);
    }
}

/* ---- LLVM's types, which some names depend on ---- */

/* Whether clang gives objects of types a and b one LLVM type in memory. */
static int same_llvm_type(struct type *a, struct type *b) {
    a = ty_unqual(a);
    b = ty_unqual(b);
    if (a->kind == TY_INT && b->kind == TY_INT) {
        return ik_size(a->ik) == ik_size(b->ik);
    }
    if (a->kind == TY_PTR && b->kind == TY_PTR) {
        return 1;
    }
    if (a->kind == TY_ARRAY && b->kind == TY_ARRAY) {
        return (a->has_len ? a->len : 0) == (b->has_len ? b->len : 0) && same_llvm_type(a->to, b->to);
    }
    if (a->kind == TY_RECORD && b->kind == TY_RECORD) {
        return a->rec == b->rec;
    }
    return 0;
}

/* The member whose type a union's LLVM type holds: the most aligned, of
 * those the largest, of those the first (lowerUnion). */
static struct member *union_storage(struct record_body *body) {
    struct member *best = &body->members[0];
    for (uint i = 1; i < body->nmembers; i++) {
        struct member *m = &body->members[i];
        uint ma = ty_align(m->ty);
        uint ba = ty_align(best->ty);
        int64_t ms = ty_size(m->ty);
        int64_t bs = ty_size(best->ty);
        if (ms < 0) {
            ms = 0;
        }
        if (bs < 0) {
            bs = 0;
        }
        if (ma > ba || (ma == ba && ms > bs)) {
            best = m;
        }
    }
    return best;
}

/* Whether a struct has padding. */
static int padded(struct record_body *body) {
    uint end = 0;
    for (uint i = 0; i < body->nmembers; i++) {
        struct member *m = &body->members[i];
        if (m->offset != end) {
            return 1;
        }
        int64_t s = ty_size(m->ty);
        end = m->offset + (uint)(s < 0 ? 0 : s);
    }
    return end != body->size;
}

static int constant_is_null(struct type *ty, struct init *init);

/* Whether the constant clang makes of init, the initializer of an object of
 * type ty, has the LLVM type clang gives such an object in memory (merlin's
 * constant_has_type, whose comment has the rules). */
static int constant_has_type(struct type *ty, struct init *init) {
    struct type *u = ty_unqual(ty);
    if (init->kind == I_LIST && init->items.n == 0) {
        return 1;
    }
    if (u->kind == TY_ARRAY && u->has_len && init->kind == I_LIST) {
        uint nonzero = 0;
        for (uint i = init->items.n; i > 0; i--) {
            if (!constant_is_null(u->to, init->items.v[i - 1])) {
                nonzero = i;
                break;
            }
        }
        if (nonzero == 0) {
            return 1;
        }
        if (u->len - nonzero >= 8) {
            return 0;
        }
        for (uint i = 0; i < init->items.n; i++) {
            if (!constant_has_type(u->to, init->items.v[i])) {
                return 0;
            }
        }
        return 1;
    }
    if (u->kind == TY_RECORD && init->kind == I_LIST) {
        struct record_body *body = u->rec->body;
        if (u->rec->is_union) {
            struct member *held = &body->members[init->items.n - 1];
            struct member *storage = union_storage(body);
            return same_llvm_type(held->ty, storage->ty) &&
                   constant_has_type(held->ty, init->items.v[init->items.n - 1]);
        }
        if (padded(body)) {
            return 0;
        }
        for (uint i = 0; i < body->nmembers && i < init->items.n; i++) {
            if (!constant_has_type(body->members[i].ty, init->items.v[i])) {
                return 0;
            }
        }
        return 1;
    }
    if (init->kind == I_EXPR && init->e->kind == E_COMPOUND) {
        return constant_has_type(init->e->ty, init->e->init);
    }
    return 1;
}

/* Whether the constant clang makes of init is all zeros (isNullValue). */
static int constant_is_null(struct type *ty, struct init *init) {
    struct type *u = ty_unqual(ty);
    if (u->kind == TY_ARRAY && init->kind == I_LIST) {
        for (uint i = 0; i < init->items.n; i++) {
            if (!constant_is_null(u->to, init->items.v[i])) {
                return 0;
            }
        }
        return 1;
    }
    if (u->kind == TY_RECORD && init->kind == I_LIST) {
        /* (A union's: the member it initializes, its last item.) */
        struct record_body *body = u->rec->body;
        uint first = u->rec->is_union ? (init->items.n > 0 ? init->items.n - 1 : 0) : 0;
        for (uint i = first; i < body->nmembers && i < init->items.n; i++) {
            if (!constant_is_null(body->members[i].ty, init->items.v[i])) {
                return 0;
            }
        }
        return 1;
    }
    if (init->kind == I_LIST) {
        return init->items.n == 0;
    }
    struct expr *e = init->e;
    if (u->kind == TY_ARRAY && u->has_len && e->kind == E_STR) {
        for (uint i = 0; i < e->len && i < u->len; i++) {
            if (e->bytes[i] != 0) {
                return 0;
            }
        }
        return 1;
    }
    if (e->kind == E_COMPOUND) {
        return constant_is_null(e->ty, e->init);
    }
    struct cval c;
    if (!consteval_eval(e, &c)) {
        return 0;
    }
    if (!c.is_addr) {
        return i128_is_zero(c.v);
    }
    return c.base == BASE_NONE && i128_is_zero(c.offset);
}

/* The definition of global d (key sym), its initializer's data made: if a
 * reference made its symbol before, with a type whose LLVM type is not the
 * constant's, clang makes a new global of that type and moves the name over
 * to it: the name counter moves on (merlin's defined). */
static void defined(struct filegen *file, struct declarator *d, const char *sym) {
    if (take_over(file, sym, 1)) {
        map_remove(&file->referred_as, sym);
        return;
    }
    struct type *referred = map_get(&file->referred_as, sym);
    if (referred == NULL) {
        return;
    }
    map_remove(&file->referred_as, sym);
    int same = same_llvm_type(referred, d->ty) && (d->init == NULL || constant_has_type(d->ty, d->init));
    if (!same) {
        symbols_rename(file, sym_name(file, sym));
        made_now(file, sym);
    }
}

/* ---- Data ---- */

/* Bytes as data directives: text as .ascii "..." (or .asciz when it ends
 * with the NUL), anything else as .byte lists. Only escapes every assembler
 * reads the same are used. Each directive goes to out as a string. */
static void byte_directives(uchar *bytes, uint n, struct vec *out) {
    uint i = 0;
    while (i < n) {
        uint start = i;
        uint b = bytes[i];
        int printable = (b >= 0x20 && b < 0x7f) || b == '\n' || b == '\t';
        struct buf t;
        memset(&t, 0, sizeof t);
        if (printable) {
            while (i < n && ((bytes[i] >= 0x20 && bytes[i] < 0x7f) || bytes[i] == '\n' || bytes[i] == '\t')) {
                i++;
            }
            for (uint k = start; k < i; k++) {
                uint c = bytes[k];
                if (c == '"') {
                    buf_str(&t, "\\\"");
                } else if (c == '\\') {
                    buf_str(&t, "\\\\");
                } else if (c == '\n') {
                    buf_str(&t, "\\n");
                } else if (c == '\t') {
                    buf_str(&t, "\\t");
                } else {
                    buf_byte(&t, c);
                }
            }
            buf_put(&t, "", 0);
            /* A NUL right after the text: one .asciz says both. */
            if (i + 1 == n && bytes[i] == 0) {
                vec_push(out, fmt(".asciz \"%s\"", t.data));
                i++;
            } else {
                vec_push(out, fmt(".ascii \"%s\"", t.data));
            }
        } else {
            while (i < n && !((bytes[i] >= 0x20 && bytes[i] < 0x7f) || bytes[i] == '\n' || bytes[i] == '\t') &&
                   i - start < 16) {
                i++;
            }
            buf_str(&t, ".byte ");
            for (uint k = start; k < i; k++) {
                if (k > start) {
                    buf_str(&t, ", ");
                }
                buf_fmt(&t, "%u", (uint)bytes[k]);
            }
            vec_push(out, buf_arena(&t));
        }
        buf_free(&t);
    }
}

/* The label of a string literal whose array is `array` (its bytes, the NUL
 * included if it has one): the same arrays are one literal, as clang's are
 * the same constant. A new one is named .str now, .L.str in the assembly. */
static const char *string_array(struct filegen *file, uchar *array, uint n) {
    for (uint i = 0; i < file->strings.n; i++) {
        struct strlit *s = file->strings.v[i];
        if (s->len == n && memcmp(s->bytes, array, n) == 0) {
            return s->label;
        }
    }
    /* A first pass's literals are taken back: they take no name. */
    const char *label = file->dry ? ".L.str" : fmt(".L%s", symbols_insert(file, ".str"));
    struct strlit *s = arena_alloc(sizeof *s);
    s->bytes = arena_alloc(n + 1);
    memcpy(s->bytes, array, n);
    s->len = n;
    s->label = label;
    vec_push(&file->strings, s);
    return label;
}

/* The label of a string literal of bytes (without the NUL). */
static const char *string_label(struct filegen *file, uchar *bytes, uint n) {
    uchar *array = arena_alloc(n + 1);
    memcpy(array, bytes, n);
    array[n] = 0;
    return string_array(file, array, n + 1);
}

/* The type of the first reference to global sym in e, the variable as it was
 * declared there. */
static struct type *referred_type(struct expr *e, const char *sym) {
    struct type *t;
    switch (e->kind) {
    case E_VAR:
        if (e->var.kind == VAR_GLOBAL && strcmp(e->var.sym, sym) == 0) {
            return e->ty;
        }
        return NULL;
    case E_ADDR:
    case E_DEREF:
    case E_UNARY:
    case E_CAST:
    case E_MEMBER:
        return referred_type(e->x, sym);
    case E_BINARY:
    case E_COMMA:
        t = referred_type(e->x, sym);
        return t != NULL ? t : referred_type(e->y, sym);
    case E_COND:
        t = referred_type(e->x, sym);
        if (t == NULL) {
            t = referred_type(e->y, sym);
        }
        return t != NULL ? t : referred_type(e->z, sym);
    default:
        return NULL;
    }
}

/* The directives for an object of type ty initialized by init. */
static void data(struct filegen *file, struct type *ty, struct init *init, struct vec *out) {
    struct type *u = ty_unqual(ty);
    if (init->kind == I_LIST && init->items.n == 0) {
        /* A subobject a designator skipped: zero. */
        vec_push(out, fmt(".zero %u", (uint)ty_size(ty)));
        return;
    }
    if (u->kind == TY_RECORD && init->kind == I_LIST && u->rec->is_union) {
        /* The member the union is initialized with is the last item. */
        struct record_body *body = u->rec->body;
        uint k = init->items.n - 1;
        struct member *m = &body->members[k];
        data(file, m->ty, init->items.v[k], out);
        uint size = (uint)ty_size(m->ty);
        if (body->size > size) {
            vec_push(out, fmt(".zero %u", body->size - size));
        }
        return;
    }
    if (u->kind == TY_RECORD && init->kind == I_LIST) {
        /* Members in order, with the padding between them and after the last
         * as zeros. */
        struct record_body *body = u->rec->body;
        uint at = 0;
        for (uint i = 0; i < body->nmembers && i < init->items.n; i++) {
            struct member *m = &body->members[i];
            if (m->offset > at) {
                vec_push(out, fmt(".zero %u", m->offset - at));
            }
            data(file, m->ty, init->items.v[i], out);
            at = m->offset + (uint)ty_size(m->ty);
        }
        if (body->size > at) {
            vec_push(out, fmt(".zero %u", body->size - at));
        }
        return;
    }
    if (u->kind == TY_ARRAY && u->has_len && init->kind == I_LIST) {
        for (uint i = 0; i < init->items.n; i++) {
            data(file, u->to, init->items.v[i], out);
        }
        uint rest = (u->len - init->items.n) * (uint)ty_size(u->to);
        if (rest > 0) {
            vec_push(out, fmt(".zero %u", rest));
        }
        return;
    }
    struct expr *e = init->e;
    if (u->kind == TY_ARRAY && u->has_len && e->kind == E_STR) {
        /* The text, cut to the array, then zeros. */
        uint take = e->len < u->len ? e->len : u->len;
        byte_directives(e->bytes, take, out);
        if (u->len > take) {
            vec_push(out, fmt(".zero %u", u->len - take));
        }
        return;
    }
    struct cval value;
    if (!consteval_eval(e, &value)) {
        die("codegen: a static initializer is not constant");
    }
    /* .quad for 8 bytes, as clang writes it. */
    int64_t size = ty_size(ty);
    const char *directive = size == 1 ? ".byte" : size == 2 ? ".half" : size == 4 ? ".word" : ".quad";
    /* The value as the directive's bytes hold it: an unsigned 0xffffffff is
     * 4294967295, a negative address offset wraps. */
    char *operand;
    if (!value.is_addr) {
        operand = i128_str(value.v);
    } else if (value.base == BASE_NONE) {
        operand = i128_str(ik_wrap(IK_ULONG, value.offset));
    } else {
        const char *sym;
        if (value.base == BASE_SYM) {
            reference(file, value.sym, referred_type(e, value.sym));
            sym = sym_name(file, value.sym);
        } else {
            sym = string_label(file, value.bytes, value.len);
        }
        if (i128_is_zero(value.offset)) {
            operand = (char *)sym;
        } else if (!i128_neg_p(value.offset)) {
            operand = fmt("%s+%s", sym, i128_str(value.offset));
        } else {
            operand = fmt("%s%s", sym, i128_str(value.offset));
        }
    }
    vec_push(out, fmt("%s %s", directive, operand));
}

/* Starts the output of a symbol. */
static void begin_symbol(struct filegen *file) {
    file->out.len = 0;
    file->section = "";
    file->pieces.n = 0;
}

/* Goes on in section. */
static void piece(struct filegen *file, const char *section) {
    if (strcmp(file->section, section) != 0) {
        if (file->out.len > 0) {
            struct piece *p = arena_alloc(sizeof *p);
            p->section = file->section;
            p->text = arena_strndup(file->out.data, file->out.len);
            p->len = file->out.len;
            vec_push(&file->pieces, p);
            file->out.len = 0;
        }
        file->section = section;
    }
}

/* Ends the output of symbol sym. */
static void finish_symbol(struct filegen *file, const char *sym) {
    struct piece *p = arena_alloc(sizeof *p);
    p->section = file->section;
    p->text = arena_strndup(file->out.len > 0 ? file->out.data : "", file->out.len);
    p->len = file->out.len;
    vec_push(&file->pieces, p);
    file->out.len = 0;
    struct vec *pieces = arena_alloc(sizeof *pieces);
    *pieces = vec_take(&file->pieces);
    map_put(&file->chunks, sym, pieces);
}

/* Emits a global variable or, when local, a static local: its symbol and
 * its data. clang makes a global's symbol after its initializer, unless
 * something referred to it before, and a static local's before. */
static void variable(struct filegen *file, struct declarator *d, int local) {
    const char *sym = d->var.sym;
    if (local) {
        if (file->dry) {
            return;
        }
        create(file, sym);
    }
    struct vec lines;
    memset(&lines, 0, sizeof lines);
    if (d->init != NULL) {
        data(file, d->ty, d->init, &lines);
    }
    defined(file, d, sym);
    create(file, sym);
    const char *name = sym_name(file, sym);
    begin_symbol(file);
    /* A const object never changes: read-only data, as clang does (a const
     * volatile one may be changed from outside). */
    uint quals = ty_quals(d->ty);
    if ((quals & Q_CONST) && !(quals & Q_VOLATILE)) {
        piece(file, ".rodata");
    } else if (d->init != NULL) {
        piece(file, ".data");
    } else {
        piece(file, ".bss");
    }
    uint size = (uint)ty_size(d->ty);
    const char *storage = d->storage == ST_STATIC ? "static " : "";
    /* A compound literal has no name in C: its symbol's. */
    const char *shown = strcmp(d->name, ".compoundliteral") == 0 ? name : d->name;
    buf_fmt(&file->out, "\n    # %s%s: %u %s\n", storage, ty_declare(d->ty, shown), size, size == 1 ? "byte" : "bytes");
    if (d->storage != ST_STATIC) {
        buf_fmt(&file->out, "    .globl %s\n", name);
    }
    if (ty_align(d->ty) > 1) {
        buf_fmt(&file->out, "    .balign %u\n", ty_align(d->ty));
    }
    buf_fmt(&file->out, "    .type %s, @object\n", name);
    buf_fmt(&file->out, "    .size %s, %u\n", name, size);
    buf_fmt(&file->out, "%s:\n", name);
    if (d->init == NULL) {
        vec_push(&lines, fmt(".zero %u", size));
    }
    for (uint i = 0; i < lines.n; i++) {
        buf_fmt(&file->out, "    %s\n", (char *)lines.v[i]);
    }
    heap_free(lines.v);
    finish_symbol(file, sym);
}

/* Emits every string literal, NUL-terminated, in .rodata. */
static void strings(struct filegen *file) {
    if (file->strings.n == 0) {
        return;
    }
    switch_section(file, ".rodata");
    for (uint i = 0; i < file->strings.n; i++) {
        struct strlit *s = file->strings.v[i];
        buf_fmt(&file->out, "\n%s:\n", s->label);
        struct vec lines;
        memset(&lines, 0, sizeof lines);
        byte_directives(s->bytes, s->len, &lines);
        for (uint k = 0; k < lines.n; k++) {
            buf_fmt(&file->out, "    %s\n", (char *)lines.v[k]);
        }
        heap_free(lines.v);
    }
    file->strings.n = 0;
}

/* ---- The frame ---- */

/* Places every local the code uses in the frame, below the header bytes at
 * its top: its offset from s0 (0 for one not used), and the frame size (a
 * multiple of 16). 0 if the frame would be larger than MAX_FRAME. */
static int layout(struct vec *locals, uchar *used, int header, int **offsets, int *frame) {
    int64_t off = -(int64_t)header;
    int *offs = arena_alloc(locals->n * sizeof(int) + 1);
    for (uint i = 0; i < locals->n; i++) {
        struct local *l = locals->v[i];
        if (!used[i]) {
            offs[i] = 0;
            continue;
        }
        uint size = (uint)ty_size(l->ty);
        /* Arrays and structs are aligned to a register so that clearing them
         * can use sd, and a struct's slot is a whole number of registers. */
        uint w = WORD;
        uint align = ty_align(l->ty);
        if ((ty_is_array(l->ty) || ty_record(l->ty) != NULL) && align < w) {
            align = w;
        }
        if (ty_record(l->ty) != NULL) {
            size = (size + w - 1) / w * w;
        }
        /* Round down to the alignment (two's complement: below zero too). */
        off = (off - (int64_t)size) & ~((int64_t)align - 1);
        if (((-off + 15) & ~15ll) > MAX_FRAME) {
            return 0;
        }
        offs[i] = (int)off;
    }
    *offsets = offs;
    *frame = (int)((-off + 15) & ~15ll);
    return 1;
}

/* Does v fit a 12-bit signed immediate (addi, lw, sw)? */
static int fits_imm12(int64_t v) {
    return v >= -2048 && v <= 2047;
}

/* The load that brings a value of type ty into a register, widened. */
static const char *load_op(struct type *ty) {
    struct type *u = ty_unqual(ty);
    if (u->kind == TY_PTR) {
        return "ld";
    }
    if (u->kind != TY_INT) {
        die("codegen: a value that is not loaded");
    }
    /* _Bool holds 0 or 1: lbu. An int or unsigned int is loaded
     * sign-extended (the 32-bit invariant). */
    uint size = ik_size(u->ik);
    int s = ik_signed(u->ik);
    if (size == 1) {
        return s ? "lb" : "lbu";
    }
    if (size == 2) {
        return s ? "lh" : "lhu";
    }
    return size == 4 ? "lw" : "ld";
}

/* The store that writes a value of type ty (its low bytes) to memory. */
static const char *store_op(struct type *ty) {
    switch (ty_size(ty)) {
    case 1:
        return "sb";
    case 2:
        return "sh";
    case 4:
        return "sw";
    case 8:
        return "sd";
    default:
        die("codegen: a value that is not stored");
        return "";
    }
}

/* log2(n) if n is a power of two, else -1. */
static int log2_of(uint n) {
    if (n == 0 || (n & (n - 1)) != 0) {
        return -1;
    }
    int k = 0;
    while ((n >> k) != 1) {
        k++;
    }
    return k;
}

/* Whether s is visited even where control cannot reach it: clang's
 * EmitSimpleStmt statements. */
static int always_visited(struct stmt *s) {
    switch (s->kind) {
    case S_BLOCK:
    case S_DECL:
    case S_EMPTY:
    case S_UNEVALUATED:
    case S_LABEL:
    case S_GOTO:
    case S_BREAK:
    case S_CONTINUE:
    case S_CASE:
    case S_DEFAULT:
        return 1;
    default:
        return 0;
    }
}

/* Whether cast e makes a null pointer constant a pointer as clang's
 * CK_NullToPointer does, whose operand clang neither emits nor visits: an
 * integer one, or (void *)0 converted where C converts as by assignment. */
static int null_to_pointer(struct expr *e) {
    if (e->kind != E_CAST) {
        return 0;
    }
    return ty_is_ptr(e->ty) && is_null_pointer(e->x) && (ty_is_integer(e->x->ty) || e->implicit);
}

/* What clang's emitStoresForConstant looks at in the constant initializer
 * of a local array, struct or union (merlin's ConstantImage): each byte (or
 * -1 for an address's), how many scalars are not zero, and whether one is
 * a symbol's own address. */
struct image {
    int *bytes;
    uint size;
    uint nonzero;
    int symbol;
};

/* Whether clang copies the object from a constant global at -O0 (merlin's
 * ConstantImage::copied). */
static int image_copied(struct image *im) {
    int all_zero = 1;
    for (uint i = 0; i < im->size; i++) {
        if (im->bytes[i] != 0) {
            all_zero = 0;
        }
    }
    if (all_zero) {
        return 0;
    }
    if (im->size <= 32) {
        return 1;
    }
    int few_stores = !im->symbol && im->nonzero <= 6;
    int one_byte = im->bytes[0] >= 0;
    for (uint i = 0; i < im->size && one_byte; i++) {
        if (im->bytes[i] != im->bytes[0]) {
            one_byte = 0;
        }
    }
    return !few_stores && !one_byte;
}

/* A scalar's little-endian bytes at byte at. */
static void image_scalar(struct image *im, uint at, i128 v, uint size, int nonzero) {
    for (uint i = 0; i < size; i++) {
        im->bytes[at + i] = (int)(i < 8 ? (v.lo >> (8 * i)) & 0xff : (v.hi >> (8 * (i - 8))) & 0xff);
    }
    im->nonzero += (uint)nonzero;
}

/* An address of size bytes at byte at; symbol: a symbol's own. */
static void image_address(struct image *im, uint at, uint size, int symbol) {
    for (uint i = 0; i < size; i++) {
        im->bytes[at + i] = -1;
    }
    im->nonzero++;
    im->symbol |= symbol;
}

/* Where the target of an assignment is: a scalar local at its offset from
 * s0, or anywhere else, its address the top temporary on the stack. */
struct place {
    int pushed;
    int off;
};

/* Per-function code generator state (merlin's FuncGen). */
struct funcgen {
    struct function *func;
    /* Offset from s0 of each local. */
    int *offsets;
    int frame;
    /* How many argument words the named parameters take. */
    uint named_words;
    /* In a variadic function, the bytes of argument registers saved at the
     * top of the frame; else 0. */
    int va_save;
    /* struct line. */
    struct line *lines;
    uint nlines;
    uint caplines;
    /* How many temporaries expression evaluation has pushed right now. */
    uint depth;
    /* Where break goes for each enclosing loop or switch, and continue for
     * each loop, innermost last (char *). */
    struct vec breaks;
    struct vec continues;
    struct filegen *file;
    /* Every return jumps here, to the epilogue. */
    const char *ret_label;
    /* The ids of the cases and defaults of each switch on a constant
     * generated so far, whose labels nothing jumps to (as id + 1). */
    struct vec elided_cases;
    /* A local's constant initializer is being generated. */
    int constant_init;
    /* The locals the code generated so far declares or uses. */
    uchar *used;
    /* The locals the frame has slots for. */
    uchar *slots;
    /* Whether control can reach the code being generated: clang's insertion
     * point. */
    int reachable;
    /* The labels the jumps and branches generated so far go to. */
    struct map reached;
};

/* ---- Emitting lines ---- */

static struct line *new_line(struct funcgen *g, int kind) {
    g->lines = grow(g->lines, &g->caplines, g->nlines + 1, sizeof(struct line));
    struct line *l = &g->lines[g->nlines++];
    memset(l, 0, sizeof *l);
    l->kind = kind;
    return l;
}

/* `mnemonic operands`, the mnemonic padded to 5 (merlin's insn_text). */
static char *insn_text(const char *mnemonic, const char *operands) {
    if (operands[0] == 0) {
        return (char *)mnemonic;
    }
    struct buf b;
    memset(&b, 0, sizeof b);
    buf_str(&b, mnemonic);
    for (uint n = (uint)strlen(mnemonic); n < 5; n++) {
        buf_byte(&b, ' ');
    }
    buf_byte(&b, ' ');
    buf_str(&b, operands);
    char *r = buf_arena(&b);
    buf_free(&b);
    return r;
}

static void ins_sized(struct funcgen *g, uint size, const char *mnemonic, const char *operands) {
    struct line *l = new_line(g, LINE_INSN);
    l->text = insn_text(mnemonic, operands);
    l->size = size;
}

/* A one-word instruction: mnemonic operands. */
static void ins(struct funcgen *g, const char *mnemonic, const char *operands) {
    ins_sized(g, 4, mnemonic, operands);
}

/* li reg, value: one instruction for small values, two for others that fit
 * 32 bits, up to 8 for a 64-bit value (relax_branches counts that many). */
static void li(struct funcgen *g, const char *reg, int64_t value) {
    uint size = fits_imm12(value) ? 4 : (value >= -2147483647ll - 1 && value <= 2147483647ll) ? 8 : 32;
    ins_sized(g, size, "li", fmt("%s, %lld", reg, value));
}

/* la reg, label, of a label that is no symbol of the module's. */
static void la_label(struct funcgen *g, const char *reg, const char *label) {
    ins_sized(g, 8, "la", fmt("%s, %s", reg, label));
}

/* la reg, symbol: for the symbol of key symbol (declared there with type
 * ty). */
static void la(struct funcgen *g, const char *reg, const char *symbol, struct type *ty) {
    reference(g->file, symbol, ty);
    la_label(g, reg, sym_name(g->file, symbol));
}

static void comment(struct funcgen *g, const char *text) {
    new_line(g, LINE_TEXT)->text = fmt("# %s", text);
}

static void text_line(struct funcgen *g, const char *text) {
    new_line(g, LINE_TEXT)->text = (char *)text;
}

static void label(struct funcgen *g, const char *name) {
    new_line(g, LINE_LABEL)->text = (char *)name;
}

static void branch(struct funcgen *g, const char *op, const char *regs, const char *target) {
    map_put(&g->reached, target, (void *)target);
    struct line *l = new_line(g, LINE_BRANCH);
    l->op = op;
    l->regs = regs;
    l->text = (char *)target;
}

static void branch_if_zero(struct funcgen *g, const char *target) {
    branch(g, "beqz", "a0", target);
}

static void branch_if_nonzero(struct funcgen *g, const char *target) {
    branch(g, "bnez", "a0", target);
}

/* A statement's jump to target, as clang's EmitBranch: generated only from
 * code control reaches, and nothing after it is reached until a label is. */
static void jump(struct funcgen *g, const char *target) {
    if (g->reachable) {
        map_put(&g->reached, target, (void *)target);
        ins(g, "j", target);
    }
    g->reachable = 0;
}

/* A label code is generated after whether or not anything reaches it:
 * clang's EmitBlock(block), which makes the block the insertion point. */
static void block_label(struct funcgen *g, const char *name) {
    label(g, name);
    g->reachable = 1;
}

/* The label where the paths of a statement meet: clang's EmitBlock(block,
 * IsFinished), which deletes a block nothing falls or jumps into. */
static void join(struct funcgen *g, const char *name) {
    if (map_get(&g->reached, name) != NULL) {
        g->reachable = 1;
    }
    if (g->reachable) {
        label(g, name);
    }
}

/* A fresh label name, .L.<what>.<n>. */
static const char *new_label(struct funcgen *g, const char *what) {
    g->file->labels++;
    return fmt(".L.%s.%u", what, g->file->labels);
}

/* rd = rs + imm for any imm; large ones go through t0. */
static void add_imm(struct funcgen *g, const char *rd, const char *rs, int imm) {
    if (fits_imm12(imm)) {
        ins(g, "addi", fmt("%s, %s, %d", rd, rs, imm));
    } else {
        li(g, "t0", imm);
        ins(g, "add", fmt("%s, %s, t0", rd, rs));
    }
}

/* A load or store op of reg at off(s0); offsets beyond 12 bits compute the
 * address in t0 first. */
static void access_local(struct funcgen *g, const char *op, const char *reg, int offset) {
    if (fits_imm12(offset)) {
        ins(g, op, fmt("%s, %d(s0)", reg, offset));
    } else {
        add_imm(g, "t0", "s0", offset);
        ins(g, op, fmt("%s, 0(t0)", reg));
    }
}

static int offset_of(struct funcgen *g, uint id) {
    g->used[id] = 1;
    return g->offsets[id];
}

/* How a register holds the value v of type k: its bits, widened as convert
 * keeps values (a 32-bit type sign-extended, unsigned int too). */
static int64_t image_of(i128 v, int k) {
    v = ik_wrap(k, v);
    if (ik_size(k) == 4) {
        return (int64_t)(int)(uint)v.lo;
    }
    return (int64_t)v.lo;
}

/* a0 = the constant v of type k. */
static void constant(struct funcgen *g, i128 v, int k) {
    li(g, "a0", image_of(v, k));
}

/* Whether arithmetic in the promoted type ty is 32-bit arithmetic on 64-bit
 * registers: an int or unsigned int, which the W instructions do. */
static int is_w(struct type *ty) {
    int k = ty_int_kind(ty);
    return k >= 0 && ik_size(k) == 4;
}

/* ---- The stack of temporaries ---- */

/* Pushes a0: a whole register. */
static void push_a0(struct funcgen *g) {
    ins(g, "addi", fmt("sp, sp, %d", -WORD));
    ins(g, "sd", "a0, 0(sp)");
    g->depth++;
}

/* Pops the top temporary into reg. */
static void pop(struct funcgen *g, const char *reg) {
    ins(g, "ld", fmt("%s, 0(sp)", reg));
    ins(g, "addi", fmt("sp, sp, %d", WORD));
    g->depth--;
}

/* Where word n of a call's arguments is: a register (*reg), or an offset
 * from the sp of the call (the callee's s0); whether it is a register. */
static int arg_word(uint n, const char **reg, int *at) {
    if (n < ARG_REGS) {
        *reg = fmt("a%u", n);
        return 1;
    }
    *at = WORD * (int)(n - ARG_REGS);
    return 0;
}

/* ---- Functions ---- */

static void stmt(struct funcgen *g, struct stmt *s);
static void expr(struct funcgen *g, struct expr *e);
static void copy(struct funcgen *g, const char *dst, const char *src, struct type *ty);
static void load_words(struct funcgen *g, const char *base, int off, uint n, struct type *ty);

/* The top of the frame, which the prologue sets up first: ra, s0 and a
 * variadic function's saved registers, rounded up to 16 bytes. */
static int frame_top(struct funcgen *g) {
    return (SAVED_REGS + g->va_save + 15) & ~15;
}

/* In the prologue: the register holding argument word `word`. A word on
 * the stack is loaded into t1 first. */
static const char *incoming(struct funcgen *g, uint word) {
    const char *reg;
    int at;
    if (arg_word(word, &reg, &at)) {
        return reg;
    }
    ins(g, "ld", fmt("t1, %d(s0)", at));
    return "t1";
}

/* A number right-aligned in a field of width (Rust's {:>width}). */
static char *right_aligned(int v, uint width) {
    char *s = fmt("%d", v);
    uint n = (uint)strlen(s);
    if (n >= width) {
        return s;
    }
    struct buf b;
    memset(&b, 0, sizeof b);
    for (uint i = n; i < width; i++) {
        buf_byte(&b, ' ');
    }
    buf_str(&b, s);
    char *r = buf_arena(&b);
    buf_free(&b);
    return r;
}

/* Whether line l is the instruction `j label`. */
static int is_jump_to(struct line *l, const char *target) {
    return l->kind == LINE_INSN && l->size == 4 && strcmp(l->text, insn_text("j", target)) == 0;
}

static void function_body(struct funcgen *g, struct vec *body) {
    struct function *f = g->func;
    /* A header that maps every local to its slot, for whoever reads the
     * output. */
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
    const char *params = p.len == 0 ? "void" : buf_arena(&p);
    buf_free(&p);
    int frame = g->frame;
    comment(g, fmt("%s: frame %d bytes", ty_declare(f->ret, fmt("%s(%s)", f->name, params)), frame));
    if (g->va_save > 0) {
        const char *regs = g->named_words == 7 ? "a7" : fmt("a%u-a7", g->named_words);
        comment(g, fmt("    %d(s0)  %s, saved for va_arg", -g->va_save, regs));
    }
    for (uint i = 0; i < f->locals.n; i++) {
        struct local *l = f->locals.v[i];
        /* (The frame has no slot for a local the code does not use.) */
        if (!g->slots[i]) {
            continue;
        }
        const char *what;
        if (i < f->nparams) {
            what = fmt("param %s", ty_declare(l->ty, l->name));
        } else if (l->temp) {
            /* A hidden local for a call or a return. */
            what = fmt("temp %s (%s)", ty_str(l->ty), l->name);
        } else {
            what = fmt("local %s", ty_declare(l->ty, l->name));
        }
        comment(g, fmt("  %s(s0)  %s", right_aligned(g->offsets[i], 5), what));
    }
    /* An inline definition provides no external definition: kept local. */
    if (!f->is_static && !f->inline_only) {
        text_line(g, fmt(".globl %s", f->name));
    }
    /* A function symbol with its size, as clang marks them. */
    text_line(g, fmt(".type %s, @function", f->name));
    label(g, f->name);

    /* Prologue: save ra and the caller's s0, set up s0, make room for
     * locals. */
    int top = frame_top(g);
    int va_save = g->va_save;
    int w = WORD;
    add_imm(g, "sp", "sp", -top);
    ins(g, "sd", fmt("ra, %d(sp)", top - va_save - w));
    ins(g, "sd", fmt("s0, %d(sp)", top - va_save - 2 * w));
    add_imm(g, "s0", "sp", top);
    if (frame > top) {
        add_imm(g, "sp", "sp", -(frame - top));
    }
    /* A variadic function stores the argument registers its named
     * parameters leave free right below s0, where the caller's stack words
     * continue them. */
    if (f->variadic && g->named_words < ARG_REGS) {
        comment(g, "the variadic arguments in registers, next to those on the stack");
        for (uint r = g->named_words; r < ARG_REGS; r++) {
            int off = -w * (int)(ARG_REGS - r);
            ins(g, "sd", fmt("a%u, %d(s0)", r, off));
        }
    }
    /* Parameters arrive in a0-a7 and then on the stack; give them their
     * slots like any local. A big struct result's address comes first. */
    struct type *ft = function_ty(f);
    if (f->ret_ptr >= 0) {
        access_local(g, "sd", "a0", offset_of(g, (uint)f->ret_ptr));
    }
    uint *words = arena_alloc((f->nparams + 1) * sizeof(uint));
    int *passes = arena_alloc((f->nparams + 1) * sizeof(int));
    abi_call_arguments(ft->fn->ret, ft->fn->params, ft->fn->nparams, words, passes);
    for (uint i = 0; i < f->nparams; i++) {
        struct param *pr = &f->params[i];
        int off = g->offsets[i];
        uint word = words[i];
        if (passes[i] == PASS_SCALAR) {
            access_local(g, store_op(pr->ty), incoming(g, word), off);
        } else if (passes[i] != PASS_REF) {
            /* A small struct: its words, as they lie in memory. */
            for (uint k = 0; k < abi_registers(passes[i]); k++) {
                access_local(g, "sd", incoming(g, word + k), off + w * (int)k);
            }
        } else {
            /* A big struct: the caller's copy is copied into the slot. */
            const char *src;
            int at;
            if (!arg_word(word, &src, &at)) {
                /* Every register argument is in its slot by now, so a0 is
                 * free. */
                ins(g, "ld", fmt("a0, %d(s0)", at));
                src = "a0";
            }
            add_imm(g, "t4", "s0", off);
            copy(g, "t4", src, pr->ty);
        }
    }

    for (uint i = 0; i < body->n; i++) {
        stmt(g, body->v[i]);
    }

    /* Falling off the end returns 0. A final return needs no jump to the
     * epilogue, which comes right after it. */
    if (g->nlines > 0 && is_jump_to(&g->lines[g->nlines - 1], g->ret_label)) {
        g->nlines--;
    }
    /* (Only if control reaches the end.) */
    if (g->reachable) {
        li(g, "a0", 0);
    }

    /* Epilogue: the result is in a0. Free the frame from s0, restore ra and
     * s0, return. */
    label(g, g->ret_label);
    ins(g, "addi", fmt("sp, s0, %d", -top));
    ins(g, "ld", fmt("ra, %d(sp)", top - va_save - w));
    ins(g, "ld", fmt("s0, %d(sp)", top - va_save - 2 * w));
    ins(g, "addi", fmt("sp, sp, %d", top));
    ins(g, "ret", "");
    text_line(g, fmt(".size %s, . - %s", f->name, f->name));
}

/* ---- Statements ---- */

/* The value of a condition clang's code generator decides at compile time:
 * -1 if it does not, else 0 or 1. */
static int constant_condition(struct expr *e) {
    i128 v;
    return consteval_condition(e, &v) ? !i128_is_zero(v) : -1;
}

static void loop_body(struct funcgen *g, struct stmt *body, const char *cont, const char *brk) {
    vec_push(&g->continues, (void *)cont);
    vec_push(&g->breaks, (void *)brk);
    stmt(g, body);
    vec_pop(&g->continues);
    vec_pop(&g->breaks);
}

/* The label of case or default number id (numbered by sema, per
 * function). */
static const char *case_label(struct funcgen *g, uint id) {
    return fmt(".L.case.%s.%u", g->func->name, id);
}

/* The label of the C label name. */
static const char *goto_label(struct funcgen *g, const char *name) {
    return fmt(".L.label.%s.%s", g->func->name, name);
}

/* Returns the struct whose address is in a0 from the function: in a0 (and
 * a1) when it is small, otherwise copied to where the caller asked. */
static void return_record(struct funcgen *g, struct type *ty) {
    if (g->func->ret_ptr >= 0) {
        access_local(g, "ld", "a1", offset_of(g, (uint)g->func->ret_ptr));
        copy(g, "a1", "a0", ty);
        return;
    }
    ins(g, "mv", "t1, a0");
    load_words(g, "t1", 0, abi_registers(abi_classify(ty)), ty);
}

static void local_decl(struct funcgen *g, struct declarator *d);

static int elided(struct funcgen *g, uint id) {
    for (uint i = 0; i < g->elided_cases.n; i++) {
        if ((uintptr_t)g->elided_cases.v[i] == (uintptr_t)id + 1) {
            return 1;
        }
    }
    return 0;
}

static void unscoped_stmt(struct funcgen *g, struct stmt *s) {
    switch (s->kind) {
    case S_BLOCK:
        for (uint i = 0; i < s->list.n; i++) {
            stmt(g, s->list.v[i]);
        }
        return;
    case S_DECL:
        /* Each declarator initialized, as clang does; a static local's data
         * first. */
        for (uint i = 0; i < s->list.n; i++) {
            struct declarator *d = s->list.v[i];
            /* A block's extern declaration, or a function's, defines
             * nothing. */
            if (d->storage == ST_EXTERN || ty_is_func(d->ty)) {
                continue;
            }
            /* A local the code declares has a slot, used or not. */
            if (d->var.kind == VAR_LOCAL) {
                g->used[d->var.local] = 1;
            }
            if (d->var.kind == VAR_GLOBAL) {
                local_decl(g, d);
                continue;
            }
            /* A local declared where control cannot reach has its slot, but
             * no initialization. */
            if (!g->reachable) {
                continue;
            }
            local_decl(g, d);
        }
        return;
    case S_EXPR:
        expr(g, s->e);
        return;
    case S_EMPTY:
    case S_UNEVALUATED:
        /* Only checked: nothing is evaluated. */
        return;
    case S_IF: {
        /* A constant condition runs one arm: only that one is generated,
         * unless the other has a label. */
        int c = constant_condition(s->e);
        if (c >= 0) {
            struct stmt *run = c ? s->body : s->els;
            struct stmt *skipped = c ? s->els : s->body;
            if (!(skipped != NULL && contains_label(skipped, 0))) {
                if (run != NULL) {
                    stmt(g, run);
                }
                return;
            }
        }
        expr(g, s->e);
        if (s->els == NULL) {
            const char *end = new_label(g, "end");
            branch_if_zero(g, end);
            stmt(g, s->body);
            join(g, end);
        } else {
            const char *els = new_label(g, "else");
            const char *end = new_label(g, "end");
            branch_if_zero(g, els);
            stmt(g, s->body);
            jump(g, end);
            block_label(g, els);
            stmt(g, s->els);
            join(g, end);
        }
        return;
    }
    case S_WHILE: {
        const char *cont = new_label(g, "continue");
        const char *brk = new_label(g, "break");
        block_label(g, cont);
        if (constant_condition(s->e) != 1) {
            /* (`while (1)`: nothing to test, as clang has it.) */
            expr(g, s->e);
            branch_if_zero(g, brk);
        }
        loop_body(g, s->body, cont, brk);
        jump(g, cont);
        join(g, brk);
        return;
    }
    case S_DOWHILE: {
        /* The condition is generated even when neither the body's end nor a
         * continue reaches it, as clang emits its block. */
        const char *top = new_label(g, "do");
        const char *cont = new_label(g, "continue");
        const char *brk = new_label(g, "break");
        block_label(g, top);
        loop_body(g, s->body, cont, brk);
        block_label(g, cont);
        if (constant_condition(s->e) != 0) {
            /* (`do ... while (0)`: nothing to test.) */
            expr(g, s->e);
            branch_if_nonzero(g, top);
        }
        join(g, brk);
        return;
    }
    case S_FOR: {
        if (s->init != NULL) {
            stmt(g, s->init);
        }
        /* The step is generated even when neither the body's end nor a
         * continue reaches it, as clang emits its block. */
        const char *top = new_label(g, "loop");
        const char *cont = new_label(g, "continue");
        const char *brk = new_label(g, "break");
        block_label(g, top);
        if (s->e != NULL) {
            expr(g, s->e);
            branch_if_zero(g, brk);
        }
        loop_body(g, s->body, cont, brk);
        if (s->step != NULL) {
            block_label(g, cont);
            expr(g, s->step);
        } else {
            join(g, cont);
        }
        jump(g, top);
        join(g, brk);
        return;
    }
    case S_RETURN:
        if (s->e != NULL) {
            expr(g, s->e);
            if (ty_record(s->e->ty) != NULL) {
                return_record(g, s->e->ty);
            }
        }
        jump(g, g->ret_label);
        return;
    case S_BREAK:
        jump(g, vec_last(&g->breaks));
        return;
    case S_CONTINUE:
        jump(g, vec_last(&g->continues));
        return;
    case S_SWITCH: {
        /* On a constant condition, only the statements that run, when they
         * can be picked out: no compares, no labels, no break. */
        i128 v;
        if (consteval_condition(s->e, &v)) {
            struct vec run;
            if (case_statements(s, v, &run)) {
                for (uint i = 0; i < s->ncases; i++) {
                    vec_push(&g->elided_cases, (void *)((uintptr_t)s->cases[i].id + 1));
                }
                if (s->id >= 0) {
                    vec_push(&g->elided_cases, (void *)((uintptr_t)s->id + 1));
                }
                for (uint i = 0; i < run.n; i++) {
                    stmt(g, run.v[i]);
                }
                heap_free(run.v);
                return;
            }
        }
        const char *brk = new_label(g, "break");
        expr(g, s->e);
        int kind = ty_int_kind(s->e->ty);
        for (uint i = 0; i < s->ncases; i++) {
            /* The value as the register holds it: an unsigned 0xffffffff is
             * li t0, -1. */
            const char *target = case_label(g, s->cases[i].id);
            li(g, "t0", image_of(s->cases[i].value, kind));
            branch(g, "beq", "a0, t0", target);
        }
        const char *otherwise = s->id >= 0 ? case_label(g, (uint)s->id) : brk;
        /* Control enters the body only at its labels. */
        jump(g, otherwise);
        vec_push(&g->breaks, (void *)brk);
        stmt(g, s->body);
        vec_pop(&g->breaks);
        join(g, brk);
        return;
    }
    case S_CASE:
    case S_DEFAULT:
        /* (Nothing jumps to the label of a switch on a constant.) */
        if (!elided(g, (uint)s->id)) {
            block_label(g, case_label(g, (uint)s->id));
        }
        stmt(g, s->body);
        return;
    case S_LABEL:
        block_label(g, goto_label(g, s->name));
        stmt(g, s->body);
        return;
    default:
        jump(g, goto_label(g, s->name));
        return;
    }
}

/* A statement. Code control cannot reach is not generated, as clang's
 * EmitStmt leaves it out: a statement but those it always visits, unless a
 * label in it can be jumped to, which makes it reachable from its start. */
static void stmt(struct funcgen *g, struct stmt *s) {
    if (!g->reachable && !always_visited(s)) {
        if (!contains_label(s, 0)) {
            return;
        }
        g->reachable = 1;
    }
    unscoped_stmt(g, s);
}

/* ---- Initializers ---- */

static void store_local(struct funcgen *g, struct type *ty, int off);
static void clear_local(struct funcgen *g, int off, int size);

/* The bytes and scalars of init, a constant initializer of an object of
 * type ty at byte at of the image, as clang's constant has them. */
static void constant_image(struct type *ty, struct init *init, uint at, struct image *im) {
    struct type *u = ty_unqual(ty);
    if (u->kind == TY_ARRAY && init->kind == I_LIST) {
        uint size = (uint)ty_size(u->to);
        for (uint i = 0; i < init->items.n; i++) {
            constant_image(u->to, init->items.v[i], at + i * size, im);
        }
        return;
    }
    if (u->kind == TY_RECORD && init->kind == I_LIST) {
        /* A union's list has empty items before the member it holds. */
        struct record_body *body = u->rec->body;
        for (uint i = 0; i < body->nmembers && i < init->items.n; i++) {
            constant_image(body->members[i].ty, init->items.v[i], at + body->members[i].offset, im);
        }
        return;
    }
    if (init->kind == I_LIST) {
        for (uint i = 0; i < init->items.n; i++) {
            constant_image(ty, init->items.v[i], at, im);
        }
        return;
    }
    struct expr *e = init->e;
    if (u->kind == TY_ARRAY && u->has_len && e->kind == E_STR) {
        /* Each char a scalar of its own. */
        for (uint i = 0; i < e->len && i < u->len; i++) {
            image_scalar(im, at + i, i128_of(e->bytes[i]), 1, e->bytes[i] != 0);
        }
        return;
    }
    if (e->kind == E_COMPOUND) {
        constant_image(e->ty, e->init, at, im);
        return;
    }
    uint size = (uint)ty_size(ty);
    struct cval c;
    consteval_eval(e, &c);
    if (!c.is_addr) {
        image_scalar(im, at, c.v, size, !i128_is_zero(c.v));
    } else if (c.base == BASE_NONE) {
        image_scalar(im, at, c.offset, size, !i128_is_zero(c.offset));
    } else {
        /* An address: no bytes are known. */
        image_address(im, at, size, ty_is_ptr(ty) && i128_is_zero(c.offset));
    }
}

/* Whether init is a constant initializer of an object of type ty, as
 * clang's Expr::isConstantInitializer has it. visit: clang's ConstantEmitter
 * trying it (merlin's constant_visit), which stops at the first element
 * that is not constant, keeping casts' types with -g (the next stage). */
static int constant_initializer(struct type *ty, struct init *init) {
    struct type *u = ty_unqual(ty);
    if (u->kind == TY_ARRAY && init->kind == I_LIST) {
        for (uint i = 0; i < init->items.n; i++) {
            if (!constant_initializer(u->to, init->items.v[i])) {
                return 0;
            }
        }
        return 1;
    }
    if (u->kind == TY_RECORD && init->kind == I_LIST) {
        struct record_body *body = u->rec->body;
        for (uint i = 0; i < body->nmembers && i < init->items.n; i++) {
            if (!constant_initializer(body->members[i].ty, init->items.v[i])) {
                return 0;
            }
        }
        return 1;
    }
    if (init->kind == I_LIST) {
        for (uint i = 0; i < init->items.n; i++) {
            if (!constant_initializer(ty, init->items.v[i])) {
                return 0;
            }
        }
        return 1;
    }
    struct expr *e = init->e;
    if (u->kind == TY_ARRAY && e->kind == E_STR) {
        return 1;
    }
    /* A struct initialized by a compound literal: constant if its list
     * is. */
    if (e->kind == E_COMPOUND) {
        return constant_initializer(e->ty, e->init);
    }
    struct cval c;
    return consteval_eval(e, &c);
}

/* Stores an initializer into the local object at off(s0); arrays and
 * structs initialized by a list have been cleared, so what the list leaves
 * out is zero already. */
static void init_local(struct funcgen *g, struct type *ty, struct init *init, int off) {
    struct type *u = ty_unqual(ty);
    if (init->kind == I_LIST && init->items.n == 0) {
        /* Zero: cleared already. */
        return;
    }
    if (u->kind == TY_ARRAY && init->kind == I_LIST) {
        int size = (int)ty_size(u->to);
        /* clang emits an array whose initializers hold more than 16 bytes as
         * a constant if it can, copied from a global it names constinit
         * after the string literals in it; else element by element. */
        int was = g->constant_init;
        int whole = 0;
        if (!was && (int64_t)init->items.n * size > 16) {
            whole = 1;
            for (uint i = 0; i < init->items.n && whole; i++) {
                whole = constant_initializer(u->to, init->items.v[i]);
            }
        }
        g->constant_init = was || whole;
        for (uint i = 0; i < init->items.n; i++) {
            init_local(g, u->to, init->items.v[i], off + (int)i * size);
        }
        g->constant_init = was;
        if (whole) {
            take_name(g->file, "constinit");
        }
        return;
    }
    if (u->kind == TY_RECORD && init->kind == I_LIST) {
        /* Sema's list is fully braced: one item per member, in order. */
        struct record_body *body = u->rec->body;
        for (uint i = 0; i < body->nmembers && i < init->items.n; i++) {
            init_local(g, body->members[i].ty, init->items.v[i], off + (int)body->members[i].offset);
        }
        return;
    }
    struct expr *e = init->e;
    if (u->kind == TY_RECORD) {
        /* A struct initialized by a struct: copy it. */
        expr(g, e);
        add_imm(g, "a1", "s0", off);
        copy(g, "a1", "a0", ty);
        return;
    }
    if (u->kind == TY_ARRAY && u->has_len && e->kind == E_STR) {
        if (g->constant_init) {
            /* Bytes of a constant clang emits whole. */
            for (uint i = 0; i < e->len && i < u->len; i++) {
                if (e->bytes[i] != 0) {
                    li(g, "a0", e->bytes[i]);
                    access_local(g, "sb", "a0", off + (int)i);
                }
            }
        } else {
            /* Otherwise clang copies the array from the literal, whose
             * array is this one, cut or padded with zeros. */
            uchar *array = arena_alloc(u->len + e->len + 2);
            memcpy(array, e->bytes, e->len);
            array[e->len] = 0;
            for (uint i = e->len + 1; i < u->len; i++) {
                array[i] = 0;
            }
            const char *lbl = string_array(g->file, array, u->len);
            la_label(g, "a0", lbl);
            add_imm(g, "a1", "s0", off);
            copy(g, "a1", "a0", ty);
        }
        return;
    }
    expr(g, e);
    store_local(g, ty, off);
}

static void local_decl(struct funcgen *g, struct declarator *d) {
    /* A static local is data, emitted where it is declared, as clang emits
     * it. */
    if (d->var.kind == VAR_GLOBAL) {
        variable(g->file, d, 1);
        return;
    }
    /* Without an initializer the local is left as it is. */
    if (d->init == NULL) {
        return;
    }
    int off = offset_of(g, d->var.local);
    /* An array, or a struct given member by member: clear the whole object,
     * then store what the initializer gives. */
    if (ty_is_array(d->ty) || (ty_record(d->ty) != NULL && d->init->kind == I_LIST)) {
        comment(g, fmt("%s = ...", ty_declare(d->ty, d->name)));
        clear_local(g, off, (int)ty_size(d->ty));
    }
    /* clang emits an array, struct or union with a constant initializer as
     * a static's. */
    int cst = (ty_is_array(d->ty) || ty_record(d->ty) != NULL) && constant_initializer(d->ty, d->init);
    g->constant_init = cst;
    init_local(g, d->ty, d->init, off);
    g->constant_init = 0;
    /* It copies the object from a constant of its own, which takes a name
     * after the string literals in it, unless it clears the object and
     * stores the few scalars that are not zero, or fills it with one byte. */
    if (cst) {
        struct image im;
        im.size = (uint)ty_size(d->ty);
        im.bytes = heap_calloc(im.size + 1, sizeof(int));
        im.nonzero = 0;
        im.symbol = 0;
        constant_image(d->ty, d->init, 0, &im);
        if (image_copied(&im)) {
            take_name(g->file, fmt("__const.%s.%s", g->func->name, d->name));
        }
        heap_free(im.bytes);
    }
}

/* Clears size bytes at off(s0) (aligned to a register): register-sized
 * stores, or a loop for more than 16 of them, then the last bytes. */
static void clear_local(struct funcgen *g, int off, int size) {
    int words = size / WORD;
    if (words > 16) {
        add_imm(g, "t1", "s0", off);
        li(g, "t0", (int64_t)words * WORD);
        ins(g, "add", "t0, t1, t0");
        label(g, "2");
        ins(g, "sd", "zero, 0(t1)");
        ins(g, "addi", fmt("t1, t1, %d", WORD));
        ins(g, "bltu", "t1, t0, 2b");
    } else {
        for (int i = 0; i < words; i++) {
            access_local(g, "sd", "zero", off + WORD * i);
        }
    }
    for (int b = words * WORD; b < size; b++) {
        access_local(g, "sb", "zero", off + b);
    }
}

/* ---- Expressions: each leaves its value in a0 ---- */

static void load(struct funcgen *g, struct type *ty);
static void load_local(struct funcgen *g, struct type *ty, int off);
static void addr(struct funcgen *g, struct expr *e);
static struct place place_of(struct funcgen *g, struct expr *e);
static void load_place(struct funcgen *g, struct place *p, struct type *ty);
static void store_place(struct funcgen *g, struct place p, struct type *ty);
static void convert(struct funcgen *g, struct type *from, struct type *to);
static void wrap_int(struct funcgen *g, struct type *ty);
static void binary(struct funcgen *g, int op, struct type *l, struct type *r);
static void call(struct funcgen *g, struct expr *e);
static void step(struct funcgen *g, struct type *ty, int delta);
static void compound_literal(struct funcgen *g, struct type *ty, struct init *init, int local);
static void va_start(struct funcgen *g, struct expr *ap);
static void va_arg(struct funcgen *g, struct expr *ap, struct type *ty);

/* An integer constant expression other than a literal: loads its value, as
 * one constant, and says so. An item of a local aggregate clang emits as a
 * constant has its evaluator's value; any other, what the instructions
 * would compute. */
static int folded(struct funcgen *g, struct expr *e) {
    if (!ty_is_integer(e->ty) || e->kind == E_NUM || e->kind == E_ENUMERATOR) {
        return 0;
    }
    struct cval v;
    int ok = g->constant_init ? consteval_eval(e, &v) : consteval_in_code(e, &v);
    if (!ok || v.is_addr) {
        return 0;
    }
    /* A disagreement would give the constant the wrong width. */
    if (ty_int_kind(e->ty) != v.ik) {
        die("codegen: consteval types a constant as sema does not");
    }
    constant(g, v.v, v.ik);
    return 1;
}

static void expr(struct funcgen *g, struct expr *e) {
    if (folded(g, e)) {
        return;
    }
    switch (e->kind) {
    case E_NUM:
    case E_ENUMERATOR:
        /* The value as the register holds it. */
        constant(g, e->value, ty_int_kind(e->ty));
        return;
    case E_STR:
        la_label(g, "a0", string_label(g->file, e->bytes, e->len));
        return;
    case E_VAR:
        if (e->var.kind == VAR_LOCAL) {
            int off = offset_of(g, e->var.local);
            if (ty_is_addressed(e->ty)) {
                /* An array or struct: its address. */
                add_imm(g, "a0", "s0", off);
            } else {
                load_local(g, e->ty, off);
            }
        } else {
            la(g, "a0", e->var.sym, e->ty);
            load(g, e->ty);
        }
        return;
    case E_ASSIGN:
        if (e->op == OP_NONE && ty_record(e->x->ty) != NULL) {
            /* Struct assignment copies the bytes; its value is the target. */
            addr(g, e->x);
            push_a0(g);
            expr(g, e->y);
            pop(g, "a1");
            copy(g, "a1", "a0", e->x->ty);
            ins(g, "mv", "a0, a1");
        } else if (e->op == OP_NONE) {
            struct place p = place_of(g, e->x);
            /* Already converted to the target's type by sema. */
            expr(g, e->y);
            store_place(g, p, e->x->ty);
        } else {
            struct place p = place_of(g, e->x);
            load_place(g, &p, e->x->ty);
            push_a0(g);
            expr(g, e->y);
            /* The target's value, then value. */
            pop(g, "a1");
            struct type *t = ty_decay(e->x->ty);
            struct type *v = ty_decay(e->y->ty);
            binary(g, e->op, t, v);
            convert(g, arith_type(e->op, t, v), e->x->ty);
            store_place(g, p, e->x->ty);
        }
        return;
    case E_INCDEC: {
        struct type *ty = e->x->ty;
        int st = ty_unqual(ty)->kind == TY_PTR ? (int)ty_step(ty) : 1;
        int delta = e->flag ? st : -st;
        struct place p = place_of(g, e->x);
        load_place(g, &p, ty);
        if (ty_int_kind(ty) == IK_BOOL) {
            /* b++ sets a _Bool, b-- flips it: the old value cannot be worked
             * out from the new one, so keep it. */
            ins(g, "mv", "t1, a0");
            add_imm(g, "a0", "a0", delta);
            wrap_int(g, ty);
            store_place(g, p, ty);
            if (!e->prefix) {
                ins(g, "mv", "a0, t1");
            }
            return;
        }
        step(g, ty, delta);
        store_place(g, p, ty);
        if (!e->prefix) {
            /* x++ gives the old value: the new one minus the step, wrapped
             * again. */
            step(g, ty, -delta);
        }
        return;
    }
    case E_BINARY:
        if (e->op == OP_LOGAND || e->op == OP_LOGOR) {
            int c = constant_condition(e->x);
            /* 0 && r and 1 || r are constants, folded above, and so are they
             * with a left operand only clang's folding of conditions
             * decides: r is left out. */
            if (c == (e->op == OP_LOGOR)) {
                li(g, "a0", e->op == OP_LOGOR);
                return;
            }
            /* 1 && r and 0 || r are r as 0 or 1. */
            if (c == (e->op == OP_LOGAND)) {
                expr(g, e->y);
                ins(g, "snez", "a0, a0");
                return;
            }
            /* The left operand decides, or the right one does, as 0 or 1. */
            const char *end = new_label(g, e->op == OP_LOGAND ? "and" : "or");
            expr(g, e->x);
            if (e->op == OP_LOGAND) {
                branch_if_zero(g, end);
            } else {
                branch_if_nonzero(g, end);
            }
            expr(g, e->y);
            label(g, end);
            ins(g, "snez", "a0, a0");
            return;
        }
        expr(g, e->x);
        push_a0(g);
        expr(g, e->y);
        pop(g, "a1");
        /* Now a1 = l, a0 = r. */
        binary(g, e->op, ty_decay(e->x->ty), ty_decay(e->y->ty));
        return;
    case E_UNARY:
        expr(g, e->x);
        if (e->op == UOP_NEG) {
            ins(g, is_w(e->ty) ? "negw" : "neg", "a0, a0");
        } else if (e->op == UOP_NOT) {
            ins(g, "seqz", "a0, a0");
        } else if (e->op == UOP_BITNOT) {
            ins(g, "not", "a0, a0");
        }
        /* (+x: promotion only, nothing to do.) */
        return;
    case E_ADDR:
        addr(g, e->x);
        return;
    case E_DEREF:
        expr(g, e->x);
        load(g, e->ty);
        return;
    case E_CAST:
        /* A null pointer constant made a pointer: null, its operand neither
         * generated nor described, as clang has it. */
        if (null_to_pointer(e)) {
            li(g, "a0", 0);
            return;
        }
        expr(g, e->x);
        convert(g, e->x->ty, e->ty);
        return;
    case E_COND: {
        /* A constant condition: only the operand it chooses, as clang has
         * it. */
        int c = constant_condition(e->x);
        if (c >= 0) {
            expr(g, c ? e->y : e->z);
            return;
        }
        const char *els = new_label(g, "else");
        const char *end = new_label(g, "end");
        expr(g, e->x);
        branch_if_zero(g, els);
        expr(g, e->y);
        ins(g, "j", end);
        label(g, els);
        expr(g, e->z);
        label(g, end);
        return;
    }
    case E_COMMA:
        expr(g, e->x);
        expr(g, e->y);
        return;
    case E_MEMBER:
        addr(g, e);
        load(g, e->ty);
        return;
    case E_CALL:
        call(g, e);
        return;
    case E_COMPOUND:
        compound_literal(g, e->ty, e->init, e->local);
        if (!ty_is_addressed(e->ty)) {
            load(g, e->ty);
        }
        return;
    case E_VA_START:
        va_start(g, e->x);
        return;
    case E_VA_ARG:
        va_arg(g, e->x, e->ty);
        return;
    default:
        die("codegen: sema folds sizeof and rewrites va_end and va_copy");
    }
}

/* a0 += delta in the type ty of a ++ or --, wrapped to it: an int steps with
 * addiw (or addw), which keeps it sign-extended. */
static void step(struct funcgen *g, struct type *ty, int delta) {
    if (is_w(ty)) {
        if (fits_imm12(delta)) {
            ins(g, "addiw", fmt("a0, a0, %d", delta));
        } else {
            li(g, "t0", delta);
            ins(g, "addw", "a0, a0, t0");
        }
    } else {
        add_imm(g, "a0", "a0", delta);
        wrap_int(g, ty);
    }
}

/* (type){ ... }: initializes its hidden local (cleared first, like a local
 * initialized by a list) and leaves its address in a0. */
static void compound_literal(struct funcgen *g, struct type *ty, struct init *init, int local) {
    int off = offset_of(g, (uint)local);
    comment(g, fmt("compound literal (%s)", ty_str(ty)));
    if (ty_is_array(ty) || ty_record(ty) != NULL) {
        clear_local(g, off, (int)ty_size(ty));
    }
    init_local(g, ty, init, off);
    add_imm(g, "a0", "s0", off);
}

/* va_start(ap, last): ap = the address of the first variadic argument, in
 * the registers saved at the top of the frame, or else among the caller's
 * stack words, past the named ones. */
static void va_start(struct funcgen *g, struct expr *ap) {
    struct place p = place_of(g, ap);
    const char *reg;
    int at;
    int first = arg_word(g->named_words, &reg, &at) ? -g->va_save : at;
    add_imm(g, "a0", "s0", first);
    store_place(g, p, ap->ty);
}

/* va_arg(ap, ty): the argument is where ap points, and ap moves past its
 * words. A scalar is loaded from there; a small struct's value is its
 * address, there; a big one's address is the word there. */
static void va_arg(struct funcgen *g, struct expr *ap, struct type *ty) {
    int pass = abi_classify(ty);
    struct place p = place_of(g, ap);
    load_place(g, &p, ap->ty);
    ins(g, "mv", "t1, a0");
    add_imm(g, "a0", "a0", WORD * (int)abi_registers(pass));
    store_place(g, p, ap->ty);
    if (pass == PASS_SCALAR) {
        /* A narrower scalar is in the low bytes of its word. */
        ins(g, load_op(ty), "a0, 0(t1)");
    } else if (pass == PASS_REF) {
        ins(g, "ld", "a0, 0(t1)");
    } else {
        ins(g, "mv", "a0, t1");
    }
}

/* Loads the value of type ty at the address in a0. Arrays, structs and
 * functions are not loaded: their value is that address; nor is *p of a
 * void * (GNU C's, which clang takes). */
static void load(struct funcgen *g, struct type *ty) {
    if (ty_is_void(ty)) {
        return;
    }
    if (!ty_is_addressed(ty)) {
        ins(g, load_op(ty), "a0, 0(a0)");
    }
}

static void load_local(struct funcgen *g, struct type *ty, int off) {
    access_local(g, load_op(ty), "a0", off);
}

static void store_local(struct funcgen *g, struct type *ty, int off) {
    access_local(g, store_op(ty), "a0", off);
}

/* Leaves the address of the lvalue e in a0. */
static void addr(struct funcgen *g, struct expr *e) {
    switch (e->kind) {
    case E_VAR:
        if (e->var.kind == VAR_LOCAL) {
            add_imm(g, "a0", "s0", offset_of(g, e->var.local));
        } else {
            la(g, "a0", e->var.sym, e->ty);
        }
        return;
    case E_DEREF:
        /* The address of *p is p. */
        expr(g, e->x);
        return;
    case E_STR:
        expr(g, e);
        return;
    case E_COMPOUND:
        compound_literal(g, e->ty, e->init, e->local);
        return;
    case E_MEMBER:
        /* A member is at its offset from the start of the struct, whose
         * value is its address (a call's result too, kept in a hidden
         * local), and p->m's p is one too. */
        expr(g, e->x);
        if (e->offset != 0) {
            add_imm(g, "a0", "a0", (int)e->offset);
        }
        return;
    default:
        die("codegen: the address of an rvalue");
    }
}

/* Prepares to store to e. */
static struct place place_of(struct funcgen *g, struct expr *e) {
    struct place p;
    if (e->kind == E_VAR && e->var.kind == VAR_LOCAL) {
        p.pushed = 0;
        p.off = offset_of(g, e->var.local);
        return p;
    }
    addr(g, e);
    push_a0(g);
    p.pushed = 1;
    p.off = 0;
    return p;
}

/* Loads the value at a place into a0; a pushed address stays pushed. */
static void load_place(struct funcgen *g, struct place *p, struct type *ty) {
    if (!p->pushed) {
        load_local(g, ty, p->off);
        return;
    }
    /* The address, left on the stack. */
    ins(g, "ld", "a0, 0(sp)");
    load(g, ty);
}

/* Stores a0 to a place, popping a pushed address. */
static void store_place(struct funcgen *g, struct place p, struct type *ty) {
    if (!p.pushed) {
        store_local(g, ty, p.off);
        return;
    }
    pop(g, "a1");
    ins(g, store_op(ty), "a0, 0(a1)");
}

/* Cuts reg down to the integer type ty and widens it back to the register
 * as convert keeps it: _Bool is 0 or 1 (snez), a byte or a half is cut and
 * widened by its signedness, a 32-bit type is sign-extended (sext.w), and a
 * register-wide one is as it is. */
static void wrap_reg(struct funcgen *g, const char *reg, struct type *ty) {
    int k = ty_int_kind(ty);
    if (k < 0) {
        return;
    }
    uint size = ik_size(k);
    uint shift = 8 * (PTR_SIZE - size);
    if (k == IK_BOOL) {
        ins(g, "snez", fmt("%s, %s", reg, reg));
    } else if (size == PTR_SIZE) {
        /* As it is. */
    } else if (size == 4) {
        ins(g, "sext.w", fmt("%s, %s", reg, reg));
    } else if (size == 1 && !ik_signed(k)) {
        ins(g, "andi", fmt("%s, %s, 255", reg, reg));
    } else {
        ins(g, "slli", fmt("%s, %s, %u", reg, reg, shift));
        ins(g, ik_signed(k) ? "srai" : "srli", fmt("%s, %s, %u", reg, reg, shift));
    }
}

static void wrap_int(struct funcgen *g, struct type *ty) {
    wrap_reg(g, "a0", ty);
}

/* The (size, signedness) of a type as convert sees it; 0 for none
 * (void, a struct). Pointers are unsigned addresses. */
static int shape(struct type *t, uint *size, int *is_signed) {
    if (t->kind == TY_INT) {
        *size = ik_size(t->ik);
        *is_signed = ik_signed(t->ik);
        return 1;
    }
    if (t->kind == TY_PTR || t->kind == TY_ARRAY || t->kind == TY_FUNC) {
        *size = PTR_SIZE;
        *is_signed = 0;
        return 1;
    }
    return 0;
}

/* Converts the value in reg from type from to type to, keeping every value
 * as a register holds it (merlin's convert_reg, whose comment lists the
 * cases). */
static void convert_reg(struct funcgen *g, const char *reg, struct type *from, struct type *to) {
    from = ty_unqual(from);
    to = ty_unqual(to);
    if (ty_int_kind(to) == IK_BOOL) {
        if (!ty_eq(from, to)) {
            ins(g, "snez", fmt("%s, %s", reg, reg));
        }
        return;
    }
    uint fsize;
    uint tsize;
    int fsigned;
    int tsigned;
    if (!shape(from, &fsize, &fsigned) || !shape(to, &tsize, &tsigned)) {
        /* To void, or a struct as it is. */
        return;
    }
    if (tsize == PTR_SIZE) {
        if (fsize == 4 && !fsigned) {
            ins(g, "slli", fmt("%s, %s, 32", reg, reg));
            ins(g, "srli", fmt("%s, %s, 32", reg, reg));
        }
        return;
    }
    if (tsize == 4) {
        if (fsize == 8) {
            ins(g, "sext.w", fmt("%s, %s", reg, reg));
        }
        return;
    }
    /* A narrower or equal type whose values all fit is already right. */
    int fits = (fsize < tsize && (tsigned || !fsigned)) || (fsize == tsize && fsigned == tsigned);
    if (!fits) {
        wrap_reg(g, reg, to);
    }
}

static void convert(struct funcgen *g, struct type *from, struct type *to) {
    convert_reg(g, "a0", from, to);
}

/* Multiplies reg by size: the scaling of pointer arithmetic. */
static void scale(struct funcgen *g, const char *reg, uint size) {
    int n = log2_of(size);
    if (n == 0) {
        return;
    }
    if (n > 0) {
        ins(g, "slli", fmt("%s, %s, %d", reg, reg, n));
        return;
    }
    li(g, "t0", size);
    ins(g, "mul", fmt("%s, %s, t0", reg, reg));
}

/* The W form of an instruction for 32-bit arithmetic. */
static const char *w_form(int w32, const char *op) {
    return w32 ? fmt("%sw", op) : op;
}

/* binary once the operands are converted: both of the integer type l (= r),
 * or pointers, or a pointer and a register-wide integer. */
static void arith(struct funcgen *g, int op, struct type *l, struct type *r) {
    /* Comparisons, division and remainder follow the common type's
     * signedness; a pointer compares as an unsigned address. */
    int u = ty_is_unsigned(l) || ty_is_unsigned(r);
    const char *slt = u ? "sltu" : "slt";
    int w32 = is_w(l);
    switch (op) {
    case OP_ADD:
        if (ty_is_ptr(l) || ty_is_ptr(r)) {
            /* A pointer plus an integer moves by whole elements. */
            if (ty_is_ptr(l)) {
                scale(g, "a0", (uint)ty_step(l));
            } else {
                scale(g, "a1", (uint)ty_step(r));
            }
            ins(g, "add", "a0, a1, a0");
        } else {
            ins(g, w_form(w32, "add"), "a0, a1, a0");
        }
        return;
    case OP_SUB:
        if (ty_is_ptr(l) && ty_is_ptr(r)) {
            /* The difference of two pointers counts elements. */
            ins(g, "sub", "a0, a1, a0");
            uint size = (uint)ty_step(l);
            int n = log2_of(size);
            if (n > 0) {
                ins(g, "srai", fmt("a0, a0, %d", n));
            } else if (n < 0) {
                li(g, "t0", size);
                ins(g, "div", "a0, a0, t0");
            }
        } else if (ty_is_ptr(l)) {
            scale(g, "a0", (uint)ty_step(l));
            ins(g, "sub", "a0, a1, a0");
        } else {
            ins(g, w_form(w32, "sub"), "a0, a1, a0");
        }
        return;
    case OP_MUL:
        ins(g, w_form(w32, "mul"), "a0, a1, a0");
        return;
    case OP_DIV:
        ins(g, w_form(w32, u ? "divu" : "div"), "a0, a1, a0");
        return;
    case OP_REM:
        ins(g, w_form(w32, u ? "remu" : "rem"), "a0, a1, a0");
        return;
    case OP_SHL:
        ins(g, w_form(w32, "sll"), "a0, a1, a0");
        return;
    case OP_SHR:
        /* Only the left operand's type matters for a shift. */
        ins(g, w_form(w32, ty_is_unsigned(l) ? "srl" : "sra"), "a0, a1, a0");
        return;
    case OP_BITAND:
        ins(g, "and", "a0, a1, a0");
        return;
    case OP_BITOR:
        ins(g, "or", "a0, a1, a0");
        return;
    case OP_BITXOR:
        ins(g, "xor", "a0, a1, a0");
        return;
    case OP_LT:
        ins(g, slt, "a0, a1, a0");
        return;
    case OP_GT:
        ins(g, slt, "a0, a0, a1");
        return;
    case OP_LE:
        /* l <= r is !(r < l). */
        ins(g, slt, "a0, a0, a1");
        ins(g, "xori", "a0, a0, 1");
        return;
    case OP_GE:
        /* l >= r is !(l < r). */
        ins(g, slt, "a0, a1, a0");
        ins(g, "xori", "a0, a0, 1");
        return;
    case OP_EQ:
        /* 0 exactly when l == r. */
        ins(g, "xor", "a0, a1, a0");
        ins(g, "seqz", "a0, a0");
        return;
    case OP_NE:
        ins(g, "xor", "a0, a1, a0");
        ins(g, "snez", "a0, a0");
        return;
    default:
        die("codegen: && and || short-circuit");
    }
}

/* a0 = a1 op a0 for the operators that always evaluate both sides, with l
 * and r the decayed types of a1 and a0. Integer operands are first
 * converted to the type the operation is done in; an integer added to a
 * pointer is first widened to the pointer's width by its signedness. */
static void binary(struct funcgen *g, int op, struct type *l, struct type *r) {
    int lk = ty_int_kind(l);
    int rk = ty_int_kind(r);
    if (lk >= 0 && rk >= 0) {
        struct type *k = ty_int(op == OP_SHL || op == OP_SHR ? ik_promoted(lk) : ik_common(lk, rk));
        convert_reg(g, "a1", l, k);
        if (op != OP_SHL && op != OP_SHR) {
            convert_reg(g, "a0", r, k);
        }
        arith(g, op, k, k);
        return;
    }
    /* A pointer and an integer: the index as a register-wide integer
     * (ptrdiff_t, or size_t for an unsigned one). */
    if (ty_is_ptr(l) && ty_is_integer(r)) {
        convert_reg(g, "a0", r, ty_int(ty_is_unsigned(r) ? ik_size_t() : ik_ptrdiff_t()));
    } else if (ty_is_ptr(r) && ty_is_integer(l)) {
        convert_reg(g, "a1", l, ty_int(ty_is_unsigned(l) ? ik_size_t() : ik_ptrdiff_t()));
    }
    arith(g, op, l, r);
}

/* ---- Calls ---- */

/* How a call's arguments travel (merlin's ArgLayout): the arguments' types
 * (the parameters', then for `...` the promoted arguments' own), each one's
 * first word and how it is passed, and how many words they take in all. */
struct arg_layout {
    struct type **types;
    uint *words;
    int *passes;
    uint used;
};

static void arg_layout(struct functype *ft, struct expr *call, struct arg_layout *l) {
    uint n = call->nargs;
    l->types = arena_alloc((n + 1) * sizeof(struct type *));
    for (uint i = 0; i < n; i++) {
        l->types[i] = i < ft->nparams ? ft->params[i] : call->args[i]->ty;
    }
    l->words = arena_alloc((n + 1) * sizeof(uint));
    l->passes = arena_alloc((n + 1) * sizeof(int));
    l->used = abi_call_arguments(ft->ret, l->types, n, l->words, l->passes);
}

/* Evaluates an argument into a0: a scalar's value, or a struct's address. A
 * struct passed by reference is first copied to its hidden local (copy >=
 * 0), and that copy's address is what is passed. */
static void argument(struct funcgen *g, struct expr *arg, int copy_local) {
    expr(g, arg);
    if (copy_local >= 0) {
        add_imm(g, "a1", "s0", offset_of(g, (uint)copy_local));
        copy(g, "a1", "a0", arg->ty);
        ins(g, "mv", "a0, a1");
    }
}

/* Loads word w (8 bytes) of the struct of type ty at the address in src into
 * reg. A struct aligned to less than a register may not be at a register
 * boundary, so its word is put together from aligned bytes, halves or
 * words, each zero-extended (in t0); bytes past its end read as zero. */
static void load_word(struct funcgen *g, const char *reg, const char *src, uint w, struct type *ty) {
    uint size = (uint)ty_size(ty);
    uint reg_size = WORD;
    uint unit = ty_align(ty) < reg_size ? ty_align(ty) : reg_size;
    if (unit == reg_size) {
        ins(g, "ld", fmt("%s, %u(%s)", reg, reg_size * w, src));
        return;
    }
    const char *op = unit == 1 ? "lbu" : unit == 2 ? "lhu" : "lwu";
    for (uint k = 0; k < reg_size; k += unit) {
        uint at = reg_size * w + k;
        if (at >= size) {
            break;
        }
        if (k == 0) {
            ins(g, op, fmt("%s, %u(%s)", reg, at, src));
        } else {
            ins(g, op, fmt("t0, %u(%s)", at, src));
            ins(g, "slli", fmt("t0, t0, %u", 8 * k));
            ins(g, "or", fmt("%s, %s, t0", reg, reg));
        }
    }
}

/* Loads the n words of the struct of type ty at the address in src into
 * a<first>, a<first+1>: how a small struct is passed and returned. */
static void load_words(struct funcgen *g, const char *src, int first, uint n, struct type *ty) {
    for (uint w = 0; w < n; w++) {
        load_word(g, fmt("a%u", (uint)first + w), src, w, ty);
    }
}

/* Copies the struct of type ty from the address in src to the one in dst
 * (neither of them t0-t3, both left as they are), in units of its
 * alignment (at most a register): unrolled up to 16 units, a loop beyond. */
static void copy(struct funcgen *g, const char *dst, const char *src, struct type *ty) {
    uint size = (uint)ty_size(ty);
    uint unit = ty_align(ty) < (uint)WORD ? ty_align(ty) : (uint)WORD;
    const char *ld = unit == 1 ? "lbu" : unit == 2 ? "lhu" : unit == 4 ? "lw" : "ld";
    const char *sd = unit == 1 ? "sb" : unit == 2 ? "sh" : unit == 4 ? "sw" : "sd";
    if (size / unit <= 16) {
        for (uint at = 0; at < size; at += unit) {
            ins(g, ld, fmt("t0, %u(%s)", at, src));
            ins(g, sd, fmt("t0, %u(%s)", at, dst));
        }
        return;
    }
    ins(g, "mv", fmt("t1, %s", src));
    ins(g, "mv", fmt("t2, %s", dst));
    li(g, "t3", size);
    ins(g, "add", "t3, t1, t3");
    label(g, "2");
    ins(g, ld, "t0, 0(t1)");
    ins(g, sd, "t0, 0(t2)");
    ins(g, "addi", fmt("t1, t1, %u", unit));
    ins(g, "addi", fmt("t2, t2, %u", unit));
    ins(g, "bltu", "t1, t3, 2b");
}

/* The call instruction: call f, or jalr t1 through a pointer. */
static void jump_to(struct funcgen *g, const char *direct) {
    if (direct != NULL) {
        /* call is auipc ra + jalr ra: two words. */
        reference(g->file, direct, NULL);
        ins_sized(g, 8, "call", direct);
    } else {
        ins(g, "jalr", "t1");
    }
}

/* After a call: a struct result is kept in its hidden local (a small one
 * arrives in a0/a1 and is stored there); the call's value is that
 * address. */
static void call_result(struct funcgen *g, struct functype *ft, int result) {
    if (result < 0) {
        return;
    }
    int off = offset_of(g, (uint)result);
    int pass = abi_classify(ft->ret);
    if (pass == PASS_WORDS1 || pass == PASS_WORDS2) {
        for (uint i = 0; i < abi_registers(pass); i++) {
            access_local(g, "sd", fmt("a%u", i), off + WORD * (int)i);
        }
    }
    add_imm(g, "a0", "s0", off);
}

/* A call whose arguments take more than a0-a7: every argument is evaluated
 * and pushed, left to right; then room is made below them for the stack
 * words, keeping sp 16-byte aligned at the call, and each word is copied to
 * its place, on the stack or in a register (merlin's call_with_stack). */
static void call_with_stack(struct funcgen *g, struct expr *e, int hidden, struct arg_layout *l) {
    struct expr *callee = e->x;
    const char *direct = direct_callee(callee);
    if (direct == NULL) {
        expr(g, callee);
        push_a0(g);
    }
    /* Each argument takes one word pushed. */
    for (uint i = 0; i < e->nargs; i++) {
        argument(g, e->args[i], e->arg_copies[i]);
        push_a0(g);
    }
    int n = (int)e->nargs;
    int pushed = WORD * (int)g->depth;
    uint sw = abi_stack_words(l->used);
    int stack = WORD * (int)sw;
    int area = (pushed + stack + 15) / 16 * 16 - pushed;
    comment(g, sw == 1 ? "1 argument word goes on the stack" : fmt("%u argument words go on the stack", sw));
    add_imm(g, "sp", "sp", -area);
    /* The stack words first (with t1, t2 and t0), then the registers. Where
     * argument i was pushed: the last one is right above the area. */
    for (uint i = 0; i < e->nargs; i++) {
        int temp = area + WORD * (int)(e->nargs - 1 - i);
        uint word = l->words[i];
        int pass = l->passes[i];
        const char *reg;
        int at;
        if (pass == PASS_SCALAR || pass == PASS_REF) {
            if (!arg_word(word, &reg, &at)) {
                ins(g, "ld", fmt("t1, %d(sp)", temp));
                ins(g, "sd", fmt("t1, %d(sp)", at));
            }
        } else {
            for (uint k = 0; k < abi_registers(pass); k++) {
                if (!arg_word(word + k, &reg, &at)) {
                    ins(g, "ld", fmt("t1, %d(sp)", temp));
                    load_word(g, "t2", "t1", k, l->types[i]);
                    ins(g, "sd", fmt("t2, %d(sp)", at));
                }
            }
        }
    }
    for (uint i = 0; i < e->nargs; i++) {
        int temp = area + WORD * (int)(e->nargs - 1 - i);
        uint word = l->words[i];
        int pass = l->passes[i];
        const char *reg;
        int at;
        if (pass == PASS_SCALAR || pass == PASS_REF) {
            if (arg_word(word, &reg, &at)) {
                ins(g, "ld", fmt("%s, %d(sp)", reg, temp));
            }
        } else {
            if (word < ARG_REGS) {
                ins(g, "ld", fmt("t1, %d(sp)", temp));
            }
            for (uint k = 0; k < abi_registers(pass); k++) {
                if (arg_word(word + k, &reg, &at)) {
                    load_word(g, reg, "t1", k, l->types[i]);
                }
            }
        }
    }
    /* A big struct result goes to the hidden local: its address in a0. */
    if (hidden >= 0) {
        add_imm(g, "a0", "s0", offset_of(g, (uint)hidden));
    }
    if (direct == NULL) {
        ins(g, "ld", fmt("t1, %d(sp)", area + WORD * n));
    }
    jump_to(g, direct);
    int popped = n + (direct == NULL ? 1 : 0);
    add_imm(g, "sp", "sp", area + WORD * popped);
    g->depth -= (uint)popped;
}

/* A call: f(args) with f a function's name (call f), or through any other
 * expression of function or pointer-to-function type (jalr). */
static void call(struct funcgen *g, struct expr *e) {
    struct expr *callee = e->x;
    struct functype *ft = ty_pointee(ty_decay(callee->ty))->fn;
    struct arg_layout l;
    arg_layout(ft, e, &l);
    const char *direct = direct_callee(callee);
    /* clang refers to the function it calls before the arguments (EmitCallee,
     * then EmitCallArgs). */
    if (direct != NULL) {
        reference(g->file, direct, NULL);
    }
    if (l.used > ARG_REGS) {
        int hidden = abi_returns_in_memory(ft->ret) ? e->result : -1;
        call_with_stack(g, e, hidden, &l);
        call_result(g, ft, e->result);
        return;
    }
    /* Through a pointer: the function's address is evaluated first and waits
     * on the stack, under the arguments, until they are loaded. */
    if (direct == NULL) {
        expr(g, callee);
        if (e->nargs == 0) {
            ins(g, "mv", "t1, a0");
        } else {
            push_a0(g);
        }
    }
    /* Evaluate every argument before loading any argument register: all but
     * the last are pushed; the last one, already in a0, goes straight to its
     * register(s), and then the others are popped into theirs. */
    if (e->nargs > 0) {
        uint last = e->nargs - 1;
        for (uint i = 0; i < last; i++) {
            argument(g, e->args[i], e->arg_copies[i]);
            push_a0(g);
        }
        argument(g, e->args[last], e->arg_copies[last]);
        uint r = l.words[last];
        int pass = l.passes[last];
        if (pass == PASS_SCALAR || pass == PASS_REF) {
            if (r != 0) {
                ins(g, "mv", fmt("a%u, a0", r));
            }
        } else {
            ins(g, "mv", "t1, a0");
            load_words(g, "t1", (int)r, abi_registers(pass), l.types[last]);
        }
        for (uint i = last; i > 0; i--) {
            uint k = i - 1;
            if (l.passes[k] == PASS_SCALAR || l.passes[k] == PASS_REF) {
                pop(g, fmt("a%u", l.words[k]));
            } else {
                pop(g, "t1");
                load_words(g, "t1", (int)l.words[k], abi_registers(l.passes[k]), l.types[k]);
            }
        }
        if (direct == NULL) {
            pop(g, "t1");
        }
    }
    /* A big struct result goes to the hidden local: its address in a0. */
    if (abi_returns_in_memory(ft->ret)) {
        add_imm(g, "a0", "s0", offset_of(g, (uint)e->result));
    }
    /* Temporaries of an enclosing expression may still be pushed: pad the
     * stack to the next multiple of 16 around the call. */
    int pad = (16 - ((int)g->depth * WORD) % 16) % 16;
    if (pad != 0) {
        ins(g, "addi", fmt("sp, sp, %d", -pad));
    }
    jump_to(g, direct);
    if (pad != 0) {
        ins(g, "addi", fmt("sp, sp, %d", pad));
    }
    call_result(g, ft, e->result);
}

/* ---- Branch relaxation and rendering ---- */

/* B-type branches reach -4096..=+4094 bytes from themselves. */
#define BRANCH_MIN (-4096)
#define BRANCH_MAX 4094

/* Decides which conditional branches need the long form (the opposite
 * branch over a j): a flag per line. Addresses are estimated with every
 * pseudo-instruction at its largest, so a distance is never
 * underestimated; making a branch long moves code after it, so this
 * repeats until nothing changes. */
static uchar *relax_branches(struct line *lines, uint n) {
    uchar *lng = heap_calloc(n + 1, 1);
    int64_t *addr = heap_alloc((n + 1) * sizeof(int64_t));
    for (;;) {
        /* Where each line starts and where each label is (as pc + 1). */
        struct map labels;
        memset(&labels, 0, sizeof labels);
        int64_t *at = heap_alloc((n + 1) * sizeof(int64_t));
        uint nat = 0;
        int64_t pc = 0;
        for (uint i = 0; i < n; i++) {
            addr[i] = pc;
            struct line *l = &lines[i];
            if (l->kind == LINE_LABEL) {
                at[nat] = pc;
                map_put(&labels, l->text, &at[nat]);
                nat++;
            } else if (l->kind == LINE_INSN) {
                pc += l->size;
            } else if (l->kind == LINE_BRANCH) {
                pc += lng[i] ? 8 : 4;
            }
        }
        int changed = 0;
        for (uint i = 0; i < n; i++) {
            if (lines[i].kind != LINE_BRANCH) {
                continue;
            }
            int64_t distance = *(int64_t *)map_get(&labels, lines[i].text) - addr[i];
            if (!lng[i] && (distance < BRANCH_MIN || distance > BRANCH_MAX)) {
                lng[i] = 1;
                changed = 1;
            }
        }
        heap_free(labels.keys);
        heap_free(labels.values);
        heap_free(at);
        if (!changed) {
            heap_free(addr);
            return lng;
        }
    }
}

/* A j reaches -1 MiB..=+1 MiB - 2 bytes from itself. */
#define JUMP_REACH ((1 << 20) - 2)

/* Every j in a function goes to a label in it, so none is out of reach
 * when the whole function fits in a jump's reach. */
static struct diag *check_size(struct function *f, struct line *lines, uint n, uchar *lng) {
    int64_t size = 0;
    for (uint i = 0; i < n; i++) {
        if (lines[i].kind == LINE_INSN) {
            size += lines[i].size;
        } else if (lines[i].kind == LINE_BRANCH) {
            size += lng[i] ? 8 : 4;
        }
    }
    if (size > JUMP_REACH) {
        return diag_error(f->pos, fmt("function '%s' is too big: up to %lld bytes of code, and the jumps inside a "
                                      "function reach only 1 MiB",
                                      f->name, size));
    }
    return NULL;
}

/* Writes a function's lines as assembly text. */
static void render(struct line *lines, uint n, uchar *lng, struct buf *out) {
    for (uint i = 0; i < n; i++) {
        struct line *l = &lines[i];
        if (l->kind == LINE_LABEL) {
            buf_fmt(out, "%s:\n", l->text);
        } else if (l->kind == LINE_INSN || l->kind == LINE_TEXT) {
            buf_fmt(out, "    %s\n", l->text);
        } else if (lng[i]) {
            /* Too far for a branch: skip over a j when the condition fails. */
            const char *op = l->op;
            const char *inverse = strcmp(op, "beqz") == 0  ? "bnez"
                                  : strcmp(op, "bnez") == 0 ? "beqz"
                                  : strcmp(op, "beq") == 0  ? "bne"
                                                            : "beq";
            buf_fmt(out, "    %s\n", insn_text(inverse, fmt("%s, 1f", l->regs)));
            buf_fmt(out, "    %s\n", insn_text("j", l->text));
            buf_str(out, "1:\n");
        } else {
            buf_fmt(out, "    %s\n", insn_text(l->op, fmt("%s, %s", l->regs, l->text)));
        }
    }
}

/* ---- The file ---- */

/* Generates function f's code (merlin's FileGen::function): a first pass,
 * generating into nothing, finds the locals the code uses; the second is
 * the function, its frame holding those alone. */
static struct diag *function(struct filegen *file, struct function *f) {
    create(file, f->name);
    take_over(file, f->name, 0);
    /* A variadic function keeps the argument registers its named parameters
     * leave free at the top of its frame. */
    struct type *ft = function_ty(f);
    uint *words = arena_alloc((f->nparams + 1) * sizeof(uint));
    int *passes = arena_alloc((f->nparams + 1) * sizeof(int));
    uint named_words = abi_call_arguments(ft->fn->ret, ft->fn->params, ft->fn->nparams, words, passes);
    int va_save = f->variadic ? WORD * (int)(ARG_REGS - (named_words < ARG_REGS ? named_words : ARG_REGS)) : 0;
    int header = SAVED_REGS + va_save;
    uint n = f->locals.n;
    uchar *all = heap_alloc(n + 1);
    memset(all, 1, n + 1);
    uint pass;
    uchar *used = NULL;
    struct funcgen *g = NULL;
    for (pass = 0; pass < 2; pass++) {
        int *offsets;
        int frame;
        uint labels = file->labels;
        uint strings_n = file->strings.n;
        if (pass == 0) {
            file->dry = 1;
            if (!layout(&f->locals, all, header, &offsets, &frame)) {
                offsets = arena_alloc(n * sizeof(int) + 1);
                frame = 0;
            }
        } else if (!layout(&f->locals, used, header, &offsets, &frame)) {
            heap_free(all);
            return diag_error(f->pos,
                              fmt("the locals of '%s' need a stack frame larger than 2 GiB", f->name));
        }
        g = heap_calloc(1, sizeof(struct funcgen));
        g->func = f;
        g->offsets = offsets;
        g->frame = frame;
        g->named_words = named_words;
        g->va_save = va_save;
        g->file = file;
        g->ret_label = fmt(".L.return.%s", f->name);
        g->used = heap_calloc(n + 1, 1);
        for (uint i = 0; i < n && i < f->nparams; i++) {
            g->used[i] = 1;
        }
        g->slots = pass == 0 ? all : used;
        g->reachable = 1;
        function_body(g, &f->body);
        if (pass == 0) {
            /* What the first pass made is taken back: its labels and string
             * literals. */
            file->dry = 0;
            file->labels = labels;
            vec_truncate(&file->strings, strings_n);
            used = g->used;
            heap_free(g->lines);
            heap_free(g->reached.keys);
            heap_free(g->reached.values);
            heap_free(g);
        }
    }
    begin_symbol(file);
    piece(file, ".text");
    buf_byte(&file->out, '\n');
    uchar *lng = relax_branches(g->lines, g->nlines);
    struct diag *e = check_size(f, g->lines, g->nlines, lng);
    if (e != NULL) {
        return e;
    }
    render(g->lines, g->nlines, lng, &file->out);
    finish_symbol(file, f->name);
    heap_free(lng);
    heap_free(g->lines);
    heap_free(all);
    return NULL;
}

/* Emits definition k: its code or its data. */
static struct diag *emit(struct filegen *file, uint k) {
    file->emitted[k] = 1;
    struct def *d = &file->defs[k];
    if (d->f != NULL) {
        return function(file, d->f);
    }
    variable(file, d->d, 0);
    return NULL;
}

/* Emits what was used and not emitted yet, each one's uses right after it,
 * depth first (clang's EmitDeferred). */
static struct diag *deferred(struct filegen *file) {
    struct vec now = vec_take(&file->queue);
    for (uint i = 0; i < now.n; i++) {
        uint k = (uint)((uintptr_t)now.v[i] - 1);
        if (!file->emitted[k]) {
            TRY(emit(file, k));
            if (file->queue.n > 0) {
                TRY(deferred(file));
            }
        }
    }
    heap_free(now.v);
    return NULL;
}

/* Whether name is a register's (excalibur's reg::parse): x0 to x31 (u8
 * digits, leading zeros allowed), fp, or an ABI name. */
static int register_name(const char *name) {
    if (name[0] == 'x' && name[1] != 0) {
        uint v = 0;
        const char *p = name + 1;
        int digits = 1;
        for (; *p; p++) {
            if (*p < '0' || *p > '9') {
                digits = 0;
                break;
            }
            v = v * 10 + (uint)(*p - '0');
            if (v > 255) {
                digits = 0;
                break;
            }
        }
        if (digits) {
            return v < 32;
        }
    }
    if (strcmp(name, "fp") == 0) {
        return 1;
    }
    for (uint i = 0; i < 32; i++) {
        if (strcmp(REG_NAMES[i], name) == 0) {
            return 1;
        }
    }
    return 0;
}

int codegen_generate(struct program *prog, struct buf *out, struct diag **error) {
    /* merlin-as reads a register name as a register, never as a symbol, so a
     * function or global called sp or a0 could not be referenced. */
    for (uint i = 0; i < prog->items.n; i++) {
        struct item *it = prog->items.v[i];
        if (it->kind == ITEM_FUNC) {
            if (register_name(it->func->name)) {
                *error = diag_error(it->func->pos, fmt("'%s' is a register name, which cannot be used as a symbol name",
                                                       it->func->name));
                return 0;
            }
        } else {
            for (uint k = 0; k < it->decls.n; k++) {
                struct declarator *d = it->decls.v[k];
                if (register_name(d->name)) {
                    *error = diag_error(d->pos, fmt("'%s' is a register name, which cannot be used as a symbol "
                                                    "name",
                                                    d->name));
                    return 0;
                }
            }
        }
    }
    /* The definitions, in the order they come. */
    struct filegen *file = heap_calloc(1, sizeof(struct filegen));
    file->section = "";
    struct vec defs;
    memset(&defs, 0, sizeof defs);
    for (uint i = 0; i < prog->items.n; i++) {
        struct item *it = prog->items.v[i];
        if (it->kind == ITEM_FUNC) {
            if (it->func->has_body) {
                struct def *d = arena_alloc(sizeof *d);
                d->f = it->func;
                vec_push(&defs, d);
            }
        } else {
            for (uint k = 0; k < it->decls.n; k++) {
                struct declarator *dr = it->decls.v[k];
                if (dr->define) {
                    struct def *d = arena_alloc(sizeof *d);
                    d->d = dr;
                    vec_push(&defs, d);
                }
            }
        }
    }
    uint n = defs.n;
    file->defs = arena_alloc((n + 1) * sizeof(struct def));
    for (uint k = 0; k < n; k++) {
        file->defs[k] = *(struct def *)defs.v[k];
        map_put(&file->by_symbol, def_symbol(&file->defs[k]), (void *)((uintptr_t)k + 1));
    }
    file->ndefs = n;
    file->emitted = heap_calloc(n + 1, 1);
    file->waiting = heap_calloc(n + 1, 1);
    /* clang's front end: the file, as it comes, emits what must be emitted
     * and keeps the rest for when it is used; then the tentative
     * definitions; then what was used, each one's uses right after it. */
    for (uint k = 0; k < n; k++) {
        struct def *d = &file->defs[k];
        if (def_hidden(d) || def_tentative(d)) {
            continue;
        }
        if (!def_deferrable(d)) {
            if ((*error = emit(file, k)) != NULL) {
                return 0;
            }
        } else if (map_get(&file->created_set, def_symbol(d)) != NULL) {
            vec_push(&file->queue, (void *)((uintptr_t)k + 1));
        } else {
            file->waiting[k] = 1;
        }
    }
    for (uint k = 0; k < n; k++) {
        struct def *d = &file->defs[k];
        if (!def_tentative(d) || def_hidden(d)) {
            continue;
        }
        if (def_deferrable(d) && map_get(&file->created_set, def_symbol(d)) == NULL) {
            file->waiting[k] = 1;
        } else if ((*error = emit(file, k)) != NULL) {
            return 0;
        }
    }
    if ((*error = deferred(file)) != NULL) {
        return 0;
    }
    /* The output: each symbol's code or data in the order the symbols were
     * made, then the string literals. */
    file->out.len = 0;
    file->section = "";
    for (uint i = 0; i < file->created.n; i++) {
        const char *sym = file->created.v[i];
        struct vec *pieces = map_get(&file->chunks, sym);
        if (pieces == NULL) {
            continue;
        }
        map_remove(&file->chunks, sym);
        for (uint k = 0; k < pieces->n; k++) {
            struct piece *p = pieces->v[k];
            if (p->section[0] != 0) {
                switch_section(file, p->section);
            }
            buf_put(&file->out, p->text, p->len);
        }
    }
    strings(file);
    buf_put(out, file->out.len > 0 ? file->out.data : "", file->out.len);
    return 1;
}
