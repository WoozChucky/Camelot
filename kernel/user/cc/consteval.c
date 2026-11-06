/* cc1: constant expressions, values known at compile time (follows merlin's
 * consteval.rs, whose comments say more).
 *
 * Three users: the parser sizes arrays (`int a[4 * sizeof(int)]`), sema
 * finds null pointer constants and checks that global initializers are
 * constant, and code generation turns those initializers into data, folds
 * every integer constant expression in code into one constant, as clang
 * does, and decides the conditions clang's code generator decides
 * (consteval_condition, which also knows that an object's address is not
 * null).
 *
 * Integer constants carry their C type and wrap the way the generated code
 * would at run time. Values are i128s. The parser asks before sema has
 * typed the tree, so integer types are worked out from the leaves
 * (literals, casts, sizeof) rather than read from the expression's type.
 * Address constants (&g, arr + 2, "text") need the types sema assigns, so
 * they are only found in a checked tree.
 *
 * An address converted to an integer type as wide as a pointer stays an
 * address constant, as clang has it: (long)&g initializes a global with
 * `.quad g`, and adding or subtracting an integer moves it by that many
 * bytes. Any other arithmetic on it is not constant, nor is a narrower
 * conversion. */
#include "cc.h"

static int value(struct expr *e, int evaluated, int checked, int hw, struct cval *out);

static int cint(struct cval *c, i128 v, int k) {
    memset(c, 0, sizeof *c);
    c->v = v;
    c->ik = k;
    return 1;
}

/* The integer of a constant: 0 for an address. */
static int int_of(struct cval *c, i128 *v, int *k) {
    if (c->is_addr) {
        return 0;
    }
    *v = c->v;
    if (k != NULL) {
        *k = c->ik;
    }
    return 1;
}

int consteval_int(struct expr *e, i128 *v) {
    struct cval c;
    return value(e, 1, 1, 0, &c) && int_of(&c, v, NULL);
}

int consteval_parsed_int(struct expr *e, i128 *v) {
    struct cval c;
    return value(e, 1, 0, 0, &c) && int_of(&c, v, NULL);
}

int consteval_parsed_unevaluated_int(struct expr *e, i128 *v) {
    struct cval c;
    return value(e, 0, 0, 0, &c) && int_of(&c, v, NULL);
}

/* An integer constant expression equal to 0, or one cast to void * (C99
 * 6.3.2.3). */
int is_null_pointer(struct expr *e) {
    if (e->kind == E_CAST && !e->implicit && ty_eq(e->ty, ty_ptr(TY_VOID_T))) {
        return is_null_pointer(e->x);
    }
    i128 v;
    return ty_is_integer(e->ty) && consteval_int(e, &v) && i128_is_zero(v);
}

int consteval_eval(struct expr *e, struct cval *out) {
    return value(e, 1, 1, 0, out);
}

/* Of an expression in code, which code generation folds: what the
 * instructions would compute where C leaves the value undefined (a shift
 * by the width or more uses the low bits of its count, as sll does). */
int consteval_in_code(struct expr *e, struct cval *out) {
    return value(e, 1, 1, 1, out);
}

/* Whether a constant is true: an object's address always, a plain number
 * used as an address if it is not 0. */
static int const_truth(struct cval *c) {
    if (!c->is_addr) {
        return !i128_is_zero(c->v);
    }
    if (c->base != BASE_NONE) {
        return 1;
    }
    return !i128_is_zero(c->offset);
}

/* How many bytes adding 1 to a constant of type t moves an address. */
static int step_of(struct type *t, i128 *out) {
    if (ty_is_integer(t)) {
        *out = i128_of(1);
        return 1;
    }
    int64_t s = ty_step(t);
    if (s < 0) {
        return 0;
    }
    *out = i128_of(s);
    return 1;
}

static i128 i128_abs(i128 a) {
    return i128_neg_p(a) ? i128_neg(a) : a;
}

/* An arithmetic, bitwise or comparison operator on two integer constants.
 * Dividing by zero is not constant where it is evaluated. A shift's count
 * past the width is as the hardware takes it if hw, else as clang's
 * evaluator does. */
static int binary(int op, i128 a, int ak, i128 b, int bk, int evaluated, int hw, struct cval *out) {
    int shift_op = op == OP_SHL || op == OP_SHR;
    int k = shift_op ? ik_promoted(ak) : ik_common(ak, bk);
    a = ik_wrap(k, a);
    if (!shift_op) {
        b = ik_wrap(k, b);
    }
    /* A shift as clang's evaluator does one (handleIntIntBinOp): a count of
     * the width or more shifts by one less than the width, and a negative
     * one shifts the other way by as much. */
    i128 width = i128_of(ik_size(k) * 8);
    int left;
    uint shift;
    if (hw) {
        /* The hardware uses the low 5 bits of a 32-bit shift's amount and
         * the low 6 bits of a 64-bit one's. */
        left = op == OP_SHL;
        shift = (uint)i128_and(b, i128_sub(width, i128_of(1))).lo;
    } else {
        left = op == OP_SHL ? !i128_neg_p(b) : i128_neg_p(b);
        i128 count = i128_min(i128_abs(b), i128_sub(width, i128_of(1)));
        shift = (uint)count.lo;
    }
    i128 v;
    switch (op) {
    case OP_ADD:
        v = i128_add(a, b);
        break;
    case OP_SUB:
        v = i128_sub(a, b);
        break;
    case OP_MUL:
        v = i128_mul(a, b);
        break;
    case OP_DIV:
    case OP_REM:
        if (i128_is_zero(b)) {
            if (evaluated) {
                return 0;
            }
            v = i128_of(0);
        } else {
            v = op == OP_DIV ? i128_div(a, b) : i128_rem(a, b);
        }
        break;
    case OP_SHL:
    case OP_SHR:
        /* a is sign-extended when k is signed (an arithmetic shift) and not
         * negative when it is unsigned (a logical one). */
        v = left ? i128_shl(a, shift) : i128_shr(a, shift);
        break;
    case OP_BITAND:
        v = i128_and(a, b);
        break;
    case OP_BITOR:
        v = i128_or(a, b);
        break;
    case OP_BITXOR:
        v = i128_xor(a, b);
        break;
    case OP_EQ:
        return cint(out, i128_of(i128_eq(a, b)), IK_INT);
    case OP_NE:
        return cint(out, i128_of(!i128_eq(a, b)), IK_INT);
    case OP_LT:
        return cint(out, i128_of(i128_lt(a, b)), IK_INT);
    case OP_LE:
        return cint(out, i128_of(i128_le(a, b)), IK_INT);
    case OP_GT:
        return cint(out, i128_of(i128_lt(b, a)), IK_INT);
    case OP_GE:
        return cint(out, i128_of(i128_le(b, a)), IK_INT);
    default:
        die("consteval: && and || short-circuit");
        return 0;
    }
    return cint(out, ik_wrap(k, v), k);
}

static int addr_of(struct expr *e, int evaluated, int checked, int hw, struct cval *out);

/* merlin's value: the constant of e, an operand `evaluated` or not, in a
 * tree sema has `checked` or not. C99 6.6p3 and p11 hold a constant
 * expression to what it evaluates: in `1 ? 2 : 1 / 0` the third operand is
 * never divided, only its type counts. */
static int value(struct expr *e, int evaluated, int checked, int hw, struct cval *out) {
    struct cval l;
    struct cval r;
    i128 v;
    int k;
    switch (e->kind) {
    case E_NUM:
    case E_ENUMERATOR:
        k = ty_int_kind(e->ty);
        return k >= 0 && cint(out, e->value, k);
    case E_SIZEOF_TYPE: {
        int64_t s = ty_size(e->of);
        return s >= 0 && cint(out, i128_of(s), ik_size_t());
    }
    case E_UNARY:
        if (!value(e->x, evaluated, checked, hw, &l)) {
            return 0;
        }
        /* An address is true: an object's is never null. */
        if (e->op == UOP_NOT) {
            return cint(out, i128_of(!const_truth(&l)), IK_INT);
        }
        if (!int_of(&l, &v, &k)) {
            return 0;
        }
        k = ik_promoted(k);
        if (e->op == UOP_NEG) {
            return cint(out, ik_wrap(k, i128_neg(v)), k);
        }
        if (e->op == UOP_BITNOT) {
            return cint(out, ik_wrap(k, i128_not(v)), k);
        }
        return cint(out, v, k);
    case E_BINARY:
        if (e->op == OP_LOGAND || e->op == OP_LOGOR) {
            if (!value(e->x, evaluated, checked, hw, &l)) {
                return 0;
            }
            int lt = const_truth(&l);
            if (e->op == OP_LOGAND && !lt) {
                return cint(out, i128_of(0), IK_INT);
            }
            if (e->op == OP_LOGOR && lt) {
                return cint(out, i128_of(1), IK_INT);
            }
            if (!value(e->y, evaluated, checked, hw, &r)) {
                return 0;
            }
            return cint(out, i128_of(const_truth(&r)), IK_INT);
        }
        if (!value(e->x, evaluated, checked, hw, &l) || !value(e->y, evaluated, checked, hw, &r)) {
            return 0;
        }
        if (!l.is_addr && !r.is_addr) {
            return binary(e->op, l.v, l.ik, r.v, r.ik, evaluated, hw, out);
        }
        /* Address arithmetic: a pointer plus or minus an integer, by whole
         * elements, or an address converted to an integer plus or minus
         * one, by bytes. */
        if (l.is_addr && !r.is_addr && (e->op == OP_ADD || e->op == OP_SUB)) {
            i128 size;
            if (!step_of(e->x->ty, &size)) {
                return 0;
            }
            i128 n = e->op == OP_SUB ? i128_neg(r.v) : r.v;
            *out = l;
            out->offset = i128_add(l.offset, i128_mul(n, size));
            return 1;
        }
        if (!l.is_addr && r.is_addr && e->op == OP_ADD) {
            i128 size;
            if (!step_of(e->y->ty, &size)) {
                return 0;
            }
            *out = r;
            out->offset = i128_add(r.offset, i128_mul(l.v, size));
            return 1;
        }
        return 0;
    case E_COND: {
        /* Only the chosen operand is evaluated; of the other only the type
         * counts: sema's, or before sema what its value has. */
        if (!value(e->x, evaluated, checked, hw, &l)) {
            return 0;
        }
        int truth = const_truth(&l);
        struct expr *chosen = truth ? e->y : e->z;
        struct expr *other = truth ? e->z : e->y;
        struct cval c;
        if (!value(chosen, evaluated, checked, hw, &c)) {
            return 0;
        }
        int ok;
        if (checked) {
            k = ty_int_kind(other->ty);
            ok = k >= 0;
        } else {
            struct cval o;
            if (!value(other, 0, 0, hw, &o)) {
                return 0;
            }
            ok = int_of(&o, &v, &k);
        }
        if (!c.is_addr && ok) {
            /* The operands' common type, whichever is chosen. */
            int ck = ik_common(c.ik, k);
            return cint(out, ik_wrap(ck, c.v), ck);
        }
        *out = c;
        return 1;
    }
    case E_CAST: {
        if (!value(e->x, evaluated, checked, hw, &l)) {
            return 0;
        }
        struct type *t = ty_unqual(e->ty);
        if (t->kind == TY_INT) {
            k = t->ik;
            if (!l.is_addr) {
                return cint(out, ik_wrap(k, l.v), k);
            }
            /* A non-null address is true. */
            if (k == IK_BOOL && l.base != BASE_NONE) {
                return cint(out, i128_of(1), IK_BOOL);
            }
            /* A plain number used as an address converts back: offsetof's
             * (unsigned)&((struct s *)0)->field is a constant. */
            if (l.base == BASE_NONE) {
                return cint(out, ik_wrap(k, l.offset), k);
            }
            /* An address converted to an integer as wide as a pointer stays
             * one; to any other width it is not a constant. */
            if (ik_size(k) == PTR_SIZE) {
                *out = l;
                return 1;
            }
            return 0;
        }
        if (t->kind == TY_PTR) {
            if (!l.is_addr) {
                /* An address is a pointer-sized unsigned number. */
                memset(out, 0, sizeof *out);
                out->is_addr = 1;
                out->base = BASE_NONE;
                out->offset = ik_wrap(IK_ULONG, l.v);
                return 1;
            }
            *out = l;
            return 1;
        }
        return 0;
    }
    case E_STR:
    case E_VAR:
    case E_DEREF:
    case E_MEMBER:
        /* Addresses, in a checked tree: an array or a function stands for
         * its address; any other object's value is not constant. */
        if (ty_is_array(e->ty) || ty_is_func(e->ty)) {
            return addr_of(e, evaluated, checked, hw, out);
        }
        return 0;
    case E_ADDR:
        return addr_of(e->x, evaluated, checked, hw, out);
    default:
        return 0;
    }
}

/* The address of an lvalue, if it is an address constant: a global object
 * (or a member or element of one, at a constant offset), a string literal,
 * or what a constant pointer points to. */
static int addr_of(struct expr *e, int evaluated, int checked, int hw, struct cval *out) {
    struct cval c;
    switch (e->kind) {
    case E_VAR:
        if (e->var.kind != VAR_GLOBAL) {
            return 0;
        }
        memset(out, 0, sizeof *out);
        out->is_addr = 1;
        out->base = BASE_SYM;
        out->sym = e->var.sym;
        return 1;
    case E_STR:
        memset(out, 0, sizeof *out);
        out->is_addr = 1;
        out->base = BASE_STR;
        out->bytes = e->bytes;
        out->len = e->len;
        return 1;
    case E_DEREF:
        if (!value(e->x, evaluated, checked, hw, &c) || !c.is_addr) {
            return 0;
        }
        *out = c;
        return 1;
    case E_MEMBER: {
        int ok = e->flag ? value(e->x, evaluated, checked, hw, &c) : addr_of(e->x, evaluated, checked, hw, &c);
        if (!ok || !c.is_addr) {
            return 0;
        }
        *out = c;
        out->offset = i128_add(c.offset, i128_of(e->offset));
        return 1;
    }
    default:
        return 0;
    }
}

/* ---- What clang's evaluator notes ---- */

/* merlin's exact_overflow: where e (its operator) overflows its signed type,
 * as clang's evaluator finds it (CheckedIntArithmetic: -Winteger-overflow):
 * a +, - or * of constant operands, or a unary - of one, whose exact result
 * the type cannot hold; with division, a / or % of the type's least value
 * by -1 too. The exact result, and the type. */
static int exact_overflow(struct expr *e, int checked, int division, i128 *exact, int *kind) {
    struct cval l;
    struct cval r;
    i128 a;
    i128 b;
    int ak;
    int bk;
    int k;
    i128 x;
    if (e->kind == E_BINARY) {
        if (!value(e->x, 1, checked, 0, &l) || !int_of(&l, &a, &ak) || !value(e->y, 1, checked, 0, &r) ||
            !int_of(&r, &b, &bk)) {
            return 0;
        }
        k = ik_common(ak, bk);
        a = ik_wrap(k, a);
        b = ik_wrap(k, b);
        if (e->op == OP_ADD) {
            x = i128_add(a, b);
        } else if (e->op == OP_SUB) {
            x = i128_sub(a, b);
        } else if (e->op == OP_MUL) {
            x = i128_mul(a, b);
        } else if ((e->op == OP_DIV || e->op == OP_REM) && division && i128_eq(b, i128_of(-1))) {
            x = i128_neg(a);
        } else {
            return 0;
        }
    } else if (e->kind == E_UNARY && e->op == UOP_NEG) {
        if (!value(e->x, 1, checked, 0, &l) || !int_of(&l, &a, &k)) {
            return 0;
        }
        k = ik_promoted(k);
        x = i128_neg(ik_wrap(k, a));
    } else {
        return 0;
    }
    if (ik_signed(k) && (i128_lt(x, ik_min(k)) || i128_lt(ik_max(k), x))) {
        *exact = x;
        *kind = k;
        return 1;
    }
    return 0;
}

int consteval_overflow_at(struct expr *e, int checked, int division, i128 *wrapped, int *kind) {
    i128 exact;
    if (!exact_overflow(e, checked, division, &exact, kind)) {
        return 0;
    }
    *wrapped = ik_wrap(*kind, exact);
    return 1;
}

/* The value of an integer operand, as note sees it. */
static int note_int(struct expr *e, int checked, i128 *v, int *k) {
    struct cval c;
    return value(e, 1, checked, 0, &c) && int_of(&c, v, k);
}

/* merlin's note: what clang's evaluator notes first of e as making it no
 * integer constant expression, though it has a value, and where: a signed
 * overflow, a shift by a negative count or by the width or more, and a <<
 * of a negative value or that shifts set bits out of a signed type. Only
 * what is evaluated counts; operands before their operator. */
static int note(struct expr *e, int checked, struct pos *at, char **msg) {
    i128 a;
    i128 b;
    int ak;
    int bk;
    switch (e->kind) {
    case E_BINARY:
        if (e->op == OP_LOGAND || e->op == OP_LOGOR) {
            if (note(e->x, checked, at, msg)) {
                return 1;
            }
            if (note_int(e->x, checked, &a, &ak) && (!i128_is_zero(a)) == (e->op == OP_LOGAND)) {
                return note(e->y, checked, at, msg);
            }
            return 0;
        }
        if (note(e->x, checked, at, msg) || note(e->y, checked, at, msg)) {
            return 1;
        }
        {
            i128 exact;
            int k;
            if (exact_overflow(e, checked, 1, &exact, &k)) {
                *at = e->pos;
                *msg = fmt("value %s is outside the range of representable values of type '%s'", i128_str(exact),
                           ty_str(ty_int(k)));
                return 1;
            }
        }
        if (!note_int(e->x, checked, &a, &ak) || !note_int(e->y, checked, &b, &bk)) {
            return 0;
        }
        {
            int k = ik_promoted(ak);
            i128 width = i128_of(ik_size(k) * 8);
            char *ty = ty_str(ty_int(k));
            int shift = e->op == OP_SHL || e->op == OP_SHR;
            if (shift && i128_neg_p(b)) {
                *msg = fmt("negative shift count %s", i128_str(b));
            } else if (shift && i128_le(width, b)) {
                *msg = fmt("shift count %s >= width of type '%s' (%s bits)", i128_str(b), ty, i128_str(width));
            } else if (e->op == OP_SHL && ik_signed(k)) {
                /* The bits above the value's that the shift keeps. */
                a = ik_wrap(k, a);
                i128 used = i128_of(128 - (int64_t)i128_leading_zeros(a));
                if (i128_neg_p(a)) {
                    *msg = fmt("left shift of negative value %s", i128_str(a));
                } else if (i128_lt(i128_sub(width, used), b)) {
                    *msg = "signed left shift discards bits";
                } else {
                    return 0;
                }
            } else {
                return 0;
            }
            *at = e->pos;
            return 1;
        }
    case E_UNARY:
        if (note(e->x, checked, at, msg)) {
            return 1;
        }
        {
            i128 exact;
            int k;
            if (exact_overflow(e, checked, 1, &exact, &k)) {
                *at = e->pos;
                *msg = fmt("value %s is outside the range of representable values of type '%s'", i128_str(exact),
                           ty_str(ty_int(k)));
                return 1;
            }
        }
        return 0;
    case E_COND:
        if (note(e->x, checked, at, msg)) {
            return 1;
        }
        if (!note_int(e->x, checked, &a, &ak)) {
            return 0;
        }
        return note(!i128_is_zero(a) ? e->y : e->z, checked, at, msg);
    case E_CAST:
        return note(e->x, checked, at, msg);
    default:
        return 0;
    }
}

int consteval_parsed_note(struct expr *e, struct pos *at, char **msg) {
    return note(e, 0, at, msg);
}

int consteval_note(struct expr *e, int checked, struct pos *at, char **msg) {
    return note(e, checked, at, msg);
}

/* merlin's why_not: why e, a checked tree, is no integer constant
 * expression, as clang's evaluator notes it: of a division by zero it
 * reaches, `division by zero` at the operator. */
int consteval_why_not(struct expr *e, struct pos *at, const char **msg) {
    i128 v;
    switch (e->kind) {
    case E_BINARY:
        if (e->op == OP_LOGAND || e->op == OP_LOGOR) {
            if (consteval_why_not(e->x, at, msg)) {
                return 1;
            }
            if (!consteval_int(e->x, &v)) {
                return 0;
            }
            if ((!i128_is_zero(v)) == (e->op == OP_LOGAND)) {
                return consteval_why_not(e->y, at, msg);
            }
            return 0;
        }
        if (consteval_why_not(e->x, at, msg)) {
            return 1;
        }
        if (!consteval_int(e->x, &v)) {
            return 0;
        }
        if (consteval_why_not(e->y, at, msg)) {
            return 1;
        }
        if ((e->op == OP_DIV || e->op == OP_REM) && consteval_int(e->y, &v) && i128_is_zero(v)) {
            *at = e->pos;
            *msg = "division by zero";
            return 1;
        }
        return 0;
    case E_COND:
        if (consteval_why_not(e->x, at, msg)) {
            return 1;
        }
        if (!consteval_int(e->x, &v)) {
            return 0;
        }
        return consteval_why_not(!i128_is_zero(v) ? e->y : e->z, at, msg);
    case E_UNARY:
    case E_CAST:
        return consteval_why_not(e->x, at, msg);
    default:
        return 0;
    }
}

/* ---- Conditions ----
 * The value clang's code generator folds a condition to at -O0
 * (ConstantFoldsToSimpleInteger): an integer constant expression's value,
 * and what is known of where objects are, never null and never one
 * another, as clang 23 folds it (merlin's condition, whose comment lists
 * the rules). */

enum { PLACE_SYM, PLACE_LOCAL, PLACE_STR, PLACE_LITERAL };

/* An object whose address is known: what it is, and its size. */
struct object {
    int what;
    const char *sym;
    uint local;
    uchar *bytes;
    uint len;
    struct expr *literal;
    int has_size;
    i128 size;
    int incomplete;
    int zero_sized;
};

/* An integer, or an address in an object (none: a plain number used as an
 * address) at an offset in bytes, with what it designates: the bytes
 * [lo, hi] of its subobject, or nothing (has_d 0). */
struct folded {
    int is_addr;
    i128 v;
    int ik;
    int has_obj;
    struct object obj;
    i128 at;
    int has_d;
    i128 lo;
    i128 hi;
};

static int object_eq(struct object *a, struct object *b) {
    if (a->what != b->what || a->has_size != b->has_size || a->incomplete != b->incomplete ||
        a->zero_sized != b->zero_sized) {
        return 0;
    }
    if (a->has_size && !i128_eq(a->size, b->size)) {
        return 0;
    }
    switch (a->what) {
    case PLACE_SYM:
        return strcmp(a->sym, b->sym) == 0;
    case PLACE_LOCAL:
        return a->local == b->local;
    case PLACE_STR:
        return a->len == b->len && memcmp(a->bytes, b->bytes, a->len) == 0;
    default:
        return a->literal == b->literal;
    }
}

static int fint(struct folded *f, i128 v, int k) {
    memset(f, 0, sizeof *f);
    f->v = v;
    f->ik = k;
    return 1;
}

static int ftruth(struct folded *f) {
    if (!f->is_addr) {
        return !i128_is_zero(f->v);
    }
    if (f->has_obj) {
        return 1;
    }
    return !i128_is_zero(f->at);
}

/* An address moved by `by` bytes: it designates what it did while it stays
 * in it, up to just past its end. */
static int moved(struct folded *a, i128 by, struct folded *out) {
    *out = *a;
    out->at = i128_add(a->at, by);
    if (out->has_d && !(i128_le(a->lo, out->at) && i128_le(out->at, a->hi))) {
        out->has_d = 0;
    }
    return 1;
}

static int fold(struct expr *e, struct folded *out);
static int fold_place(struct expr *e, struct folded *out);

/* Whether string literals l and r, at offsets at and bt in them, could be
 * at one address: where the two would overlap, their bytes (the NULs too)
 * are the same. */
static int str_byte(uchar *s, uint len, i128 i, int *b) {
    if (i128_neg_p(i)) {
        return 0;
    }
    if (i128_lt(i, i128_of(len))) {
        *b = s[i.lo];
        return 1;
    }
    if (i128_eq(i, i128_of(len))) {
        *b = 0;
        return 1;
    }
    return 0;
}

static int overlap(struct object *l, i128 at, struct object *r, i128 bt) {
    i128 from = i128_neg(i128_min(at, bt));
    i128 to = i128_min(i128_sub(i128_of((int64_t)l->len + 1), at), i128_sub(i128_of((int64_t)r->len + 1), bt));
    for (i128 d = from; i128_lt(d, to); d = i128_add(d, i128_of(1))) {
        int x;
        int y;
        int hx = str_byte(l->bytes, l->len, i128_add(at, d), &x);
        int hy = str_byte(r->bytes, r->len, i128_add(bt, d), &y);
        if (hx != hy || (hx && x != y)) {
            return 0;
        }
    }
    return 1;
}

/* Just past the end: what is known of an address that designates something,
 * by its offset; of one that does not, only that it may be if its object's
 * size is unknown. */
static int past(struct object *o, i128 at, int has_d) {
    if (has_d) {
        return o->has_size && i128_eq(o->size, at);
    }
    return o->incomplete;
}

/* a op b of two addresses, l the type of the left one, as clang's
 * EvaluateComparisonBinaryOperator decides it. */
static int compare(int op, struct folded *a, struct folded *b, struct type *l, struct folded *out) {
    i128 at = a->at;
    i128 bt = b->at;
    /* Two string literals may share their bytes, or not. */
    if (a->has_obj && b->has_obj && a->obj.what == PLACE_STR && b->obj.what == PLACE_STR) {
        if (overlap(&a->obj, at, &b->obj, bt)) {
            return 0;
        }
        if (op == OP_EQ) {
            return fint(out, i128_of(0), IK_INT);
        }
        if (op == OP_NE) {
            return fint(out, i128_of(1), IK_INT);
        }
        return 0;
    }
    int same;
    if (a->has_obj && b->has_obj) {
        same = object_eq(&a->obj, &b->obj);
    } else if (!a->has_obj && !b->has_obj) {
        same = 1;
    } else {
        /* An object's address is not null; nor any plain number but 0. */
        int null = !a->has_obj ? i128_is_zero(at) : i128_is_zero(bt);
        if (op == OP_EQ && null) {
            return fint(out, i128_of(0), IK_INT);
        }
        if (op == OP_NE && null) {
            return fint(out, i128_of(1), IK_INT);
        }
        return 0;
    }
    if (same) {
        /* Ordered only inside the object, or just past its end. */
        int ordered = 1;
        if (a->has_obj) {
            struct object *o = &a->obj;
            ordered = o->has_size && !i128_neg_p(at) && i128_le(at, o->size) && !i128_neg_p(bt) &&
                      i128_le(bt, o->size);
        }
        switch (op) {
        case OP_EQ:
            return fint(out, i128_of(i128_eq(at, bt)), IK_INT);
        case OP_NE:
            return fint(out, i128_of(!i128_eq(at, bt)), IK_INT);
        case OP_LT:
            return ordered && fint(out, i128_of(i128_lt(at, bt)), IK_INT);
        case OP_LE:
            return ordered && fint(out, i128_of(i128_le(at, bt)), IK_INT);
        case OP_GT:
            return ordered && fint(out, i128_of(i128_lt(bt, at)), IK_INT);
        case OP_GE:
            return ordered && fint(out, i128_of(i128_le(bt, at)), IK_INT);
        case OP_SUB: {
            i128 s;
            if (!step_of(l, &s)) {
                return 0;
            }
            return fint(out, i128_div(i128_sub(at, bt), s), ik_ptrdiff_t());
        }
        default:
            return 0;
        }
    }
    /* Two objects: never at one address, unless one may hold nothing at all,
     * or one points just past its end and the other at its start. */
    if (a->obj.zero_sized || b->obj.zero_sized) {
        return 0;
    }
    if ((i128_is_zero(bt) && past(&a->obj, at, a->has_d)) || (i128_is_zero(at) && past(&b->obj, bt, b->has_d))) {
        return 0;
    }
    if (op == OP_EQ) {
        return fint(out, i128_of(0), IK_INT);
    }
    if (op == OP_NE) {
        return fint(out, i128_of(1), IK_INT);
    }
    return 0;
}

static int fold(struct expr *e, struct folded *out) {
    /* An integer constant expression, as everywhere. */
    struct cval c;
    if (consteval_eval(e, &c) && !c.is_addr) {
        return fint(out, c.v, c.ik);
    }
    struct folded l;
    struct folded r;
    switch (e->kind) {
    case E_UNARY: {
        if (!fold(e->x, &l)) {
            return 0;
        }
        if (e->op == UOP_NOT) {
            return fint(out, i128_of(!ftruth(&l)), IK_INT);
        }
        if (l.is_addr) {
            return 0;
        }
        int k = ik_promoted(l.ik);
        if (e->op == UOP_NEG) {
            return fint(out, ik_wrap(k, i128_neg(l.v)), k);
        }
        if (e->op == UOP_BITNOT) {
            return fint(out, ik_wrap(k, i128_not(l.v)), k);
        }
        return fint(out, l.v, k);
    }
    case E_BINARY:
        if (e->op == OP_LOGAND || e->op == OP_LOGOR) {
            /* Short-circuits: what the left operand decides is enough. */
            if (!fold(e->x, &l)) {
                return 0;
            }
            int lt = ftruth(&l);
            int v;
            if (lt == (e->op == OP_LOGOR)) {
                v = lt;
            } else {
                if (!fold(e->y, &r)) {
                    return 0;
                }
                v = ftruth(&r);
            }
            return fint(out, i128_of(v), IK_INT);
        }
        if (!fold(e->x, &l) || !fold(e->y, &r)) {
            return 0;
        }
        if (!l.is_addr && !r.is_addr) {
            if (!binary(e->op, l.v, l.ik, r.v, r.ik, 1, 0, &c) || c.is_addr) {
                return 0;
            }
            return fint(out, c.v, c.ik);
        }
        /* A pointer moved by whole elements. */
        if (l.is_addr && !r.is_addr && (e->op == OP_ADD || e->op == OP_SUB)) {
            i128 s;
            if (!step_of(e->x->ty, &s)) {
                return 0;
            }
            i128 n = e->op == OP_SUB ? i128_neg(r.v) : r.v;
            return moved(&l, i128_mul(n, s), out);
        }
        if (!l.is_addr && r.is_addr && e->op == OP_ADD) {
            i128 s;
            if (!step_of(e->y->ty, &s)) {
                return 0;
            }
            return moved(&r, i128_mul(l.v, s), out);
        }
        if (l.is_addr && r.is_addr) {
            return compare(e->op, &l, &r, e->x->ty, out);
        }
        /* An address compared with a null pointer constant, &g != 0. */
        if (l.is_addr) {
            struct folded n;
            memset(&n, 0, sizeof n);
            n.is_addr = 1;
            n.at = r.v;
            return compare(e->op, &l, &n, e->x->ty, out);
        }
        {
            struct folded n;
            memset(&n, 0, sizeof n);
            n.is_addr = 1;
            n.at = l.v;
            return compare(e->op, &n, &r, e->y->ty, out);
        }
    case E_COND: {
        if (!fold(e->x, &l)) {
            return 0;
        }
        struct folded v;
        if (!fold(ftruth(&l) ? e->y : e->z, &v)) {
            return 0;
        }
        int k = ty_int_kind(e->ty);
        if (!v.is_addr && k >= 0) {
            return fint(out, ik_wrap(k, v.v), k);
        }
        *out = v;
        return 1;
    }
    case E_CAST: {
        if (!fold(e->x, &l)) {
            return 0;
        }
        struct type *t = ty_unqual(e->ty);
        if (t->kind == TY_INT && t->ik == IK_BOOL) {
            return fint(out, i128_of(ftruth(&l)), IK_BOOL);
        }
        if (t->kind == TY_INT && !l.is_addr) {
            return fint(out, ik_wrap(t->ik, l.v), t->ik);
        }
        /* A plain number used as an address is that number again; an
         * object's address as a number is not known. */
        if (t->kind == TY_INT && !l.has_obj) {
            return fint(out, ik_wrap(t->ik, l.at), t->ik);
        }
        if (t->kind == TY_PTR && !l.is_addr) {
            memset(out, 0, sizeof *out);
            out->is_addr = 1;
            out->at = ik_wrap(IK_ULONG, l.v);
            return 1;
        }
        /* A pointer cast to another pointer type designates nothing any
         * more (clang's CK_BitCast), but to void * or to a pointer to a
         * similar type (CK_NoOp). */
        if (t->kind == TY_PTR) {
            struct type *to = t->to;
            struct type *from = ty_unqual(e->x->ty);
            int keeps = ty_is_void(to);
            if (!keeps) {
                if (from->kind == TY_PTR || from->kind == TY_ARRAY) {
                    keeps = ty_similar(from->to, to);
                } else if (from->kind == TY_FUNC) {
                    keeps = ty_similar(from, to);
                } else {
                    keeps = 1;
                }
            }
            *out = l;
            if (!keeps) {
                out->has_d = 0;
            }
            return 1;
        }
        return 0;
    }
    case E_STR:
    case E_VAR:
    case E_DEREF:
    case E_MEMBER:
    case E_COMPOUND:
        /* An array or a function stands for its address. */
        if (ty_is_array(e->ty) || ty_is_func(e->ty)) {
            return fold_place(e, out);
        }
        return 0;
    case E_ADDR:
        return fold_place(e->x, out);
    default:
        return 0;
    }
}

/* An object's address: offset 0, designating the whole object. */
static int object_at(struct expr *e, int what, struct folded *out) {
    int64_t s = ty_size(e->ty);
    memset(out, 0, sizeof *out);
    out->is_addr = 1;
    out->has_obj = 1;
    out->obj.what = what;
    out->obj.has_size = s >= 0;
    out->obj.size = i128_of(s >= 0 ? s : 0);
    /* An incomplete type has no size; a function has none either. */
    out->obj.incomplete = s < 0 && !ty_is_func(e->ty);
    out->obj.zero_sized = out->obj.incomplete && ty_is_array(e->ty);
    out->has_d = 1;
    out->lo = i128_of(0);
    out->hi = i128_of(s >= 0 ? s : 0);
    return 1;
}

/* The address of an lvalue, if it is known. */
static int fold_place(struct expr *e, struct folded *out) {
    int64_t size = ty_size(e->ty);
    struct folded a;
    switch (e->kind) {
    case E_VAR:
        if (e->var.kind == VAR_GLOBAL) {
            object_at(e, PLACE_SYM, out);
            out->obj.sym = e->var.sym;
            return 1;
        }
        if (e->var.kind == VAR_LOCAL) {
            object_at(e, PLACE_LOCAL, out);
            out->obj.local = e->var.local;
            return 1;
        }
        return 0;
    case E_STR:
        object_at(e, PLACE_STR, out);
        out->obj.bytes = e->bytes;
        out->obj.len = e->len;
        return 1;
    case E_COMPOUND:
        /* A compound literal is an object, never null. */
        if (e->local >= 0) {
            object_at(e, PLACE_LOCAL, out);
            out->obj.local = (uint)e->local;
        } else {
            object_at(e, PLACE_LITERAL, out);
            out->obj.literal = e;
        }
        return 1;
    case E_DEREF:
        if (!fold(e->x, &a) || !a.is_addr) {
            return 0;
        }
        *out = a;
        /* An array element that is an array itself is the subobject. */
        if (ty_is_array(e->ty)) {
            if (out->has_d && size >= 0) {
                out->lo = a.at;
                out->hi = i128_add(a.at, i128_of(size));
            } else {
                out->has_d = 0;
            }
        }
        return 1;
    case E_MEMBER: {
        int ok = e->flag ? fold(e->x, &a) : fold_place(e->x, &a);
        if (!ok || !a.is_addr) {
            return 0;
        }
        *out = a;
        out->at = i128_add(a.at, i128_of(e->offset));
        /* A member is the subobject designated. */
        if (out->has_d && size >= 0) {
            out->lo = out->at;
            out->hi = i128_add(out->at, i128_of(size));
        } else {
            out->has_d = 0;
        }
        return 1;
    }
    default:
        return 0;
    }
}

/* merlin's cfg_condition: the condition as clang's CFG builder evaluates
 * it, folding what has undefined behaviour too. */
int consteval_cfg_condition(struct expr *e, i128 *v) {
    if (!ty_is_integer(e->ty)) {
        return 0;
    }
    struct folded f;
    if (!fold(e, &f) || f.is_addr) {
        return 0;
    }
    *v = f.v;
    return 1;
}

/* merlin's condition: what has undefined behaviour clang's code generator
 * does not fold, both ways kept, what the instructions give deciding. */
int consteval_condition(struct expr *e, i128 *v) {
    struct pos at;
    char *msg;
    if (note(e, 1, &at, &msg)) {
        return 0;
    }
    return consteval_cfg_condition(e, v);
}
