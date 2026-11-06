/* cc1: tokens to the syntax tree, by recursive descent (follows merlin's
 * parser.rs, whose module comment has the grammar). It stops at the first
 * error, which is clang's, in its words and where it puts it (`expected
 * ';' after expression`, after the expression; `expected ')'`, at the
 * token there instead, with clang's note `to match this '('`). What clang
 * only warns of, the parser warns of too, and goes on.
 *
 * Binary precedence (precedence climbing), loosest first: || 1, && 2, | 3,
 * ^ 4, & 5, == != 6, < <= > >= 7, << >> 8, + - 9, * / % 10. All
 * left-associative.
 *
 * The parser already knows some types: integer literals carry theirs from
 * the lexer, a string literal is char[len + 1], a cast has its target
 * type, and array sizes are evaluated here (consteval.c), so declared types
 * are complete. a[i] becomes *(a + i), which is what C defines it to mean.
 *
 * C cannot be parsed without knowing which identifiers are typedef names:
 * `T * x;` declares a pointer when T is a type and multiplies otherwise. So
 * the parser keeps block scopes of its own for what it needs to resolve:
 * typedef names (replaced by their types), enumeration constants (replaced
 * by their values, so `int a[N]` works) and struct, union and enum tags.
 * Variables and functions are recorded only as names that hide an outer
 * typedef; sema resolves them.
 *
 * A declarator is read inside out, as C defines it: declarator_ops collects
 * its steps (struct derive), from the base type to the declared one, and
 * declarator applies them.
 *
 * Errors: a function that can fail returns the error (struct diag *), NULL
 * when it succeeds, what it reads in its out arguments; TRY passes an error
 * on, as Rust's `?`. */
#include "cc.h"

#define TRY(x)                                                                                               \
    do {                                                                                                     \
        struct diag *try_ = (x);                                                                             \
        if (try_ != NULL) {                                                                                  \
            return try_;                                                                                     \
        }                                                                                                    \
    } while (0)

/* clang's warning for an array size it folds, and its flag. */
static const char FOLDED_MSG[] = "variable length array folded to constant array as an extension";
static const char FOLDED_FLAG[] = "gnu-folding-constant";

/* The error of an array size that is no integer constant expression. */
static const char NOT_CONSTANT_SIZE[] = "array size must be an integer constant expression "
                                        "(literals, enum constants, operators, casts and sizeof)";

/* Where a declaration appears: what storage classes and declarators it may
 * have. */
enum { CTX_FILE, CTX_BLOCK, CTX_FOR_INIT, CTX_PARAM, CTX_MEMBER, CTX_TYPE_NAME };

/* The storage class a declaration's specifiers give. */
enum { CLASS_NONE, CLASS_STATIC, CLASS_EXTERN, CLASS_TYPEDEF, CLASS_REGISTER, CLASS_AUTO };

/* What a list of declaration specifiers says (merlin's Specs). */
struct specs {
    struct type *ty;
    int class;
    /* Where the storage class is (where the specifiers start, if none is). */
    struct pos class_pos;
    /* Where `inline` is, if it is there. */
    int has_inline;
    struct pos inline_pos;
    /* They declare something by themselves (a tag, enumeration constants). */
    int declares;
    /* They define a struct or union without a tag. */
    int anonymous;
    struct spelling *spelled;
    struct pos pos;
};

/* What an ordinary identifier means to the parser (merlin's Name). */
enum { NAME_TYPEDEF, NAME_ENUM_CONST, NAME_OBJECT };
struct name {
    int kind;
    /* NAME_TYPEDEF: what it stands for; NAME_OBJECT: its declared type. */
    struct type *ty;
    /* NAME_TYPEDEF: the name as a spelling; NAME_ENUM_CONST: its enum, once
     * the enum's definition is complete. */
    struct spelling *spelled;
    /* NAME_ENUM_CONST. */
    i128 value;
    /* Where it is declared. */
    struct pos pos;
};

/* What a struct, union or enum tag names. */
enum { TAG_RECORD, TAG_ENUM };
struct tag {
    int kind;
    struct record *rec;
    /* TAG_ENUM: the integer type it is, the enum as a spelling, where its
     * tag is. */
    struct type *ty;
    struct spelling *spelled;
    struct pos pos;
};

/* One block scope: the file, a function's parameters and body, a block, a
 * `for` statement, or a parameter list. */
struct scope {
    /* name -> struct name *. */
    struct map names;
    /* tag -> struct tag *. */
    struct map tags;
    /* The names a block declares with linkage (any non-NULL value). */
    struct map linked;
};

/* Where the array sizes a declarator folded are, seen from what it
 * declares, from the best to the worst (merlin's Reach). */
enum { REACH_ARRAYS, REACH_POINTERS, REACH_OTHER, REACH_FUNCTION };

/* One step of a declarator, from the base type towards the declared type
 * (merlin's Derive). */
enum { D_PTR, D_ARRAY, D_FUNC };
struct derive {
    int kind;
    /* D_PTR: the pointer's qualifiers. */
    uint quals;
    /* D_ARRAY (merlin's Array): how many elements (none for []), where the
     * `[` and the size are, the qualifiers in the brackets and whether
     * `static` was there, where the size is if the parser folded it, which
     * of the waiting unevaluated expressions are its size's, and whether
     * the size is a constant. */
    int has_len;
    i128 len;
    struct pos open;
    struct pos size;
    int has_quals;
    uint array_quals;
    int is_static;
    int has_folded;
    struct pos folded;
    struct range entries;
    int constant;
    /* D_FUNC: the parameters, `...`, a prototype's list (not `()`), the
     * scope holding their names, and where its `(` is (open). */
    struct param *params;
    uint nparams;
    int variadic;
    int prototyped;
    struct scope *scope;
};

/* A parsed declarator (merlin's Decl). */
struct decl {
    /* "" when the declarator has no name. */
    const char *name;
    struct type *ty;
    struct pos pos;
    /* When it declares a function: its parameters and their scope. */
    int has_func;
    struct param *params;
    uint nparams;
    struct scope *scope;
    int prototyped;
    int has_open;
    struct pos open;
    /* Where the first array size of its own the parser folded is. */
    int has_folded;
    struct pos folded;
    int reach;
    /* Its folded sizes' unevaluated expressions (from its first): its own
     * array's, and the others'. */
    int has_top;
    struct range top;
    struct range *inner;
    uint ninner;
    /* Where its own unevaluated expressions are among those waiting. */
    struct range unevaluated;
};

/* A parenthesized expression in a _Static_assert's condition: its key (its
 * position and kind), and where its `(` is. */
struct paren {
    struct pos pos;
    int kind;
    struct pos open;
};

struct parser {
    struct token *toks;
    uint ntoks;
    uint pos;
    /* Current nesting level; see MAX_DEPTH. */
    uint depth;
    /* struct scope *, innermost last; the first is the file scope. */
    struct vec scopes;
    /* struct unevaluated *: what the constants evaluated so far have that
     * sema must still check, until their declaration or statement is
     * done. */
    struct vec unevaluated;
    /* struct proto_param *: the parameters declared so far in the lists
     * being parsed, outermost first. */
    struct vec prototype;
    /* The constant being evaluated folded an operand that is not constant. */
    int folding;
    /* struct vec * of statements: the blocks a syntax error cut short, the
     * innermost first. */
    struct vec salvaged;
    /* Where the `(` around the type name being read is, for the note of an
     * `expected ')'` inside it, if type_open_matched: not for
     * __builtin_va_arg's, whose `)` clang does not match with its `(`. */
    struct pos type_open;
    int type_open_matched;
    /* struct expr *: the names read as expressions that nothing declares. */
    struct vec undeclared;
    struct item *partial;
    /* While a _Static_assert's condition is parsed: struct paren *. */
    int in_parens;
    struct vec parens;
    /* struct diag *. */
    struct vec warnings;
    struct written written;
    /* The `else` of the `if` the statement just parsed ends with. */
    int has_trailing_else;
    struct pos trailing_else;
};

static int at_decl(struct parser *p);
static struct diag *initializer(struct parser *p, struct init **out);

/* ---- Typo correction ---- */

/* The characters of s (UTF-8), in *n of them. */
static uint *chars_of(const char *s, uint *n) {
    uint len = (uint)strlen(s);
    uint *out = heap_alloc((len + 1) * sizeof(uint));
    uint k = 0;
    for (uint i = 0; i < len;) {
        uint l;
        out[k++] = utf8_decode((const uchar *)s + i, len - i, &l);
        i += l;
    }
    *n = k;
    return out;
}

/* Levenshtein's distance of a and b, in characters. */
static uint edit_distance(const char *a, const char *b) {
    uint na;
    uint nb;
    uint *ca = chars_of(a, &na);
    uint *cb = chars_of(b, &nb);
    uint *row = heap_alloc((nb + 1) * sizeof(uint));
    for (uint j = 0; j <= nb; j++) {
        row[j] = j;
    }
    for (uint i = 0; i < na; i++) {
        uint prev = row[0];
        row[0] = i + 1;
        for (uint j = 0; j < nb; j++) {
            uint here = row[j + 1];
            uint best = prev + (ca[i] != cb[j] ? 1 : 0);
            if (row[j] + 1 < best) {
                best = row[j] + 1;
            }
            if (here + 1 < best) {
                best = here + 1;
            }
            row[j + 1] = best;
            prev = here;
        }
    }
    uint d = row[nb];
    heap_free(ca);
    heap_free(cb);
    heap_free(row);
    return d;
}

/* clang's typo correction (Sema::CorrectTypo) of typo among n candidates:
 * the index of the one closest by edit distance, -1 for none or if two are
 * as close. A name is a candidate only if the lengths differ by less than
 * a third of the typo's and the distance is at most a third of it, rounded
 * up; and the best is taken only if the typo is three times as long as the
 * distance or more (merlin's correction). */
int correction(const char *typo, const char **names, uint n) {
    uint len;
    heap_free(chars_of(typo, &len));
    uint bound = (len + 2) / 3;
    int best = -1;
    uint best_d = 0;
    int tied = 0;
    for (uint i = 0; i < n; i++) {
        uint nlen;
        heap_free(chars_of(names[i], &nlen));
        uint diff = len > nlen ? len - nlen : nlen - len;
        if (diff > 0 && len / diff < 3) {
            continue;
        }
        uint d = edit_distance(typo, names[i]);
        if (d > bound || d == 0) {
            continue;
        }
        if (best >= 0 && d > best_d) {
            continue;
        }
        if (best >= 0 && d == best_d) {
            tied = 1;
            continue;
        }
        best = (int)i;
        best_d = d;
        tied = 0;
    }
    if (best < 0 || tied || len / best_d < 3) {
        return -1;
    }
    return best;
}

/* ---- Tokens ---- */

static struct diag *parse_enter(struct parser *p);

static struct token *tok(struct parser *p) {
    return &p->toks[p->pos < p->ntoks ? p->pos : p->ntoks - 1];
}

static struct token *peek_at(struct parser *p, uint n) {
    uint i = p->pos + n;
    return &p->toks[i < p->ntoks ? i : p->ntoks - 1];
}

static struct pos here(struct parser *p) {
    return tok(p)->pos;
}

/* Just past the previous token: where a missing `;` or `)` belongs. */
static struct pos after_prev(struct parser *p) {
    if (p->pos == 0) {
        return here(p);
    }
    return p->toks[p->pos - 1].end;
}

static struct token *advance(struct parser *p) {
    struct token *t = tok(p);
    if (p->pos < p->ntoks - 1) {
        p->pos++;
    }
    return t;
}

static int tok_punct(struct token *t, const char *s) {
    return t->kind == TK_PUNCT && strcmp(t->name, s) == 0;
}

static int tok_kw(struct token *t, int k) {
    return t->kind == TK_KW && t->kw == k;
}

static int is_punct(struct parser *p, const char *s) {
    return tok_punct(tok(p), s);
}

static int is_kw(struct parser *p, int k) {
    return tok_kw(tok(p), k);
}

static int is_eof(struct parser *p) {
    return tok(p)->kind == TK_EOF;
}

static int eat_punct(struct parser *p, const char *s) {
    if (is_punct(p, s)) {
        advance(p);
        return 1;
    }
    return 0;
}

static int eat_kw(struct parser *p, int k) {
    if (is_kw(p, k)) {
        advance(p);
        return 1;
    }
    return 0;
}

/* clang's error for what the current token is not: `expected expression`. */
static struct diag *expected(struct parser *p, const char *what) {
    return diag_error(here(p), fmt("expected %s", what));
}

static void warn(struct parser *p, struct diag *w) {
    vec_push(&p->warnings, w);
}

/* The punctuator s, or clang's error: at the token there instead for an
 * opening bracket, else right after the previous token (ExpectAndConsume). */
static struct diag *expect_punct(struct parser *p, const char *s, struct pos *at) {
    if (is_punct(p, s)) {
        struct token *t = advance(p);
        if (at != NULL) {
            *at = t->pos;
        }
        return NULL;
    }
    struct pos where = strcmp(s, "(") == 0 || strcmp(s, "[") == 0 || strcmp(s, "{") == 0 ? here(p) : after_prev(p);
    return diag_error(where, fmt("expected '%s'", s));
}

/* The bracket that closes the one at open, or clang's error at the token
 * that is there instead, with its note: what clang's
 * BalancedDelimiterTracker does where it closes a bracket it opened
 * (consumeClose, its note diagnoseMissingClose's). */
static struct diag *expect_close(struct parser *p, const char *close, struct pos open, struct pos *at) {
    if (is_punct(p, close)) {
        struct token *t = advance(p);
        if (at != NULL) {
            *at = t->pos;
        }
        return NULL;
    }
    /* A `;` right before the closing token: consumeClose calls it out
     * (`f(1;)`, `{1;}`), consumes the `;` and the bracket and goes on
     * (RAIIObjectsForParser.h). Where clang closes a bracket without the
     * tracker (__builtin_va_arg's), there is no such rule. */
    if (is_punct(p, ";") && tok_punct(peek_at(p, 1), close)) {
        return diag_error(here(p), fmt("unexpected ';' before '%s'", close));
    }
    const char *opener = strcmp(close, ")") == 0 ? "(" : strcmp(close, "]") == 0 ? "[" : "{";
    return diag_note(expected(p, fmt("'%s'", close)), open, fmt("to match this '%s'", opener));
}

static struct diag *expect_semi(struct parser *p, const char *after) {
    if (eat_punct(p, ";")) {
        return NULL;
    }
    return diag_error(after_prev(p), fmt("expected ';' %s", after));
}

static struct diag *expect_ident(struct parser *p, const char **name, struct pos *pos) {
    if (tok(p)->kind != TK_IDENT) {
        return expected(p, "identifier");
    }
    struct token *t = advance(p);
    *name = t->name;
    *pos = t->pos;
    return NULL;
}

/* ---- Nesting ---- */

/* One nesting level deeper, or the error at the current token. */
static struct diag *parse_enter(struct parser *p) {
    if (p->depth == MAX_DEPTH) {
        return diag_error(here(p), fmt("nesting too deep: merlin allows at most %u levels", MAX_DEPTH));
    }
    p->depth++;
    return NULL;
}

/* ---- Scopes ---- */

static struct scope *scope_new(void) {
    return arena_alloc(sizeof(struct scope));
}

static struct scope *innermost(struct parser *p) {
    return vec_last(&p->scopes);
}

static void push_scope(struct parser *p, struct scope *s) {
    vec_push(&p->scopes, s);
}

static struct scope *pop_scope(struct parser *p) {
    return vec_pop(&p->scopes);
}

static struct name *name_new(int kind, struct type *ty, struct pos pos) {
    struct name *n = arena_alloc(sizeof *n);
    n->kind = kind;
    n->ty = ty;
    n->pos = pos;
    return n;
}

static struct name *lookup_name(struct parser *p, const char *name) {
    for (uint i = p->scopes.n; i > 0; i--) {
        struct scope *s = p->scopes.v[i - 1];
        struct name *n = map_get(&s->names, name);
        if (n != NULL) {
            return n;
        }
    }
    return NULL;
}

/* The type a typedef name in scope stands for, or NULL. */
static struct type *typedef_of(struct parser *p, const char *name) {
    struct name *n = lookup_name(p, name);
    return n != NULL && n->kind == NAME_TYPEDEF ? n->ty : NULL;
}

static int is_typedef(struct parser *p, struct token *t) {
    return t->kind == TK_IDENT && typedef_of(p, t->name) != NULL;
}

static struct tag *lookup_tag(struct parser *p, const char *name) {
    for (uint i = p->scopes.n; i > 0; i--) {
        struct scope *s = p->scopes.v[i - 1];
        struct tag *t = map_get(&s->tags, name);
        if (t != NULL) {
            return t;
        }
    }
    return NULL;
}

static void bind_tag(struct parser *p, const char *name, struct tag *t) {
    map_put(&innermost(p)->tags, name, t);
}

/* Every name in scope, innermost first, each once (an inner one hides the
 * outer ones), for typo correction: the names in *n of them, and what each
 * means. */
static const char **names_in_scope(struct parser *p, struct name ***what, uint *n) {
    struct map seen;
    memset(&seen, 0, sizeof seen);
    const char **names = NULL;
    struct name **whats = NULL;
    uint count = 0;
    uint cap = 0;
    uint capw = 0;
    for (uint i = p->scopes.n; i > 0; i--) {
        struct scope *s = p->scopes.v[i - 1];
        for (uint k = 0; k < s->names.cap; k++) {
            const char *key = s->names.keys[k];
            if (key == NULL || map_get(&seen, key) != NULL) {
                continue;
            }
            map_put(&seen, key, (void *)key);
            names = grow(names, &cap, count + 1, sizeof(char *));
            whats = grow(whats, &capw, count + 1, sizeof(struct name *));
            names[count] = key;
            whats[count] = s->names.values[k];
            count++;
        }
    }
    heap_free(seen.keys);
    heap_free(seen.values);
    *what = whats;
    *n = count;
    return names;
}

/* What clang's typo correction takes the undeclared typo, read as an
 * expression, for: a variable, function or enumeration constant in scope;
 * NULL for none. */
static struct correction *correct_expr(struct parser *p, const char *typo) {
    struct name **what;
    uint n;
    const char **names = names_in_scope(p, &what, &n);
    const char **cand = heap_alloc((n + 1) * sizeof(char *));
    struct name **cwhat = heap_alloc((n + 1) * sizeof(struct name *));
    uint k = 0;
    for (uint i = 0; i < n; i++) {
        if (what[i]->kind != NAME_TYPEDEF) {
            cand[k] = names[i];
            cwhat[k] = what[i];
            k++;
        }
    }
    int best = correction(typo, cand, k);
    struct correction *c = NULL;
    if (best >= 0) {
        c = arena_alloc(sizeof *c);
        c->name = cand[best];
        c->pos = cwhat[best]->pos;
        if (cwhat[best]->kind == NAME_ENUM_CONST) {
            c->has_enumerator = 1;
            c->value = cwhat[best]->value;
            c->of = cwhat[best]->spelled;
        }
    }
    heap_free(names);
    heap_free(what);
    heap_free(cand);
    heap_free(cwhat);
    return c;
}

/* What clang's typo correction takes the unknown type name typo for: a
 * typedef name in scope (*decl where it is declared, has_decl 1), or a
 * keyword of a type specifier or qualifier; NULL for none. */
static const char *correct_type(struct parser *p, const char *typo, int *has_decl, struct pos *decl) {
    struct name **what;
    uint n;
    const char **names = names_in_scope(p, &what, &n);
    const char **cand = heap_alloc((n + NTYPE_KEYWORDS + 1) * sizeof(char *));
    struct name **cwhat = heap_alloc((n + NTYPE_KEYWORDS + 1) * sizeof(struct name *));
    uint k = 0;
    for (uint i = 0; i < n; i++) {
        if (what[i]->kind == NAME_TYPEDEF) {
            cand[k] = names[i];
            cwhat[k] = what[i];
            k++;
        }
    }
    for (uint i = 0; i < NTYPE_KEYWORDS; i++) {
        cand[k] = TYPE_KEYWORDS[i];
        cwhat[k] = NULL;
        k++;
    }
    int best = correction(typo, cand, k);
    const char *r = NULL;
    *has_decl = 0;
    if (best >= 0) {
        r = cand[best];
        if (cwhat[best] != NULL) {
            *has_decl = 1;
            *decl = cwhat[best]->pos;
        }
    }
    heap_free(names);
    heap_free(what);
    heap_free(cand);
    heap_free(cwhat);
    return r;
}

/* clang's `unknown type name` of the name here, with what its typo
 * correction takes it for. */
static struct diag *unknown_type_name(struct parser *p, const char *name) {
    struct pos at = here(p);
    int has_decl;
    struct pos decl;
    const char *to = correct_type(p, name, &has_decl, &decl);
    if (to == NULL) {
        return diag_error(at, fmt("unknown type name '%s'", name));
    }
    struct diag *e = diag_error(at, fmt("unknown type name '%s'; did you mean '%s'?", name, to));
    if (has_decl) {
        diag_note(e, decl, fmt("'%s' declared here", to));
    }
    return e;
}

/* A block's statement that starts with two names, the first declared
 * nowhere (`myit x = 1;`): clang's typo correction of it among the names
 * in scope, typedefs and the keywords a statement can start with. */
static struct diag *unknown_statement_name(struct parser *p, const char *name) {
    struct pos at = here(p);
    struct name **what;
    uint n;
    const char **names = names_in_scope(p, &what, &n);
    const char **cand = heap_alloc((n + NSTATEMENT_KEYWORDS + 1) * sizeof(char *));
    for (uint i = 0; i < n; i++) {
        cand[i] = names[i];
    }
    for (uint i = 0; i < NSTATEMENT_KEYWORDS; i++) {
        cand[n + i] = STATEMENT_KEYWORDS[i];
    }
    int best = correction(name, cand, n + NSTATEMENT_KEYWORDS);
    struct diag *e;
    if (best >= 0 && (uint)best < n && what[best]->kind == NAME_TYPEDEF) {
        e = diag_error(at, fmt("unknown type name '%s'; did you mean '%s'?", name, cand[best]));
        diag_note(e, what[best]->pos, fmt("'%s' declared here", cand[best]));
    } else if (best >= 0 && (uint)best >= n) {
        e = diag_error(at, fmt("use of undeclared identifier '%s'; did you mean '%s'?", name, cand[best]));
    } else {
        e = diag_error(at, fmt("unknown type name '%s'", name));
    }
    heap_free(names);
    heap_free(what);
    heap_free(cand);
    return e;
}

/* The composite of ty and the type of the earlier declaration of name with
 * linkage this block can see: the innermost around it, unless it has no
 * linkage; ty if there is none, or it is not compatible (sema's error). */
static struct type *visible_composite(struct parser *p, const char *name, struct type *ty) {
    for (uint i = p->scopes.n; i > 0; i--) {
        struct scope *s = p->scopes.v[i - 1];
        struct name *n = map_get(&s->names, name);
        if (n == NULL) {
            continue;
        }
        if (n->kind == NAME_OBJECT && (i - 1 == 0 || map_get(&s->linked, name) != NULL) &&
            ty_compatible(n->ty, ty)) {
            return ty_composite(n->ty, ty);
        }
        return ty;
    }
    return ty;
}

/* Rust's derived == of two Names: the same kind, of the same type or value
 * (spellings never count). */
static int name_eq(struct name *a, struct name *b) {
    if (a->kind != b->kind) {
        return 0;
    }
    if (a->kind == NAME_ENUM_CONST) {
        return i128_eq(a->value, b->value);
    }
    return ty_eq(a->ty, b->ty);
}

/* How a typedef's own declaration spelled its type. */
static struct spelling *typedef_base(struct spelling *name) {
    return name != NULL && name->kind == SP_TYPEDEF ? name->td->spelled : NULL;
}

static struct diag *redefinition(struct pos pos, const char *how, struct pos prev) {
    return diag_note(diag_error(pos, fmt("redefinition of %s", how)), prev, "previous definition is here");
}

/* Declares an ordinary identifier in the innermost scope (merlin's
 * bind_name). Declaring a variable or function again is sema's to judge;
 * any other clash is an error here, with clang's note where the name was
 * declared before. */
static struct diag *bind_name(struct parser *p, const char *name, struct name *what) {
    struct scope *s = innermost(p);
    struct name *prev = map_get(&s->names, name);
    struct pos pos = what->pos;
    if (prev == NULL) {
        map_put(&s->names, name, what);
        return NULL;
    }
    /* A variable declared again: keep the type that says the most. */
    if (prev->kind == NAME_OBJECT) {
        if (what->kind == NAME_OBJECT) {
            if (ty_size(what->ty) >= 0 || ty_size(prev->ty) < 0) {
                prev->ty = what->ty;
            }
            return NULL;
        }
        if (what->kind == NAME_ENUM_CONST) {
            return redefinition(pos, fmt("'%s'", name), prev->pos);
        }
        return redefinition(pos, fmt("'%s' as different kind of symbol", name), prev->pos);
    }
    /* A typedef name defined again: of the same type, what C11 allows and
     * clang warns of under C99 (the name is then the later's); of another,
     * an error. Two enums are two types, though merlin's are their integer
     * type. */
    if (prev->kind == NAME_TYPEDEF && what->kind == NAME_TYPEDEF) {
        struct spelling *prev_base = typedef_base(prev->spelled);
        struct spelling *new_base = typedef_base(what->spelled);
        if (ty_eq(prev->ty, what->ty) && !distinct_enums(prev->ty, prev_base, what->ty, new_base)) {
            struct diag *w = diag_warning(pos, fmt("redefinition of typedef '%s' is a C11 feature", name),
                                          "typedef-redefinition");
            warn(p, diag_note(w, prev->pos, "previous definition is here"));
            map_put(&s->names, name, what);
            return NULL;
        }
        struct diag *e = diag_error(pos, fmt("typedef redefinition with different types (%s vs %s)",
                                             ty_named(what->ty, new_base), ty_named(prev->ty, prev_base)));
        return diag_note(e, prev->pos, "previous definition is here");
    }
    if (name_eq(prev, what) && what->kind != NAME_ENUM_CONST) {
        return NULL;
    }
    if (prev->kind == NAME_ENUM_CONST && what->kind == NAME_ENUM_CONST) {
        return redefinition(pos, fmt("enumerator '%s'", name), prev->pos);
    }
    /* An enumerator after anything else is plainly a redefinition. */
    if (what->kind == NAME_ENUM_CONST) {
        return redefinition(pos, fmt("'%s'", name), prev->pos);
    }
    return redefinition(pos, fmt("'%s' as different kind of symbol", name), prev->pos);
}

/* ---- Unevaluated expressions ---- */

/* An unevaluated expression, with the parameters in scope where it is. */
static struct unevaluated *unevaluated_here(struct parser *p, struct expr *e) {
    struct unevaluated *u = unevaluated_new(e);
    u->nparams = p->prototype.n;
    u->params = arena_alloc(p->prototype.n * sizeof(struct proto_param) + 1);
    for (uint i = 0; i < p->prototype.n; i++) {
        u->params[i] = *(struct proto_param *)p->prototype.v[i];
    }
    return u;
}

static struct unevaluated *unevaluated_at(struct parser *p, uint i) {
    return p->unevaluated.v[i];
}

static struct type *operand_type(struct parser *p, struct expr *e);

/* The type of e's value: operand_type after decay, or NULL. */
static struct type *value_type(struct parser *p, struct expr *e) {
    struct type *t = operand_type(p, e);
    return t != NULL ? ty_decay(t) : NULL;
}

static int init_len(struct type *elem, struct init *init, uint *out);

/* The type of sizeof's operand (or of an operand ?: does not choose), as
 * far as the parser can tell before sema, as sema types it (merlin's
 * operand_type); NULL for what it cannot tell. */
static struct type *operand_type(struct parser *p, struct expr *e) {
    struct type *lt;
    struct type *rt;
    switch (e->kind) {
    case E_NUM:
    case E_ENUMERATOR:
    case E_STR:
    case E_CAST:
        return e->ty;
    case E_COMPOUND:
        /* (int[]){1, 2, 3}: the list completes the array's size. */
        if (e->ty->kind == TY_ARRAY && !e->ty->has_len) {
            uint n;
            if (!init_len(e->ty->to, e->init, &n)) {
                return NULL;
            }
            return ty_array(e->ty->to, 1, n);
        }
        return e->ty;
    case E_SIZEOF_TYPE:
    case E_SIZEOF_EXPR:
        return ty_int(ik_size_t());
    case E_VAR: {
        struct name *n = lookup_name(p, e->name);
        return n != NULL && n->kind == NAME_OBJECT ? n->ty : NULL;
    }
    case E_DEREF:
        /* a[i] is *(a + i). */
        lt = value_type(p, e->x);
        return lt != NULL ? ty_pointee(lt) : NULL;
    case E_ADDR:
        lt = operand_type(p, e->x);
        return lt != NULL ? ty_ptr(lt) : NULL;
    case E_MEMBER: {
        struct type *rec;
        if (e->flag) {
            lt = value_type(p, e->x);
            rec = lt != NULL ? ty_pointee(lt) : NULL;
        } else {
            rec = operand_type(p, e->x);
        }
        struct record *r = rec != NULL ? ty_record(rec) : NULL;
        if (r == NULL || r->body == NULL) {
            return NULL;
        }
        struct member *m = record_member(r->body, e->name);
        return m != NULL ? m->ty : NULL;
    }
    case E_BINARY:
        switch (e->op) {
        case OP_ADD:
        case OP_SUB:
            lt = value_type(p, e->x);
            rt = lt != NULL ? value_type(p, e->y) : NULL;
            if (rt == NULL) {
                return NULL;
            }
            /* The difference of two pointers is a ptrdiff_t. */
            if (ty_is_ptr(lt) && ty_is_ptr(rt)) {
                return e->op == OP_SUB ? ty_int(ik_ptrdiff_t()) : NULL;
            }
            if (ty_is_ptr(lt)) {
                return lt;
            }
            if (ty_is_ptr(rt)) {
                return e->op == OP_ADD ? rt : NULL;
            }
            if (ty_int_kind(lt) < 0 || ty_int_kind(rt) < 0) {
                return NULL;
            }
            return ty_int(ik_common(ty_int_kind(lt), ty_int_kind(rt)));
        case OP_MUL:
        case OP_DIV:
        case OP_REM:
        case OP_BITAND:
        case OP_BITOR:
        case OP_BITXOR:
            lt = value_type(p, e->x);
            if (lt == NULL || ty_int_kind(lt) < 0) {
                return NULL;
            }
            rt = value_type(p, e->y);
            if (rt == NULL || ty_int_kind(rt) < 0) {
                return NULL;
            }
            return ty_int(ik_common(ty_int_kind(lt), ty_int_kind(rt)));
        case OP_SHL:
        case OP_SHR:
            /* A shift is of its promoted left operand's type. */
            lt = value_type(p, e->x);
            if (lt == NULL || ty_int_kind(lt) < 0) {
                return NULL;
            }
            return ty_int(ik_promoted(ty_int_kind(lt)));
        default:
            return ty_int(IK_INT);
        }
    case E_UNARY:
        if (e->op == UOP_NOT) {
            return ty_int(IK_INT);
        }
        lt = value_type(p, e->x);
        if (lt == NULL || ty_int_kind(lt) < 0) {
            return NULL;
        }
        return ty_int(ik_promoted(ty_int_kind(lt)));
    case E_COND:
        lt = value_type(p, e->y);
        rt = lt != NULL ? value_type(p, e->z) : NULL;
        if (rt == NULL) {
            return NULL;
        }
        if (ty_int_kind(lt) >= 0 && ty_int_kind(rt) >= 0) {
            return ty_int(ik_common(ty_int_kind(lt), ty_int_kind(rt)));
        }
        return ty_eq(lt, rt) ? lt : NULL;
    case E_COMMA:
        return value_type(p, e->y);
    case E_ASSIGN:
    case E_INCDEC:
        lt = value_type(p, e->x);
        return lt != NULL ? ty_unqual(lt) : NULL;
    case E_CALL: {
        lt = value_type(p, e->x);
        struct type *f = lt != NULL ? ty_pointee(lt) : NULL;
        if (f == NULL || f->kind != TY_FUNC) {
            return NULL;
        }
        return ty_unqual(f->fn->ret);
    }
    default:
        return NULL;
    }
}

static void resolve_sizeof(struct parser *p, struct expr *e);

/* The operand a constant ?: does not choose (merlin's not_chosen). It is
 * not evaluated (C99 6.6p3): only its type counts. One that is no integer
 * constant expression becomes a 0 of its type, as sema types it, and sema
 * checks what it was; clang takes it, folding it. If the parser cannot work
 * out its type, or it is no integer type, the 0 is an int and sema reports
 * the error. The expression is replaced in place. */
static void not_chosen(struct parser *p, struct expr *e) {
    uint mark = p->unevaluated.n;
    struct expr *resolved = expr_clone(e);
    resolve_sizeof(p, resolved);
    /* A constant, as it is not evaluated: `1 / 0` is one there. */
    i128 v;
    if (consteval_parsed_unevaluated_int(resolved, &v)) {
        *e = *resolved;
        return;
    }
    /* The whole operand is checked, so not what is in it again. */
    vec_truncate(&p->unevaluated, mark);
    struct type *vt = value_type(p, e);
    struct type *ty;
    int untyped;
    if (vt != NULL && ty_is_integer(vt)) {
        ty = ty_unqual(vt);
        untyped = 0;
    } else {
        ty = ty_int(IK_INT);
        untyped = 1;
    }
    struct pos pos = e->pos;
    struct expr *original = arena_alloc(sizeof *original);
    *original = *e;
    struct expr *zero = expr_num(i128_of(0), ty, pos);
    *e = *zero;
    struct unevaluated *u = unevaluated_here(p, original);
    if (untyped) {
        u->has_untyped = 1;
        u->untyped_pos = original->pos;
        u->untyped_msg = "";
    }
    u->folded = 1;
    vec_push(&p->unevaluated, u);
    p->folding = 1;
}

/* Rewrites sizeof expr into sizeof (type) throughout the operators
 * consteval evaluates, where operand_type knows the type, and the operand a
 * constant ?: does not choose as not_chosen says. */
static void resolve_sizeof(struct parser *p, struct expr *e) {
    switch (e->kind) {
    case E_SIZEOF_EXPR: {
        struct type *t = operand_type(p, e->x);
        if (t != NULL) {
            vec_push(&p->unevaluated, unevaluated_here(p, expr_clone(e->x)));
            e->kind = E_SIZEOF_TYPE;
            e->of = t;
            e->x = NULL;
        }
        break;
    }
    case E_UNARY:
    case E_CAST:
    case E_ADDR:
    case E_DEREF:
        resolve_sizeof(p, e->x);
        break;
    case E_MEMBER: {
        /* A member's offset is sema's to fill in, which comes too late for a
         * constant the parser evaluates: offsetof's
         * (unsigned long)&((struct s *)0)->m in a _Static_assert. */
        resolve_sizeof(p, e->x);
        struct type *rec;
        if (e->flag) {
            struct type *vt = value_type(p, e->x);
            rec = vt != NULL ? ty_pointee(vt) : NULL;
        } else {
            rec = operand_type(p, e->x);
        }
        struct record *r = rec != NULL ? ty_record(rec) : NULL;
        if (r != NULL && r->body != NULL) {
            struct member *m = record_member(r->body, e->name);
            if (m != NULL) {
                e->offset = m->offset;
            }
        }
        break;
    }
    case E_BINARY:
        resolve_sizeof(p, e->x);
        resolve_sizeof(p, e->y);
        break;
    case E_COND: {
        resolve_sizeof(p, e->x);
        i128 v;
        if (consteval_parsed_int(e->x, &v)) {
            struct expr *chosen = !i128_is_zero(v) ? e->y : e->z;
            struct expr *other = !i128_is_zero(v) ? e->z : e->y;
            resolve_sizeof(p, chosen);
            not_chosen(p, other);
        } else {
            resolve_sizeof(p, e->y);
            resolve_sizeof(p, e->z);
        }
        break;
    }
    default:
        break;
    }
}

/* The value of an integer constant expression the parser needs now (an
 * array size, an enumerator's value or a _Static_assert), or the error msg
 * at `at` if it is none (merlin's const_int, whose comment explains it).
 * *sure says whether the value is: 0 if the parser could not type an
 * operand ?: does not choose, or found no constant, whose error sema will
 * report; the value is then 1. non_integer is clang's error for a constant
 * of a type that is no integer, if it has one there (or NULL). e may be
 * rewritten (its sizeofs resolved). */
static void const_int(struct parser *p, struct expr *e, struct pos at, const char *msg, const char *non_integer,
                      i128 *value, int *sure) {
    uint mark = p->unevaluated.n;
    struct expr *original = expr_clone(e);
    resolve_sizeof(p, e);
    i128 v;
    if (!consteval_parsed_int(e, &v)) {
        /* No constant the parser can work out: sema checks the whole
         * expression, and reports its error if it has one, else msg. */
        vec_truncate(&p->unevaluated, mark);
        p->folding = 0;
        struct unevaluated *u = unevaluated_here(p, original);
        u->has_untyped = 1;
        u->untyped_pos = at;
        u->untyped_msg = msg;
        if (non_integer != NULL) {
            u->has_non_integer = 1;
            u->non_integer_pos = at;
            u->non_integer_msg = non_integer;
        }
        vec_push(&p->unevaluated, u);
        *value = i128_of(1);
        *sure = 0;
        return;
    }
    int ok = 1;
    for (uint i = mark; i < p->unevaluated.n; i++) {
        struct unevaluated *u = unevaluated_at(p, i);
        if (u->has_untyped) {
            u->untyped_pos = at;
            u->untyped_msg = msg;
            ok = 0;
        }
    }
    if (!ok && non_integer != NULL) {
        vec_truncate(&p->unevaluated, mark);
        p->folding = 0;
        struct unevaluated *u = unevaluated_here(p, original);
        u->has_untyped = 1;
        u->untyped_pos = at;
        u->untyped_msg = msg;
        u->has_non_integer = 1;
        u->non_integer_pos = at;
        u->non_integer_msg = non_integer;
        vec_push(&p->unevaluated, u);
        *value = i128_of(1);
        *sure = 0;
        return;
    }
    /* The constant itself, for sema to check as clang does: not a lone
     * literal, constant or sizeof, of which there is nothing to say. */
    int leaf = e->kind == E_NUM || e->kind == E_ENUMERATOR || e->kind == E_SIZEOF_TYPE;
    if (ok && !leaf) {
        struct unevaluated *u = unevaluated_here(p, expr_clone(e));
        u->constant = 1;
        vec_push(&p->unevaluated, u);
    }
    *value = v;
    *sure = ok;
}

/* What is said of a declarator's folded size: kept with the last of its own
 * operands, for sema to give if none of them has an error. */
static void fold_diagnostic(struct parser *p, struct decl *d, struct diag *said) {
    uint n = d->unevaluated.end - d->unevaluated.start;
    struct unevaluated *u = unevaluated_at(p, d->unevaluated.end - 1);
    u->diagnostic = said;
    u->with = n - 1;
}

/* An array size folded (`int a[1 ? 2 : f()]`, whose f() is no constant):
 * clang takes the array as a constant one where an array cannot be
 * variable-length, warning that it folds it, where it can (merlin's folded,
 * whose comment lists the places); elsewhere it is a variable-length
 * array, or a type made of one, which merlin does not have: the size is an
 * error. d is declared in ctx with storage class `class`, and an
 * initializer if init. */
static void folded(struct parser *p, struct decl *d, int ctx, int class, int init) {
    if (!d->has_folded) {
        return;
    }
    struct pos at = d->folded;
    if (ty_is_func(d->ty) && class != CLASS_TYPEDEF && d->reach == REACH_FUNCTION) {
        fold_diagnostic(p, d, diag_error(d->pos, "function declaration cannot have variably modified type"));
        return;
    }
    /* What clang can make a constant array of. */
    int fixable = d->reach <= REACH_POINTERS;
    int folds;
    switch (ctx) {
    case CTX_FILE:
        if (!fixable) {
            fold_diagnostic(p, d, diag_error(d->pos, "variably modified type declaration not allowed at file scope"));
            return;
        }
        folds = 1;
        break;
    case CTX_MEMBER:
        if (!fixable) {
            fold_diagnostic(p, d,
                            diag_error(d->pos, "fields must have a constant size: 'variable length array in "
                                               "structure' extension will never be supported"));
            return;
        }
        folds = 1;
        break;
    case CTX_BLOCK:
    case CTX_FOR_INIT:
        if (class == CLASS_STATIC) {
            folds = d->reach == REACH_ARRAYS;
        } else if (class == CLASS_EXTERN) {
            folds = fixable;
        } else if (class == CLASS_TYPEDEF) {
            folds = 0;
        } else {
            folds = init && d->reach == REACH_ARRAYS;
        }
        break;
    default:
        folds = 0;
        break;
    }
    struct diag *said = folds ? diag_warning(d->pos, FOLDED_MSG, FOLDED_FLAG) : diag_error(at, NOT_CONSTANT_SIZE);
    fold_diagnostic(p, d, said);
    if (folds && ctx == CTX_FILE) {
        struct unevaluated *u = unevaluated_at(p, d->unevaluated.end - 1);
        struct file_vla *vla = arena_alloc(sizeof *vla);
        vla->at = d->pos;
        vla->has_top = d->has_top;
        vla->top = d->top;
        vla->inner = d->inner;
        vla->ninner = d->ninner;
        u->vla = vla;
    }
}

/* A type name's folded size (merlin's type_name_folded): a compound
 * literal's array (its `(` at literal) is folded as an initialized array
 * is, with clang's warning at the `(`; one clang does not fold is its
 * error. At file scope, a type that only points to such an array is taken
 * without a word. Anything else is a variable-length array, an error. */
static void type_name_folded(struct parser *p, struct decl *d, int has_literal, struct pos literal) {
    if (!d->has_folded) {
        return;
    }
    struct pos at = d->folded;
    if (has_literal && d->reach == REACH_ARRAYS) {
        fold_diagnostic(p, d, diag_warning(literal, FOLDED_MSG, FOLDED_FLAG));
    } else if (has_literal && ty_is_array(d->ty)) {
        /* A literal of an array clang does not fold is one of variable
         * length, which nothing initializes. */
        fold_diagnostic(p, d, diag_error(at, "variable-sized object may not be initialized"));
    } else if (p->scopes.n == 1 && d->reach > REACH_ARRAYS) {
        /* Taken as the array of the folded size. */
    } else {
        fold_diagnostic(p, d, diag_error(at, NOT_CONSTANT_SIZE));
    }
}

/* The undeclared names read since mark that are in the item a syntax error
 * cut short (read after `item`, its expr_next_id), as a statement for sema
 * to check, unevaluated; NULL for none. The names since mark are taken. */
static struct stmt *cut_names(struct parser *p, uint mark, uint64_t item) {
    struct vec names = vec_split_off(&p->undeclared, mark);
    struct stmt *s = NULL;
    for (uint i = 0; i < names.n; i++) {
        struct expr *e = names.v[i];
        if (e->id < item) {
            continue;
        }
        if (s == NULL) {
            s = stmt_new(S_UNEVALUATED, e->pos);
        }
        vec_push(&s->list, unevaluated_new(e));
    }
    heap_free(names.v);
    return s;
}

/* What the declaration or statement just parsed left for sema to check, as
 * a statement after it, at pos; NULL for nothing. */
static struct stmt *unevaluated_stmt(struct parser *p, struct pos pos) {
    if (p->unevaluated.n == 0) {
        return NULL;
    }
    struct stmt *s = stmt_new(S_UNEVALUATED, pos);
    s->list = vec_take(&p->unevaluated);
    return s;
}

/* ---- Helpers of declarations ---- */

/* The length an initializer gives an array of unknown size with elements
 * of type elem, when it is a plain one: a string literal (or one in
 * braces) for a character array, or a list of scalars, each designated by
 * a constant index or none ({[3] = 1, 2} has 5). 0 for the rest, which
 * sema works out. */
static int init_string_len(struct type *elem, struct init *i, uint *out) {
    int k = ty_int_kind(elem);
    int is_char = k >= 0 && ik_size(k) == 1;
    if (i->kind == I_EXPR && i->e->kind == E_STR && is_char) {
        *out = i->e->len + 1;
        return 1;
    }
    return 0;
}

static int init_len(struct type *elem, struct init *init, uint *out) {
    if (init->kind == I_LIST && init->items.n == 1 && init_string_len(elem, init->items.v[0], out)) {
        return 1;
    }
    if (init->kind == I_LIST && ty_is_scalar(elem)) {
        /* The index of the next element, and the length so far. */
        i128 next = i128_of(0);
        i128 len = i128_of(0);
        for (uint i = 0; i < init->items.n; i++) {
            struct init *item = init->items.v[i];
            if (item->kind == I_DESIGNATED && item->inner->kind == I_EXPR) {
                if (item->nds != 1 || item->ds[0]->member) {
                    return 0;
                }
                i128 v;
                if (!consteval_parsed_int(item->ds[0]->index, &v) || i128_neg_p(v)) {
                    return 0;
                }
                next = v;
            } else if (item->kind != I_EXPR) {
                return 0;
            }
            next = i128_add(next, i128_of(1));
            len = i128_max(len, next);
        }
        if (len.hi != 0 || len.lo > 0xffffffffu) {
            return 0;
        }
        *out = (uint)len.lo;
        return 1;
    }
    return init_string_len(elem, init, out);
}

/* How a parameter declared with type `declared` and spelling `spelled`
 * spells its adjusted type (merlin's decayed_spelling): declared as an
 * array through an array typedef, its own, it is a pointer to the
 * elements, and the typedef is gone with the array it named. */
static struct spelling *decayed_spelling(struct type *declared, struct spelling *spelled) {
    if (!ty_is_array(declared)) {
        return spelled;
    }
    while (spelled != NULL && spelled->kind == SP_TYPEDEF) {
        struct typedef_name *t = spelled->td;
        if (!ty_is_array(t->ty) || ty_quals_added_to(declared, t->ty) < 0) {
            break;
        }
        spelled = t->spelled;
    }
    return spelled;
}

/* `n` as clang writes a count, its digits grouped by threes with `'`. */
static char *grouped(i128 n) {
    char *digits = i128_str(n);
    uint len = (uint)strlen(digits);
    struct buf b;
    memset(&b, 0, sizeof b);
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

/* The Reach of the arrays `pick` chooses among a declarator's steps (the
 * base's first): those whose size is folded (pick 0), or no constant (pick
 * 1). */
static int reach(struct derive *ops, uint n, int pick) {
    int r = REACH_ARRAYS;
    for (uint k = 0; k < n; k++) {
        struct derive *op = &ops[k];
        if (op->kind != D_ARRAY || !(pick == 0 ? op->has_folded : !op->constant)) {
            continue;
        }
        /* From what is declared in to this array. */
        int arrays = 0;
        for (uint j = n; j > k + 1; j--) {
            struct derive *o = &ops[j - 1];
            if (o->kind == D_FUNC) {
                r = REACH_FUNCTION;
                break;
            }
            if (o->kind == D_PTR) {
                int m = arrays ? REACH_OTHER : REACH_POINTERS;
                if (m > r) {
                    r = m;
                }
            } else if (!o->has_len) {
                if (REACH_OTHER > r) {
                    r = REACH_OTHER;
                }
            } else {
                arrays = 1;
            }
        }
    }
    return r;
}

/* ---- Declaration specifiers ---- */

static struct diag *record_spec(struct parser *p, int is_union, struct pos kw_pos, struct type **ty, int *declares,
                                int *anonymous);
static struct diag *enum_spec(struct parser *p, struct pos kw_pos, struct type **ty, int *declares,
                              struct spelling **spelled);

/* The one type specifier besides the width and the sign (merlin's Base). */
enum { BASE_NONE_, BASE_VOID, BASE_CHAR, BASE_INT, BASE_BOOL, BASE_NAMED };
enum { WIDTH_NONE, WIDTH_SHORT, WIDTH_LONG, WIDTH_LONG_LONG };

static const char *width_name(int w) {
    return w == WIDTH_SHORT ? "short" : w == WIDTH_LONG ? "long" : "long long";
}

static const char *base_name(int base, const char *named) {
    switch (base) {
    case BASE_VOID:
        return "void";
    case BASE_CHAR:
        return "char";
    case BASE_INT:
        return "int";
    case BASE_BOOL:
        return "_Bool";
    default:
        return named;
    }
}

static struct diag *combine(struct pos pos, const char *prev) {
    return diag_error(pos, fmt("cannot combine with previous '%s' declaration specifier", prev));
}

/* clang's warning of what is there twice. */
static void duplicate(struct parser *p, struct pos pos, int k) {
    warn(p, diag_warning(pos, fmt("duplicate '%s' declaration specifier", KEYWORDS[k]), "duplicate-decl-specifier"));
}

/* Type specifiers, qualifiers, storage class and inline, in any order, read
 * as clang's DeclSpec reads them (merlin's specifiers): a width, a sign,
 * and one other type specifier, each checked as it comes against what came
 * before, then the three against each other. With none of them, the type
 * is C89's implicit int, an error in C99. */
static struct diag *specifiers(struct parser *p, int ctx, struct specs *out) {
    struct pos start = here(p);
    int has_class = 0;
    int class = CLASS_NONE;
    int class_kw = 0;
    struct pos class_pos = start;
    int has_inline = 0;
    struct pos inline_pos = start;
    uint quals = 0;
    int restrict_ = 0;
    int width = WIDTH_NONE;
    struct pos width_pos = start;
    /* 1 for unsigned, 0 for signed, -1 for none. */
    int sign = -1;
    struct pos sign_pos = start;
    int base = BASE_NONE_;
    struct type *named = NULL;
    const char *named_name = NULL;
    struct spelling *spelled = NULL;
    int declares = 0;
    int anonymous = 0;
    for (;;) {
        struct pos pos = here(p);
        struct token *t = tok(p);
        int k;
        if (t->kind == TK_KW && kw_starts_decl(t->kw)) {
            k = t->kw;
        } else if (t->kind == TK_IDENT && base == BASE_NONE_ && width == WIDTH_NONE && sign < 0 &&
                   typedef_of(p, t->name) != NULL) {
            /* A typedef name is a type specifier only where no other is
             * yet: in `int T;` the T is the declarator. */
            struct name *n = lookup_name(p, t->name);
            named = n->ty;
            named_name = "type-name";
            base = BASE_NAMED;
            spelled = n->spelled;
            advance(p);
            continue;
        } else {
            break;
        }
        advance(p);
        switch (k) {
        case KW_STATIC:
        case KW_EXTERN:
        case KW_TYPEDEF:
        case KW_REGISTER:
        case KW_AUTO: {
            if (ctx == CTX_MEMBER || ctx == CTX_TYPE_NAME) {
                return diag_error(pos, "type name does not allow storage class to be specified");
            }
            /* A parameter may be register, and nothing else. */
            if (ctx == CTX_PARAM && k != KW_REGISTER) {
                return diag_error(pos, "invalid storage class specifier in function declarator");
            }
            int c = k == KW_STATIC     ? CLASS_STATIC
                    : k == KW_EXTERN   ? CLASS_EXTERN
                    : k == KW_REGISTER ? CLASS_REGISTER
                    : k == KW_AUTO     ? CLASS_AUTO
                                       : CLASS_TYPEDEF;
            if (!has_class) {
                has_class = 1;
                class = c;
                class_kw = k;
                class_pos = pos;
            } else if (class_kw == k) {
                duplicate(p, pos, k);
            } else {
                return combine(pos, KEYWORDS[class_kw]);
            }
            break;
        }
        case KW_INLINE:
            if (ctx == CTX_MEMBER || ctx == CTX_TYPE_NAME) {
                return diag_error(pos, "type name does not allow function specifier to be specified");
            }
            if (ctx == CTX_PARAM) {
                return diag_error(pos, "'inline' can only appear on functions");
            }
            if (!has_inline) {
                has_inline = 1;
                inline_pos = pos;
            } else {
                duplicate(p, pos, k);
            }
            break;
        case KW_CONST:
        case KW_VOLATILE:
        case KW_RESTRICT: {
            int had;
            if (k == KW_CONST) {
                had = (quals & Q_CONST) != 0;
                quals |= Q_CONST;
            } else if (k == KW_VOLATILE) {
                had = (quals & Q_VOLATILE) != 0;
                quals |= Q_VOLATILE;
            } else {
                /* restrict promises no aliasing, which merlin never relies on. */
                had = restrict_;
                restrict_ = 1;
            }
            if (had) {
                duplicate(p, pos, k);
            }
            break;
        }
        case KW_SHORT:
        case KW_LONG:
            if (width == WIDTH_NONE) {
                width = k == KW_SHORT ? WIDTH_SHORT : WIDTH_LONG;
                width_pos = pos;
            } else if (width == WIDTH_SHORT && k == KW_SHORT) {
                duplicate(p, pos, k);
            } else if (width == WIDTH_LONG && k == KW_LONG) {
                /* long long, which is where the first long is. */
                width = WIDTH_LONG_LONG;
            } else {
                return combine(pos, width_name(width));
            }
            break;
        case KW_SIGNED:
        case KW_UNSIGNED: {
            int u = k == KW_UNSIGNED;
            if (sign < 0) {
                sign = u;
                sign_pos = pos;
            } else if (sign == u) {
                duplicate(p, pos, k);
            } else {
                return combine(pos, sign ? "unsigned" : "signed");
            }
            break;
        }
        default:
            if (base != BASE_NONE_) {
                return combine(pos, base_name(base, named_name));
            }
            if (k == KW_VOID) {
                base = BASE_VOID;
            } else if (k == KW_CHAR) {
                base = BASE_CHAR;
            } else if (k == KW_INT) {
                base = BASE_INT;
            } else if (k == KW_BOOL) {
                base = BASE_BOOL;
            } else {
                struct type *ty;
                int d;
                int anon = 0;
                struct spelling *s = NULL;
                TRY(parse_enter(p));
                struct diag *e;
                if (k == KW_ENUM) {
                    e = enum_spec(p, pos, &ty, &d, &s);
                } else {
                    e = record_spec(p, k == KW_UNION, pos, &ty, &d, &anon);
                }
                p->depth--;
                if (e != NULL) {
                    return e;
                }
                declares = d;
                spelled = s;
                anonymous = anon;
                base = BASE_NAMED;
                named = ty;
                named_name = KEYWORDS[k];
            }
            break;
        }
    }
    /* Specifiers that end the declaration (`static;`) have none to give a
     * type: clang only warns that they declare nothing. */
    if (base == BASE_NONE_ && width == WIDTH_NONE && sign < 0 && !is_punct(p, ";")) {
        struct diag *e = diag_error(here(p), "type specifier missing, defaults to 'int'; ISO C99 and later do not "
                                             "support implicit int");
        e->flag = "implicit-int";
        return e;
    }
    /* Only char and int have a sign, and only int a width. */
    if (sign >= 0 && base != BASE_NONE_ && base != BASE_CHAR && base != BASE_INT) {
        return diag_error(sign_pos, fmt("'%s' cannot be signed or unsigned", base_name(base, named_name)));
    }
    if (width != WIDTH_NONE && base != BASE_NONE_ && base != BASE_INT) {
        return diag_error(width_pos, fmt("'%s %s' is invalid", width_name(width), base_name(base, named_name)));
    }
    int u = sign == 1;
    struct type *ty;
    if (base == BASE_VOID) {
        ty = TY_VOID_T;
    } else if (base == BASE_BOOL) {
        ty = ty_int(IK_BOOL);
    } else if (base == BASE_NAMED) {
        ty = named;
    } else if (base == BASE_CHAR) {
        ty = ty_int(sign == 1 ? IK_UCHAR : sign == 0 ? IK_SCHAR : IK_CHAR);
    } else if (width == WIDTH_SHORT) {
        ty = ty_int(u ? IK_USHORT : IK_SHORT);
    } else if (width == WIDTH_LONG) {
        ty = ty_int(u ? IK_ULONG : IK_LONG);
    } else if (width == WIDTH_LONG_LONG) {
        ty = ty_int(u ? IK_ULLONG : IK_LLONG);
    } else {
        ty = ty_int(u ? IK_UINT : IK_INT);
    }
    memset(out, 0, sizeof *out);
    out->ty = ty_qualified(ty, quals);
    out->class = has_class ? class : CLASS_NONE;
    out->class_pos = has_class ? class_pos : start;
    out->has_inline = has_inline;
    out->inline_pos = inline_pos;
    out->declares = declares;
    out->anonymous = anonymous;
    out->spelled = spelled;
    out->pos = start;
    return NULL;
}

/* A tag of the other kind, first used at prev: clang's error is at the
 * keyword. */
static struct diag *tag_mismatch(struct pos kw_pos, const char *name, struct pos prev) {
    struct diag *e =
        diag_error(kw_pos, fmt("use of '%s' with tag type that does not match previous declaration", name));
    return diag_note(e, prev, "previous use is here");
}

static struct pos tag_pos(struct tag *t) {
    return t->kind == TAG_RECORD ? t->rec->pos : t->pos;
}

static struct tag *record_tag(struct record *r) {
    struct tag *t = arena_alloc(sizeof *t);
    t->kind = TAG_RECORD;
    t->rec = r;
    return t;
}

static struct diag *record_body(struct parser *p, struct record *record, struct pos at);
static struct diag *after_definition(struct parser *p, const char *kw);

/* After struct or union: a definition (the tag optional) or a reference to a
 * tag (merlin's record_spec). The type, whether the specifier declares a
 * tag by itself, and whether it defines a struct or union without one. */
static struct diag *record_spec(struct parser *p, int is_union, struct pos kw_pos, struct type **ty, int *declares,
                                int *anonymous) {
    const char *kw = is_union ? "union" : "struct";
    const char *name = NULL;
    struct pos pos = kw_pos;
    if (tok(p)->kind == TK_IDENT) {
        struct token *t = advance(p);
        name = t->name;
        pos = t->pos;
    }
    struct type *rt = arena_alloc(sizeof *rt);
    rt->kind = TY_RECORD;
    if (is_punct(p, "{")) {
        struct record *record;
        if (name != NULL) {
            struct tag *t = map_get(&innermost(p)->tags, name);
            if (t != NULL && t->kind == TAG_RECORD && t->rec->is_union == is_union && t->rec->body == NULL) {
                /* `struct s;` earlier in this scope: this completes it. */
                record = t->rec;
            } else if (t != NULL && t->kind == TAG_RECORD && t->rec->is_union == is_union) {
                struct pos prev = t->rec->body != NULL ? t->rec->body->pos : t->rec->pos;
                return diag_note(diag_error(pos, fmt("redefinition of '%s'", name)), prev, "previous definition is here");
            } else if (t != NULL) {
                return tag_mismatch(kw_pos, name, tag_pos(t));
            } else {
                /* Declared before the body, so members can point to it. */
                record = record_new(is_union, name, pos);
                bind_tag(p, name, record_tag(record));
            }
        } else {
            record = record_new(is_union, NULL, kw_pos);
        }
        TRY(record_body(p, record, name != NULL ? pos : kw_pos));
        TRY(after_definition(p, kw));
        rt->rec = record;
        *ty = rt;
        *declares = name != NULL;
        *anonymous = name == NULL;
        return NULL;
    }
    if (name == NULL) {
        return diag_error(kw_pos, fmt("declaration of anonymous %s must be a definition", kw));
    }
    /* `struct s;` on its own declares a new type in this scope, even when an
     * outer scope has a struct s. */
    int alone = is_punct(p, ";");
    struct tag *found = alone ? map_get(&innermost(p)->tags, name) : lookup_tag(p, name);
    *anonymous = 0;
    if (found != NULL && found->kind == TAG_RECORD && found->rec->is_union == is_union) {
        rt->rec = found->rec;
        *ty = rt;
        *declares = alone;
        return NULL;
    }
    if (found != NULL) {
        return tag_mismatch(kw_pos, name, tag_pos(found));
    }
    /* A tag not seen before: an incomplete type, completed by a definition
     * later in the same scope. */
    struct record *r = record_new(is_union, name, pos);
    bind_tag(p, name, record_tag(r));
    rt->rec = r;
    *ty = rt;
    *declares = 1;
    return NULL;
}

static struct diag *declarator(struct parser *p, struct type *base, int ctx, struct decl *out);

/* clang's error where a declaration's specifiers should be and are not. */
static struct diag *expected_specifier(struct parser *p) {
    return diag_error(here(p), "type name requires a specifier or qualifier");
}

/* clang's warning for specifiers that end their declaration and declare
 * nothing (`int;`), at their start. */
static void nothing_declared(struct parser *p, struct specs *specs) {
    warn(p, diag_warning(specs->pos, "declaration does not declare anything", "missing-declarations"));
}

/* `{ member-decl+ }`: the members, laid out into record, whose definition is
 * at `at`. */
static struct diag *record_body(struct parser *p, struct record *record, struct pos at) {
    struct pos open;
    TRY(expect_punct(p, "{", &open));
    struct member_decl *members = NULL;
    uint n = 0;
    uint cap = 0;
    struct map seen;
    memset(&seen, 0, sizeof seen);
    /* The layout so far, to reject a struct too big before laying it out. */
    uint64_t end = 0;
    while (!eat_punct(p, "}")) {
        if (!at_decl(p)) {
            if (is_eof(p)) {
                return expect_close(p, "}", open, NULL);
            }
            return expected_specifier(p);
        }
        struct specs specs;
        TRY(specifiers(p, CTX_MEMBER, &specs));
        if (is_punct(p, ";")) {
            /* C11's anonymous members, which merlin's language has none of. */
            if (specs.anonymous) {
                struct record *r = ty_record(specs.ty);
                const char *kw = r != NULL ? record_keyword(r) : "struct";
                return diag_error(specs.pos, fmt("anonymous %ss are not supported", kw));
            }
            advance(p);
            nothing_declared(p, &specs);
            continue;
        }
        for (;;) {
            struct decl d;
            TRY(declarator(p, specs.ty, CTX_MEMBER, &d));
            folded(p, &d, CTX_MEMBER, CLASS_NONE, 0);
            if (is_punct(p, ":")) {
                return diag_error(here(p), "bit-fields are not supported yet");
            }
            if (ty_is_func(d.ty)) {
                return diag_error(d.pos, fmt("field '%s' declared as a function", d.name));
            }
            int64_t size = ty_size(d.ty);
            if (size < 0) {
                if (d.ty->kind == TY_ARRAY && !d.ty->has_len) {
                    return diag_error(d.pos, "flexible array members are not supported yet");
                }
                struct diag *e = diag_error(d.pos, fmt("field has incomplete type '%s'", ty_str(d.ty)));
                struct type *u = ty_unqual(d.ty);
                if (u->kind == TY_RECORD) {
                    diag_note(e, u->rec->pos, fmt("forward declaration of '%s'", ty_str(u)));
                }
                return e;
            }
            struct pos *prev = map_get(&seen, d.name);
            map_put(&seen, d.name, pos_box(d.pos));
            if (prev != NULL) {
                return diag_note(diag_error(d.pos, fmt("duplicate member '%s'", d.name)), *prev,
                                 "previous declaration is here");
            }
            if (record->is_union) {
                if ((uint64_t)size > end) {
                    end = (uint64_t)size;
                }
            } else {
                end = align_up(end, ty_align(d.ty)) + (uint64_t)size;
            }
            if (end > MAX_OBJECT_SIZE) {
                return diag_error(d.pos, fmt("%s is too large", record_keyword(record)));
            }
            members = grow(members, &cap, n + 1, sizeof(struct member_decl));
            members[n].name = d.name;
            members[n].ty = d.ty;
            members[n].spelled = specs.spelled;
            members[n].pos = d.pos;
            members[n].begin = specs.pos;
            n++;
            if (!eat_punct(p, ",")) {
                break;
            }
        }
        /* The last member's `;` left out: clang only warns. */
        if (is_punct(p, "}")) {
            warn(p, diag_warning_unflagged(after_prev(p), "expected ';' at end of declaration list"));
        } else {
            TRY(expect_semi(p, "at end of declaration list"));
        }
    }
    if (n == 0) {
        return diag_error(open, fmt("%s has no members", record_keyword(record)));
    }
    uint align = 1;
    for (uint i = 0; i < n; i++) {
        if (ty_align(members[i].ty) > align) {
            align = ty_align(members[i].ty);
        }
    }
    if (align_up(end, align) > MAX_OBJECT_SIZE) {
        return diag_error(open, fmt("%s is too large", record_keyword(record)));
    }
    record_complete(record, members, n, at);
    heap_free(members);
    return NULL;
}

static struct diag *cond_expr(struct parser *p, struct expr **out);

/* After enum: a definition, whose constants go in scope one by one (so
 * `B = A + 1` works), or a reference to a defined tag (merlin's enum_spec).
 * An enum type is unsigned int when no constant is negative and int
 * otherwise; the constants themselves are ints. */
static struct diag *enum_spec(struct parser *p, struct pos kw_pos, struct type **ty, int *declares,
                              struct spelling **spelled) {
    const char *tag = NULL;
    struct pos tag_at = kw_pos;
    if (tok(p)->kind == TK_IDENT) {
        struct token *t = advance(p);
        tag = t->name;
        tag_at = t->pos;
    }
    if (!eat_punct(p, "{")) {
        if (tag == NULL) {
            return expected(p, "identifier or '{'");
        }
        struct tag *t = lookup_tag(p, tag);
        if (t != NULL && t->kind == TAG_ENUM) {
            *ty = t->ty;
            *declares = 0;
            *spelled = t->spelled;
            return NULL;
        }
        if (t != NULL) {
            return tag_mismatch(kw_pos, tag, tag_pos(t));
        }
        return diag_error(tag_at,
                          fmt("use of undefined 'enum %s' (merlin needs an enum's constants before its name)", tag));
    }
    if (tag != NULL) {
        struct tag *t = map_get(&innermost(p)->tags, tag);
        if (t != NULL && t->kind == TAG_ENUM) {
            return diag_note(diag_error(tag_at, fmt("redefinition of '%s'", tag)), t->pos,
                             "previous definition is here");
        }
        if (t != NULL) {
            return tag_mismatch(kw_pos, tag, tag_pos(t));
        }
    }
    i128 int_min = ik_min(IK_INT);
    i128 int_max = ik_max(IK_INT);
    i128 next = i128_of(0);
    int negative = 0;
    struct enum_const *constants = NULL;
    uint n = 0;
    uint cap = 0;
    for (;;) {
        if (is_punct(p, "}")) {
            if (n == 0) {
                return diag_error(here(p), "use of empty enum");
            }
            advance(p);
            break;
        }
        const char *name;
        struct pos pos;
        TRY(expect_ident(p, &name, &pos));
        int valued = is_punct(p, "=");
        i128 value;
        if (eat_punct(p, "=")) {
            struct pos at = here(p);
            struct expr *e;
            TRY(cond_expr(p, &e));
            int sure;
            const_int(p, e, at, "expression is not an integer constant expression",
                      "integer constant expression must have integer type, not", &value, &sure);
            /* Not sure: sema reports the error. */
            if (!sure) {
                value = i128_of(0);
            }
        } else {
            value = next;
        }
        if (i128_lt(value, int_min) || i128_lt(int_max, value)) {
            return diag_error(pos, fmt("enumerator value %s is not representable as an 'int'", i128_str(value)));
        }
        struct name *nm = name_new(NAME_ENUM_CONST, NULL, pos);
        nm->value = value;
        TRY(bind_name(p, name, nm));
        if (i128_neg_p(value)) {
            negative = 1;
        }
        next = i128_add(value, i128_of(1));
        constants = grow(constants, &cap, n + 1, sizeof(struct enum_const));
        constants[n].name = name;
        constants[n].value = value;
        n++;
        if (eat_punct(p, ",") || is_punct(p, "}")) {
            continue;
        }
        /* Another enumerator: clang takes the comma to be missing. */
        if (tok(p)->kind == TK_IDENT) {
            return diag_error(after_prev(p), "missing ',' between enumerators");
        }
        if (valued) {
            return expected(p, "'}' or ','");
        }
        return expected(p, "'= constant-expression' or end of enumerator definition");
    }
    struct type *t = ty_int(negative ? IK_INT : IK_UINT);
    struct enum_type *en = arena_alloc(sizeof *en);
    en->tag = tag;
    en->ty = t;
    en->constants = arena_alloc(n * sizeof(struct enum_const));
    memcpy(en->constants, constants, n * sizeof(struct enum_const));
    en->nconstants = n;
    en->pos = kw_pos;
    heap_free(constants);
    struct spelling *s = spelling_enum(en);
    /* The constants now name their enum, for the code after it. */
    struct scope *sc = innermost(p);
    for (uint i = 0; i < n; i++) {
        struct name *nm = map_get(&sc->names, en->constants[i].name);
        if (nm != NULL && nm->kind == NAME_ENUM_CONST) {
            nm->spelled = s;
        }
    }
    if (tag != NULL) {
        struct tag *tg = arena_alloc(sizeof *tg);
        tg->kind = TAG_ENUM;
        tg->ty = t;
        tg->spelled = s;
        tg->pos = tag_at;
        bind_tag(p, tag, tg);
    }
    TRY(after_definition(p, "enum"));
    *ty = t;
    *declares = 1;
    *spelled = s;
    return NULL;
}

/* A keyword that is a type specifier, or a typedef name. */
static int type_specifier(struct parser *p, struct token *t) {
    if (t->kind == TK_KW) {
        switch (t->kw) {
        case KW_VOID:
        case KW_CHAR:
        case KW_SHORT:
        case KW_INT:
        case KW_LONG:
        case KW_SIGNED:
        case KW_UNSIGNED:
        case KW_BOOL:
        case KW_STRUCT:
        case KW_UNION:
        case KW_ENUM:
            return 1;
        default:
            return 0;
        }
    }
    if (t->kind == TK_IDENT) {
        struct name *n = lookup_name(p, t->name);
        return n != NULL && n->kind == NAME_TYPEDEF;
    }
    return 0;
}

/* After a struct, union or enum's `}`, what can follow a type specifier, as
 * clang's isValidAfterTypeSpecifier has it, else clang's error that the
 * `;` is missing after it. */
static struct diag *after_definition(struct parser *p, const char *kw) {
    struct token *t = tok(p);
    int valid = 0;
    if (t->kind == TK_IDENT) {
        valid = 1;
    } else if (t->kind == TK_PUNCT) {
        static const char *const ok[] = {";", "*", ")", "(", ",", "[", "...", ":", "}"};
        for (uint i = 0; i < sizeof ok / sizeof ok[0]; i++) {
            if (strcmp(t->name, ok[i]) == 0) {
                valid = 1;
            }
        }
    } else if (t->kind == TK_KW) {
        switch (t->kw) {
        case KW_CONST:
        case KW_VOLATILE:
        case KW_RESTRICT:
        case KW_INLINE:
        case KW_STATIC:
        case KW_EXTERN:
        case KW_TYPEDEF:
        case KW_REGISTER:
        case KW_AUTO:
            valid = !type_specifier(p, peek_at(p, 1));
            break;
        default:
            break;
        }
    }
    if (valid) {
        return NULL;
    }
    return diag_error(after_prev(p), fmt("expected ';' after %s", kw));
}

/* const, volatile and restrict after a `*`. */
static uint qualifiers(struct parser *p) {
    uint q = 0;
    for (;;) {
        if (is_kw(p, KW_CONST)) {
            q |= Q_CONST;
        } else if (is_kw(p, KW_VOLATILE)) {
            q |= Q_VOLATILE;
        } else if (!is_kw(p, KW_RESTRICT)) {
            return q;
        }
        advance(p);
    }
}

/* ---- Declarators ---- */

/* A growable list of declarator steps. */
struct derives {
    struct derive *v;
    uint n;
    uint cap;
};

static struct derive *derive_push(struct derives *l, int kind) {
    l->v = grow(l->v, &l->cap, l->n + 1, sizeof(struct derive));
    struct derive *d = &l->v[l->n++];
    memset(d, 0, sizeof *d);
    d->kind = kind;
    return d;
}

/* Types nest like expressions, so a declarator may have at most MAX_DEPTH
 * `*`s and as many `[]`s. */
static struct diag *too_deep(struct parser *p, uint n, const char *what) {
    if (n == MAX_DEPTH) {
        return diag_error(here(p), fmt("type nesting too deep: merlin allows at most %u %s", MAX_DEPTH, what));
    }
    return NULL;
}

/* After the `*`s, whether a `(` opens a parenthesized declarator rather
 * than a parameter list. */
static int nested_declarator_follows(struct parser *p) {
    struct token *t = peek_at(p, 1);
    if (tok_punct(t, "*") || tok_punct(t, "(")) {
        return 1;
    }
    return t->kind == TK_IDENT && !is_typedef(p, t);
}

static struct diag *params(struct parser *p, struct pos open, struct param **out, uint *n, int *variadic,
                           struct scope **scope);

/* `('[' constant? ']' | '(' params ')')*` after a declarator's name, in
 * source order. */
static struct diag *suffixes(struct parser *p, struct derives *out) {
    uint dims = 0;
    for (;;) {
        if (is_punct(p, "[")) {
            TRY(too_deep(p, dims, "array dimensions"));
            struct pos open = advance(p)->pos;
            /* [const 3], [static 3]: qualifiers and static before the size
             * (declarator checks they are on a parameter's outermost
             * array). */
            int has_quals = 0;
            uint quals = 0;
            int has_static = 0;
            struct pos static_at = open;
            for (;;) {
                if (is_kw(p, KW_STATIC)) {
                    has_static = 1;
                    static_at = advance(p)->pos;
                    has_quals = 1;
                } else if (is_kw(p, KW_CONST) || is_kw(p, KW_VOLATILE) || is_kw(p, KW_RESTRICT)) {
                    quals |= qualifiers(p);
                    has_quals = 1;
                } else {
                    break;
                }
            }
            if (has_static && is_punct(p, "]")) {
                return diag_error(static_at, "'static' may not be used without an array size");
            }
            struct pos size = here(p);
            if (eat_punct(p, "]")) {
                struct derive *d = derive_push(out, D_ARRAY);
                d->open = open;
                d->size = size;
                d->has_quals = has_quals;
                d->array_quals = quals;
                d->is_static = has_static;
                d->constant = 1;
                dims++;
                continue;
            }
            uint first = p->unevaluated.n;
            struct expr *e;
            TRY(cond_expr(p, &e));
            TRY(expect_close(p, "]", open, NULL));
            int outer = p->folding;
            p->folding = 0;
            i128 n;
            int sure;
            const_int(p, e, size, NOT_CONSTANT_SIZE, "size of array has non-integer type", &n, &sure);
            int was_folding = p->folding;
            p->folding = outer;
            struct range entries;
            entries.start = first;
            entries.end = p->unevaluated.n;
            /* A size clang's evaluator notes as no constant expression clang
             * folds wherever it is, with its warning where the size begins,
             * given unless the size has an error. */
            struct pos note_at;
            char *note;
            if (sure && consteval_parsed_note(e, &note_at, &note)) {
                was_folding = 0;
                struct unevaluated *u = vec_last(&p->unevaluated);
                u->diagnostic = diag_note(diag_warning(size, FOLDED_MSG, FOLDED_FLAG), note_at, note);
                u->with = entries.end - entries.start - 1;
            }
            /* Not sure: sema reports the error, and the size is not used. */
            struct derive *d = derive_push(out, D_ARRAY);
            d->has_len = 1;
            d->len = sure ? n : i128_of(1);
            d->open = open;
            d->size = size;
            d->has_quals = has_quals;
            d->array_quals = quals;
            d->is_static = has_static;
            d->has_folded = was_folding;
            d->folded = size;
            d->entries = entries;
            d->constant = sure;
            dims++;
        } else if (is_punct(p, "(")) {
            struct pos open = advance(p)->pos;
            int prototyped = !is_punct(p, ")");
            struct param *ps;
            uint np;
            int variadic;
            struct scope *scope;
            TRY(params(p, open, &ps, &np, &variadic, &scope));
            struct derive *d = derive_push(out, D_FUNC);
            d->params = ps;
            d->nparams = np;
            d->variadic = variadic;
            d->prototyped = prototyped;
            d->scope = scope;
            d->open = open;
            dims = 0;
        } else {
            return NULL;
        }
    }
}

/* Reads a declarator: its name (if any), where the name is (or would be),
 * and its steps in the order they apply to the base type. */
static struct diag *declarator_ops(struct parser *p, int ctx, const char **name, struct pos *pos,
                                   struct derives *ops) {
    struct derives ptrs;
    memset(&ptrs, 0, sizeof ptrs);
    while (is_punct(p, "*")) {
        TRY(too_deep(p, ptrs.n, "'*'s"));
        advance(p);
        derive_push(&ptrs, D_PTR)->quals = qualifiers(p);
    }
    *pos = here(p);
    struct derives inner;
    memset(&inner, 0, sizeof inner);
    if (is_punct(p, "(") && nested_declarator_follows(p)) {
        struct pos open = advance(p)->pos;
        TRY(parse_enter(p));
        struct diag *e = declarator_ops(p, ctx, name, pos, &inner);
        p->depth--;
        if (e != NULL) {
            return e;
        }
        TRY(expect_close(p, ")", open, NULL));
    } else if (tok(p)->kind == TK_IDENT) {
        if (ctx == CTX_TYPE_NAME) {
            struct diag *e = expected(p, "')'");
            return p->type_open_matched ? diag_note(e, p->type_open, "to match this '('") : e;
        }
        *name = advance(p)->name;
    } else if (ctx == CTX_PARAM || ctx == CTX_TYPE_NAME) {
        *name = "";
    } else if (ctx == CTX_MEMBER) {
        return expected(p, "member name or ';' after declaration specifiers");
    } else {
        return expected(p, "identifier or '('");
    }
    struct derives sfx;
    memset(&sfx, 0, sizeof sfx);
    TRY(suffixes(p, &sfx));
    memset(ops, 0, sizeof *ops);
    for (uint i = 0; i < ptrs.n; i++) {
        *derive_push(ops, 0) = ptrs.v[i];
    }
    for (uint i = sfx.n; i > 0; i--) {
        *derive_push(ops, 0) = sfx.v[i - 1];
    }
    for (uint i = 0; i < inner.n; i++) {
        *derive_push(ops, 0) = inner.v[i];
    }
    heap_free(ptrs.v);
    heap_free(sfx.v);
    heap_free(inner.v);
    return NULL;
}

/* The sizes of a declarator's own arrays that are no constants (at sizes,
 * through reach), which sema reports as merlin's error of a variable-length
 * array, or as clang's where it can have none: at file scope and in a
 * member, at the declarator's name. */
static void variable_sizes(struct parser *p, struct decl *d, int ctx, struct pos *sizes, uint nsizes, int r) {
    if (nsizes == 0) {
        return;
    }
    const char *msg;
    if (ctx == CTX_FILE && r == REACH_ARRAYS) {
        msg = "variable length array declaration not allowed at file scope";
    } else if (ctx == CTX_FILE && r == REACH_FUNCTION) {
        msg = "function declaration cannot have variably modified type";
    } else if (ctx == CTX_FILE) {
        msg = "variably modified type declaration not allowed at file scope";
    } else if (ctx == CTX_MEMBER) {
        msg = "fields must have a constant size: 'variable length array in structure' extension will never be "
              "supported";
    } else {
        return;
    }
    /* A size's own (not an operand ?: did not choose). */
    for (uint i = d->unevaluated.start; i < d->unevaluated.end; i++) {
        struct unevaluated *u = unevaluated_at(p, i);
        if (u->folded || !u->has_untyped) {
            continue;
        }
        for (uint k = 0; k < nsizes; k++) {
            if (pos_eq(sizes[k], u->untyped_pos) && strcmp(u->untyped_msg, NOT_CONSTANT_SIZE) == 0) {
                u->untyped_pos = d->pos;
                u->untyped_msg = msg;
                break;
            }
        }
    }
}

/* A declarator, applied to base. In a parameter list or a type name the
 * name may be missing. */
static struct diag *declarator(struct parser *p, struct type *base, int ctx, struct decl *out) {
    uint start = p->unevaluated.n;
    const char *name = "";
    struct pos pos;
    struct derives ops;
    TRY(declarator_ops(p, ctx, &name, &pos, &ops));
    int folded_reach = reach(ops.v, ops.n, 0);
    int variable = reach(ops.v, ops.n, 1);
    memset(out, 0, sizeof *out);
    struct type *ty = base;
    struct pos *sizes = NULL;
    uint nsizes = 0;
    uint capsizes = 0;
    uint capinner = 0;
    /* An array larger than merlin lays out, though clang takes it. */
    int has_too_large = 0;
    struct pos too_large = pos;
    /* The size of ty, kept as it grows (asking at every dimension would be
     * quadratic in their number). -1 for none. */
    int64_t size = ty_size(ty);
    uint steps = ops.n;
    for (uint i = 0; i < steps; i++) {
        struct derive *op = &ops.v[i];
        if (op->kind == D_PTR) {
            ty = ty_qualified(ty_ptr(ty), op->quals);
            size = PTR_SIZE;
        } else if (op->kind == D_ARRAY) {
            if (!out->has_folded && op->has_folded) {
                out->has_folded = 1;
                out->folded = op->folded;
            }
            /* The last step is what is declared: its own array. */
            if (op->has_folded) {
                struct range entries;
                entries.start = op->entries.start - start;
                entries.end = op->entries.end - start;
                if (i + 1 == steps) {
                    out->has_top = 1;
                    out->top = entries;
                } else {
                    out->inner = grow(out->inner, &capinner, out->ninner + 1, sizeof(struct range));
                    out->inner[out->ninner++] = entries;
                }
            }
            if (!op->constant) {
                sizes = grow(sizes, &capsizes, nsizes + 1, sizeof(struct pos));
                sizes[nsizes++] = op->size;
            }
            /* Only a parameter's outermost array becomes a pointer, the one
             * thing the qualifiers in the brackets can apply to. */
            if (op->has_quals && !(ctx == CTX_PARAM && i + 1 == steps)) {
                const char *what = op->is_static ? "'static'" : "type qualifier";
                const char *place = ctx == CTX_PARAM ? "non-outermost array type derivation"
                                                     : "array declarator outside of function prototype";
                return diag_error(op->open, fmt("%s used in %s", what, place));
            }
            if (ty_is_func(ty)) {
                const char *what = name[0] == 0 ? "type name" : name;
                return diag_error(op->open,
                                  fmt("'%s' declared as array of functions of type '%s'", what, ty_str(ty)));
            }
            if (size < 0) {
                struct diag *e = diag_error(op->open, fmt("array has incomplete element type '%s'", ty_str(ty)));
                struct type *u = ty_unqual(ty);
                if (u->kind == TY_RECORD) {
                    diag_note(e, u->rec->pos, fmt("forward declaration of '%s'", ty_str(u)));
                }
                return e;
            }
            if (op->has_len && i128_neg_p(op->len)) {
                const char *msg = name[0] == 0 ? "array size is negative"
                                               : fmt("'%s' declared as an array with a negative size", name);
                return diag_error(op->size, msg);
            }
            /* A GNU extension clang takes: merlin's objects all have a size. */
            if (op->has_len && i128_is_zero(op->len)) {
                return diag_error(op->size, "zero-length arrays are not supported");
            }
            i128 n = op->has_len ? op->len : i128_of(1);
            i128 total = i128_mul(i128_of(size), n);
            /* clang's limit: the size in bytes has to fit in a size_t (of 61
             * bits at most). */
            uint bits = 8 * PTR_SIZE < 61 ? 8 * PTR_SIZE : 61;
            if (!i128_is_zero(i128_shr(total, bits))) {
                return diag_error(op->size, fmt("array is too large (%s elements)", grouped(n)));
            }
            if (i128_lt(i128_of(MAX_OBJECT_SIZE), total) && !has_too_large) {
                has_too_large = 1;
                too_large = op->size;
            }
            size = op->has_len ? (int64_t)total.lo : -1;
            uint len = op->has_len ? (op->len.hi != 0 || op->len.lo > 0xffffffffu ? 0xffffffffu : (uint)op->len.lo) : 0;
            ty = ty_array(ty, op->has_len, len);
            if (op->has_quals) {
                /* The parameter adjustment, done here so the qualifiers land
                 * on the pointer: `int a[const 3]` declares `int *const a`. */
                ty = ty_qualified(ty_decay(ty), op->array_quals);
                size = PTR_SIZE;
            }
        } else {
            if (ty_is_array(ty) || ty_is_func(ty)) {
                const char *what = ty_is_array(ty) ? "array" : "function";
                return diag_error(op->open, fmt("function cannot return %s type '%s'", what, ty_str(ty)));
            }
            /* A qualified result is part of the type, though a call's value
             * is unqualified (sema). */
            struct type **types = arena_alloc(op->nparams * sizeof(struct type *) + 1);
            for (uint k = 0; k < op->nparams; k++) {
                types[k] = ty_unqual(op->params[k].ty);
            }
            ty = ty_func(ty, types, op->nparams, op->variadic);
            ty->fn->spelled = arena_alloc(op->nparams * sizeof(struct param_spelling) + 1);
            for (uint k = 0; k < op->nparams; k++) {
                ty->fn->spelled[k].ty = op->params[k].ty;
                ty->fn->spelled[k].spelled = op->params[k].spelled;
            }
            ty->fn->nspelled = op->nparams;
            ty->fn->unprototyped = (uchar)!op->prototyped;
            size = -1;
            if (i + 1 == steps) {
                out->has_func = 1;
                out->params = op->params;
                out->nparams = op->nparams;
                out->scope = op->scope;
                out->prototyped = op->prototyped;
                out->has_open = 1;
                out->open = op->open;
            }
        }
    }
    heap_free(ops.v);
    if (has_too_large) {
        return diag_error(too_large,
                          fmt("array is too large: merlin's objects are at most %u bytes", MAX_OBJECT_SIZE));
    }
    out->name = name;
    out->ty = ty;
    out->pos = pos;
    out->reach = folded_reach;
    out->unevaluated.start = start;
    out->unevaluated.end = p->unevaluated.n;
    variable_sizes(p, out, ctx, sizes, nsizes, variable);
    heap_free(sizes);
    /* A parameter is never folded (see folded); a type name is as its use
     * says (type_name_folded). */
    if (ctx == CTX_PARAM) {
        folded(p, out, ctx, CLASS_NONE, 0);
    }
    return NULL;
}

/* `int f(x, y)`: a list of parameter names, C89's, whose types are declared
 * after it, before the body. merlin's language has no such definitions:
 * clang's error on a name the list leaves undeclared, as it does when the
 * body follows, or where a declaration has the list. */
static struct diag *identifier_list(struct parser *p, struct pos open) {
    const char *last = NULL;
    struct pos last_pos;
    struct pos first;
    uint n = 0;
    for (;;) {
        const char *name;
        struct pos pos;
        TRY(expect_ident(p, &name, &pos));
        if (n++ == 0) {
            first = pos;
        }
        last = name;
        last_pos = pos;
        if (eat_punct(p, ")")) {
            break;
        }
        if (!eat_punct(p, ",")) {
            return expect_close(p, ")", open, NULL);
        }
    }
    if (is_punct(p, "{")) {
        struct diag *e = diag_error(last_pos, fmt("parameter '%s' was not declared, defaults to 'int'; ISO C99 and "
                                                  "later do not support implicit int",
                                                  last));
        e->flag = "implicit-int";
        return e;
    }
    if (at_decl(p)) {
        return diag_error(first, "old-style parameter declarations are not supported");
    }
    return diag_error(first, "a parameter list without types is only allowed in a function definition");
}

static struct diag *params_inner(struct parser *p, struct pos open, struct param **out, uint *n, int *variadic) {
    struct param *ps = NULL;
    uint count = 0;
    uint cap = 0;
    *variadic = 0;
    *out = NULL;
    *n = 0;
    if (eat_punct(p, ")")) {
        return NULL;
    }
    if (is_kw(p, KW_VOID) && tok_punct(peek_at(p, 1), ")")) {
        advance(p);
        advance(p);
        return NULL;
    }
    for (;;) {
        if (is_punct(p, "...")) {
            /* C99 wants a named parameter before `...` (C23 does not). */
            if (count == 0) {
                return diag_error(here(p), "ISO C requires a named parameter before '...'");
            }
            advance(p);
            TRY(expect_close(p, ")", open, NULL));
            *variadic = 1;
            break;
        }
        if (!at_decl(p)) {
            /* `int f(unsinged x)`: a type's name, misspelt or none. */
            if (tok(p)->kind == TK_IDENT && peek_at(p, 1)->kind == TK_IDENT) {
                return unknown_type_name(p, tok(p)->name);
            }
            if (count == 0 && tok(p)->kind == TK_IDENT) {
                return identifier_list(p, open);
            }
            return expected(p, "parameter declarator");
        }
        struct specs specs;
        TRY(specifiers(p, CTX_PARAM, &specs));
        struct decl d;
        TRY(declarator(p, specs.ty, CTX_PARAM, &d));
        /* `(V)` with `typedef void V;` is `(void)` too. */
        if (count == 0 && d.name[0] == 0 && ty_eq(d.ty, TY_VOID_T) && eat_punct(p, ")")) {
            break;
        }
        struct pos pos = d.pos;
        /* Any other void parameter is clang's error. */
        if (ty_is_void(d.ty)) {
            if (d.name[0] != 0) {
                return diag_error(pos, "argument may not have 'void' type");
            }
            if (count == 0 && is_punct(p, ")")) {
                return diag_error(open, "'void' as parameter must not have type qualifiers");
            }
            return diag_error(pos, "'void' must be the first and only parameter if specified");
        }
        if (count == MAX_PARAMS) {
            return diag_error(pos, fmt("too many parameters: merlin supports at most %u", MAX_PARAMS));
        }
        /* C adjusts a parameter declared as an array or a function to a
         * pointer (keeping qualifiers on what it points to). */
        struct spelling *spelled = decayed_spelling(d.ty, specs.spelled);
        struct type *array = ty_is_array(d.ty) ? d.ty : NULL;
        struct type *ty = ty_is_array(d.ty) || ty_is_func(d.ty) ? ty_decay(d.ty) : d.ty;
        if (d.name[0] != 0) {
            struct name *prev = map_get(&innermost(p)->names, d.name);
            if (prev != NULL) {
                return diag_note(diag_error(pos, fmt("redefinition of parameter '%s'", d.name)), prev->pos,
                                 "previous declaration is here");
            }
            TRY(bind_name(p, d.name, name_new(NAME_OBJECT, ty, pos)));
            struct proto_param *pp = arena_alloc(sizeof *pp);
            pp->name = d.name;
            pp->ty = ty;
            pp->pos = pos;
            vec_push(&p->prototype, pp);
        }
        ps = grow(ps, &cap, count + 1, sizeof(struct param));
        memset(&ps[count], 0, sizeof ps[count]);
        ps[count].name = d.name;
        ps[count].ty = ty;
        ps[count].pos = pos;
        ps[count].spelled = spelled;
        ps[count].register_ = specs.class == CLASS_REGISTER;
        ps[count].array = array;
        count++;
        if (eat_punct(p, ")")) {
            break;
        }
        if (!eat_punct(p, ",")) {
            return expect_close(p, ")", open, NULL);
        }
    }
    *out = arena_alloc(count * sizeof(struct param) + 1);
    if (count > 0) {
        memcpy(*out, ps, count * sizeof(struct param));
    }
    heap_free(ps);
    *n = count;
    return NULL;
}

/* After `(` (at open), up to and including `)`: the parameters, in a scope
 * of their own, and whether `...` ends them. */
static struct diag *params(struct parser *p, struct pos open, struct param **out, uint *n, int *variadic,
                           struct scope **scope) {
    push_scope(p, scope_new());
    uint outer = p->prototype.n;
    struct diag *e = params_inner(p, open, out, n, variadic);
    vec_truncate(&p->prototype, outer);
    *scope = pop_scope(p);
    return e;
}

/* A type name's declarator, and how it spelled its base, in parentheses
 * opened at open, which clang matches its `)` with if `matched` (a cast's,
 * sizeof's and a compound literal's). What it folded is for its use to
 * say. */
static struct diag *type_name_decl(struct parser *p, struct pos open, int matched, struct decl *d,
                                   struct spelling **spelled) {
    p->type_open = open;
    p->type_open_matched = matched;
    struct specs specs;
    TRY(specifiers(p, CTX_TYPE_NAME, &specs));
    TRY(declarator(p, specs.ty, CTX_TYPE_NAME, d));
    *spelled = specs.spelled;
    return NULL;
}

/* A type name, __builtin_va_arg's: in parentheses opened at open, whose `)`
 * clang does not match with it. */
static struct diag *type_name(struct parser *p, struct pos open, struct type **ty) {
    struct decl d;
    struct spelling *s;
    TRY(type_name_decl(p, open, 0, &d, &s));
    type_name_folded(p, &d, 0, open);
    *ty = d.ty;
    return NULL;
}

/* ---- Initializers ---- */

static struct diag *assign_expr(struct parser *p, struct expr **out);

/* `.member` and `[index]` designators, through the `=`. A lone `[index]`
 * may go without it (`[0] 1`): GNU's older syntax, which clang takes with a
 * warning. */
static struct diag *designation(struct parser *p, struct designator ***out, uint *n) {
    struct designator **ds = NULL;
    uint count = 0;
    uint cap = 0;
    for (;;) {
        struct designator *d;
        if (is_punct(p, ".")) {
            struct pos pos = advance(p)->pos;
            if (tok(p)->kind != TK_IDENT) {
                return expected(p, "a field designator, such as '.field = 4'");
            }
            struct token *t = advance(p);
            d = arena_alloc(sizeof *d);
            d->member = 1;
            d->name = t->name;
            d->pos = pos;
            d->at = t->pos;
        } else if (is_punct(p, "[")) {
            struct pos pos = advance(p)->pos;
            struct expr *index;
            TRY(cond_expr(p, &index));
            TRY(expect_close(p, "]", pos, NULL));
            d = arena_alloc(sizeof *d);
            d->index = index;
            d->pos = pos;
        } else {
            break;
        }
        ds = grow(ds, &cap, count + 1, sizeof(struct designator *));
        ds[count++] = d;
    }
    *out = arena_alloc(count * sizeof(struct designator *) + 1);
    if (count > 0) {
        memcpy(*out, ds, count * sizeof(struct designator *));
    }
    heap_free(ds);
    *n = count;
    if (eat_punct(p, "=")) {
        return NULL;
    }
    if (count == 1 && !(*out)[0]->member) {
        warn(p, diag_warning(here(p), "use of GNU 'missing =' extension in designator", "gnu-designator"));
        return NULL;
    }
    return expected(p, "'=' or another designator");
}

static struct diag *initializer_list(struct parser *p, struct init **out) {
    struct init *list = arena_alloc(sizeof *list);
    list->kind = I_LIST;
    list->pos = advance(p)->pos;
    for (;;) {
        if (eat_punct(p, "}")) {
            *out = list;
            return NULL;
        }
        struct init *item;
        if (is_punct(p, ".") || is_punct(p, "[")) {
            item = arena_alloc(sizeof *item);
            item->kind = I_DESIGNATED;
            TRY(designation(p, &item->ds, &item->nds));
            TRY(initializer(p, &item->inner));
        } else {
            TRY(initializer(p, &item));
        }
        vec_push(&list->items, item);
        if (!eat_punct(p, ",") && !is_punct(p, "}")) {
            return expect_close(p, "}", list->pos, NULL);
        }
    }
}

/* `'{' ... '}'` (empty too, as clang takes it) or an expression. */
static struct diag *initializer(struct parser *p, struct init **out) {
    if (!is_punct(p, "{")) {
        struct init *i = arena_alloc(sizeof *i);
        i->kind = I_EXPR;
        TRY(assign_expr(p, &i->e));
        *out = i;
        return NULL;
    }
    TRY(parse_enter(p));
    struct diag *e = initializer_list(p, out);
    p->depth--;
    return e;
}

/* ---- Declarations ---- */

static struct diag *for_semi(struct parser *p) {
    if (eat_punct(p, ";")) {
        return NULL;
    }
    return expected(p, "';' in 'for' statement specifier");
}

/* clang's error for a declaration's missing `;`, which says where it is. */
static struct diag *end_of_declaration(struct parser *p, int ctx) {
    if (ctx == CTX_FILE) {
        return expect_semi(p, "after top level declarator");
    }
    if (ctx == CTX_FOR_INIT) {
        return for_semi(p);
    }
    return expect_semi(p, "at end of declaration");
}

/* A function declaration or definition from its declarator, with what the
 * declarator left for sema to check. */
static struct function *make_function(struct decl *d, struct param *ps, uint nps, struct specs *specs, int has_body,
                                      struct vec body, struct pos end, struct vec unevaluated) {
    struct function *f = arena_alloc(sizeof *f);
    f->name = d->name;
    f->ret = d->ty->fn->ret;
    f->ret_spelled = specs->spelled;
    f->params = ps;
    f->nparams = nps;
    f->variadic = d->ty->fn->variadic;
    f->is_static = specs->class == CLASS_STATIC;
    f->is_inline = (uchar)specs->has_inline;
    f->is_extern = specs->class == CLASS_EXTERN;
    f->prototyped = (uchar)d->prototyped;
    f->has_body = (uchar)has_body;
    f->body = body;
    f->ret_ptr = -1;
    f->pos = d->pos;
    f->end = end;
    f->unevaluated = unevaluated;
    return f;
}

/* A function's declaration (no body) from its declarator. */
static struct function *declared_function(struct decl *d, struct specs *specs, struct vec unevaluated) {
    struct param *ps;
    uint n;
    if (d->has_func) {
        ps = d->params;
        n = d->nparams;
    } else {
        /* Declared through a typedef of a function type: unnamed
         * parameters. */
        struct functype *f = d->ty->fn;
        n = f->nparams;
        ps = arena_alloc(n * sizeof(struct param) + 1);
        for (uint i = 0; i < n; i++) {
            ps[i].name = "";
            ps[i].ty = f->params[i];
            ps[i].pos = d->pos;
        }
    }
    struct vec none;
    memset(&none, 0, sizeof none);
    return make_function(d, ps, n, specs, 0, none, d->pos, unevaluated);
}

static struct item *item_new(int kind) {
    struct item *it = arena_alloc(sizeof *it);
    it->kind = kind;
    return it;
}

/* The declarators of a declaration of objects and functions, from first on,
 * through the `;` (merlin's init_declarators); what the declaration left
 * for sema to check from start on goes with them. At file scope a function
 * is an item of its own, between the declarations of the objects before
 * and after it; in a block it is a declarator, which sema binds to the
 * function outside. The items go to *items. */
static struct diag *init_declarators(struct parser *p, struct specs *specs, struct decl *first, int ctx, uint start,
                                     struct vec *items) {
    int storage = specs->class == CLASS_STATIC     ? ST_STATIC
                  : specs->class == CLASS_EXTERN   ? ST_EXTERN
                  : specs->class == CLASS_REGISTER ? ST_REGISTER
                                                   : ST_NONE;
    struct vec decls;
    memset(&decls, 0, sizeof decls);
    memset(items, 0, sizeof *items);
    for (;;) {
        struct decl d;
        if (first != NULL) {
            d = *first;
            first = NULL;
        } else {
            TRY(declarator(p, specs->ty, ctx, &d));
        }
        if (ty_is_func(d.ty) && is_punct(p, "=")) {
            return diag_error(d.pos, "illegal initializer (only variables can be initialized)");
        }
        /* register and auto are for objects in a block (C99 6.9p2). */
        if (specs->class == CLASS_REGISTER || specs->class == CLASS_AUTO) {
            if (ty_is_func(d.ty)) {
                return diag_error(specs->class_pos, "illegal storage class on function");
            }
            if (ctx == CTX_FILE) {
                return diag_error(d.pos, "illegal storage class on file-scoped variable");
            }
        }
        if (ty_is_func(d.ty) && ctx == CTX_FILE) {
            folded(p, &d, ctx, specs->class, 0);
            /* What the function's declarator left, checked at file scope. */
            struct vec unevaluated = vec_split_off(&p->unevaluated, start);
            TRY(bind_name(p, d.name, name_new(NAME_OBJECT, d.ty, d.pos)));
            if (decls.n > 0) {
                struct item *it = item_new(ITEM_DECL);
                it->decls = vec_take(&decls);
                vec_push(items, it);
            }
            struct item *it = item_new(ITEM_FUNC);
            it->func = declared_function(&d, specs, unevaluated);
            vec_push(items, it);
            if (!eat_punct(p, ",")) {
                break;
            }
            continue;
        }
        if (ty_is_func(d.ty) && specs->class == CLASS_STATIC) {
            return diag_error(specs->class_pos, "function declared in block scope cannot have 'static' storage class");
        }
        if (specs->has_inline && !ty_is_func(d.ty)) {
            return diag_error(specs->inline_pos, "'inline' can only appear on functions");
        }
        /* A block's declaration with linkage has the composite type of it
         * and the earlier one it can see (C99 6.2.7p4). */
        int linked = ctx != CTX_FILE && (specs->class == CLASS_EXTERN || ty_is_func(d.ty));
        struct type *ty = linked ? visible_composite(p, d.name, d.ty) : d.ty;
        TRY(bind_name(p, d.name, name_new(NAME_OBJECT, ty, d.pos)));
        if (linked) {
            map_put(&innermost(p)->linked, d.name, (void *)d.name);
        }
        struct init *init = NULL;
        if (eat_punct(p, "=")) {
            TRY(initializer(p, &init));
        }
        folded(p, &d, ctx, specs->class, init != NULL);
        /* What its declarator left, and what its initializer did. */
        struct vec unevaluated = vec_split_off(&p->unevaluated, d.unevaluated.end);
        struct vec sized = vec_split_off(&p->unevaluated, start);
        /* `int a[] = {1, 2, 3};`: the size the initializer gives, for a later
         * sizeof a in an array size. Unless an earlier declaration gave the
         * size. */
        struct name *bound = map_get(&innermost(p)->names, d.name);
        int incomplete = bound != NULL && bound->kind == NAME_OBJECT && ty_size(bound->ty) < 0;
        if (d.ty->kind == TY_ARRAY && !d.ty->has_len && init != NULL && incomplete) {
            uint n;
            if (init_len(d.ty->to, init, &n)) {
                TRY(bind_name(p, d.name, name_new(NAME_OBJECT, ty_array(d.ty->to, 1, n), d.pos)));
            }
        }
        struct declarator *dr = arena_alloc(sizeof *dr);
        dr->name = d.name;
        dr->ty = d.ty;
        dr->storage = storage;
        dr->init = init;
        dr->pos = d.pos;
        dr->begin = specs->pos;
        dr->spelled = specs->spelled;
        dr->sized = sized;
        dr->unevaluated = unevaluated;
        vec_push(&decls, dr);
        if (!eat_punct(p, ",")) {
            break;
        }
    }
    TRY(end_of_declaration(p, ctx));
    if (decls.n > 0) {
        struct item *it = item_new(ITEM_DECL);
        it->decls = decls;
        vec_push(items, it);
    }
    return NULL;
}

/* The declarators of a typedef, each binding a name to its type. */
static struct diag *typedefs(struct parser *p, struct specs *specs, struct decl *first, int ctx) {
    if (specs->has_inline) {
        return diag_error(specs->inline_pos, "'inline' can only appear on functions");
    }
    struct decl d = *first;
    for (;;) {
        if (is_punct(p, "=")) {
            return diag_error(d.pos, "illegal initializer (only variables can be initialized)");
        }
        folded(p, &d, ctx, CLASS_TYPEDEF, 0);
        struct typedef_name *td = arena_alloc(sizeof *td);
        td->name = d.name;
        td->ty = d.ty;
        td->spelled = specs->spelled;
        td->pos = d.pos;
        /* A typedef of a struct, union or enum without a tag names it (the
         * first typedef that is the type itself, unqualified). */
        if (d.ty->kind == TY_RECORD && d.ty->rec->tag == NULL) {
            if (d.ty->rec->typedef_name == NULL) {
                d.ty->rec->typedef_name = d.name;
            }
        } else if (specs->spelled != NULL && specs->spelled->kind == SP_ENUM && specs->spelled->en->tag == NULL &&
                   ty_eq(d.ty, specs->spelled->en->ty)) {
            if (specs->spelled->en->typedef_name == NULL) {
                specs->spelled->en->typedef_name = d.name;
            }
        }
        struct name *n = name_new(NAME_TYPEDEF, d.ty, d.pos);
        n->spelled = spelling_typedef(td);
        TRY(bind_name(p, d.name, n));
        if (!eat_punct(p, ",")) {
            break;
        }
        TRY(declarator(p, specs->ty, ctx, &d));
    }
    return end_of_declaration(p, ctx);
}

/* After specifiers that stand alone, at the `;`: they should declare
 * something, or clang warns. */
static void bare_specifiers(struct parser *p, struct specs *specs) {
    advance(p);
    if (!specs->declares) {
        nothing_declared(p, specs);
    }
}

/* Whether the current token starts a declaration: a type keyword, storage
 * class or qualifier, or a typedef name (that is not a label). */
static int at_decl(struct parser *p) {
    struct token *t = tok(p);
    if (t->kind == TK_KW) {
        return kw_starts_decl(t->kw);
    }
    return is_typedef(p, t) && !tok_punct(peek_at(p, 1), ":");
}

static int type_name_at(struct parser *p, struct token *t) {
    if (t->kind == TK_KW) {
        /* Not a storage class or inline: `(static int)1` is no cast. */
        switch (t->kw) {
        case KW_STATIC:
        case KW_EXTERN:
        case KW_TYPEDEF:
        case KW_REGISTER:
        case KW_AUTO:
        case KW_INLINE:
            return 0;
        default:
            return kw_starts_decl(t->kw);
        }
    }
    return is_typedef(p, t);
}

/* Whether the token after the current one starts a type name: `(` then a
 * type means a cast or sizeof (type). */
static int type_name_follows(struct parser *p) {
    return type_name_at(p, peek_at(p, 1));
}

static int type_name_here(struct parser *p) {
    return type_name_at(p, tok(p));
}

/* clang's error where a declaration should start at file scope and none
 * does. */
static struct diag *not_a_declaration(struct parser *p) {
    struct token *t = tok(p);
    if (tok_punct(t, "}")) {
        return diag_error(here(p), "extraneous closing brace ('}')");
    }
    /* What would start an Objective-C method, which clang tells apart. */
    if (tok_punct(t, "+") || tok_punct(t, "-")) {
        return diag_error(here(p), "expected external declaration");
    }
    if (t->kind == TK_IDENT) {
        struct token *n = peek_at(p, 1);
        static const char *const after[] = {"[", "(", ")", ";", ",", "=", "{", ":"};
        int valid = 0;
        for (uint i = 0; i < sizeof after / sizeof after[0]; i++) {
            if (tok_punct(n, after[i])) {
                valid = 1;
            }
        }
        if (!valid) {
            return unknown_type_name(p, t->name);
        }
        /* C89's implicit int: clang reports the missing type once the
         * declaration is read; a function's declarator that nothing a
         * declaration can go on with follows is its syntax error instead. */
        uint start = p->pos;
        uint warned = p->warnings.n;
        uint kept = p->unevaluated.n;
        uint named = p->undeclared.n;
        struct pos at = here(p);
        struct decl d;
        int func = declarator(p, ty_int(IK_INT), CTX_FILE, &d) == NULL && ty_is_func(d.ty);
        struct token *now = tok(p);
        int goes_on = tok_punct(now, ";") || tok_punct(now, ",") || tok_punct(now, "=") || tok_punct(now, "{");
        struct pos where = here(p);
        p->pos = start;
        vec_truncate(&p->warnings, warned);
        vec_truncate(&p->unevaluated, kept);
        vec_truncate(&p->undeclared, named);
        if (func && !goes_on) {
            return diag_error(where, "expected function body after function declarator");
        }
        struct diag *e = diag_error(at, "type specifier missing, defaults to 'int'; ISO C99 and later do not "
                                        "support implicit int");
        e->flag = "implicit-int";
        return e;
    }
    return expected(p, "identifier or '('");
}

static struct diag *static_assertion(struct parser *p);
static struct diag *block_body(struct parser *p, int new_scope, struct vec *out);

/* A top-level function, or a declaration: the items it makes, none for one
 * that only declares types. */
static struct diag *item(struct parser *p, struct vec *items) {
    memset(items, 0, sizeof *items);
    if (is_kw(p, KW_STATIC_ASSERT)) {
        return static_assertion(p);
    }
    /* An empty declaration, which clang takes. */
    if (eat_punct(p, ";")) {
        return NULL;
    }
    if (!at_decl(p)) {
        return not_a_declaration(p);
    }
    uint start = p->unevaluated.n;
    struct specs specs;
    TRY(specifiers(p, CTX_FILE, &specs));
    if (is_punct(p, ";")) {
        bare_specifiers(p, &specs);
        return NULL;
    }
    struct decl d;
    TRY(declarator(p, specs.ty, CTX_FILE, &d));
    if (specs.class == CLASS_TYPEDEF) {
        return typedefs(p, &specs, &d, CTX_FILE);
    }
    if (!ty_is_func(d.ty) || is_punct(p, ";") || is_punct(p, ",") || is_punct(p, "=")) {
        return init_declarators(p, &specs, &d, CTX_FILE, start, items);
    }
    if (!is_punct(p, "{")) {
        return expected(p, "function body after function declarator");
    }
    /* register and auto are for objects in a block: clang's error, after
     * which it goes on with the function. */
    if (specs.class == CLASS_REGISTER || specs.class == CLASS_AUTO) {
        warn(p, diag_error(specs.class_pos, "illegal storage class on function"));
    }
    folded(p, &d, CTX_FILE, specs.class, 0);
    /* What the function's declarator left, checked at file scope. */
    struct vec unevaluated = vec_split_off(&p->unevaluated, start);
    TRY(bind_name(p, d.name, name_new(NAME_OBJECT, d.ty, d.pos)));
    if (!d.has_func) {
        return diag_error(d.pos, "a function definition needs its parameter list");
    }
    for (uint i = 0; i < d.nparams; i++) {
        if (d.params[i].name[0] == 0) {
            warn(p, diag_warning(d.params[i].pos,
                                 "omitting the parameter name in a function definition is a C23 extension",
                                 "c23-extensions"));
        }
    }
    /* A qualified void result, which no value has: clang's warning of a
     * definition, at its parameter list. */
    if (d.has_open) {
        struct type *ret = d.ty->fn->ret;
        if (ty_is_void(ret) && ty_quals(ret) != 0) {
            warn(p, diag_warning(d.open,
                                 fmt("function cannot return qualified void type %s", ty_named(ret, specs.spelled)),
                                 "qualified-void-return-type"));
        }
    }
    /* The body's outermost block shares the parameters' scope. */
    push_scope(p, d.scope);
    struct vec body;
    struct diag *e = block_body(p, 0, &body);
    pop_scope(p);
    if (e != NULL) {
        /* The function as far as it got: each block cut short ends with the
         * one inside it. */
        struct vec blocks = vec_take(&p->salvaged);
        struct vec inner;
        memset(&inner, 0, sizeof inner);
        if (blocks.n > 0) {
            inner = *(struct vec *)blocks.v[0];
        }
        for (uint i = 1; i < blocks.n; i++) {
            struct vec *outer = blocks.v[i];
            struct stmt *b = stmt_new(S_BLOCK, e->pos);
            b->list = inner;
            vec_push(outer, b);
            inner = *outer;
        }
        struct item *it = item_new(ITEM_FUNC);
        it->func = make_function(&d, d.params, d.nparams, &specs, 1, inner, e->pos, unevaluated);
        p->partial = it;
        return e;
    }
    /* The `}` just read. */
    struct pos end = p->toks[p->pos - 1].pos;
    struct item *it = item_new(ITEM_FUNC);
    it->func = make_function(&d, d.params, d.nparams, &specs, 1, body, end, unevaluated);
    vec_push(items, it);
    return NULL;
}

/* ---- Blocks ---- */

static struct diag *stmt(struct parser *p, struct stmt **out);
static struct diag *decl_stmt(struct parser *p, int ctx, struct stmt **out);

/* The items of a block after its `{` (at open), up to and including its
 * `}`; *item is the expr_next_id of where the item being read began. */
static struct diag *block_items_into(struct parser *p, struct vec *items, struct pos open, uint64_t *item) {
    for (;;) {
        *item = expr_next_id();
        if (eat_punct(p, "}")) {
            return NULL;
        }
        if (is_eof(p)) {
            return expect_close(p, "}", open, NULL);
        }
        struct pos pos = here(p);
        if (is_kw(p, KW_STATIC_ASSERT)) {
            TRY(static_assertion(p));
        } else if (at_decl(p)) {
            struct stmt *s;
            TRY(decl_stmt(p, CTX_BLOCK, &s));
            if (s != NULL) {
                vec_push(items, s);
            }
        } else {
            struct stmt *s;
            TRY(stmt(p, &s));
            vec_push(items, s);
        }
        struct stmt *u = unevaluated_stmt(p, pos);
        if (u != NULL) {
            vec_push(items, u);
        }
    }
}

static struct diag *block_items(struct parser *p, struct vec *out) {
    struct pos open;
    TRY(expect_punct(p, "{", &open));
    /* What the statement around the block left so far (`if (c)`) is checked
     * with that statement, out of the block's scope. */
    struct vec outer = vec_take(&p->unevaluated);
    struct vec items;
    memset(&items, 0, sizeof items);
    uint mark = p->undeclared.n;
    uint64_t item = expr_next_id();
    struct diag *e = block_items_into(p, &items, open, &item);
    if (e != NULL) {
        /* What the error leaves of the block, and the undeclared names of
         * the item it cut short. */
        struct stmt *u = unevaluated_stmt(p, here(p));
        if (u != NULL) {
            vec_push(&items, u);
        }
        struct stmt *c = cut_names(p, mark, item);
        if (c != NULL) {
            vec_push(&items, c);
        }
        struct vec *saved = arena_alloc(sizeof *saved);
        *saved = items;
        vec_push(&p->salvaged, saved);
        p->unevaluated = outer;
        return e;
    }
    p->unevaluated = outer;
    *out = items;
    return NULL;
}

/* `{ item* }`, in a new scope unless new_scope is 0 (a function body
 * continues its parameters' scope). */
static struct diag *block_body(struct parser *p, int new_scope, struct vec *out) {
    if (!new_scope) {
        return block_items(p, out);
    }
    push_scope(p, scope_new());
    struct diag *e = block_items(p, out);
    pop_scope(p);
    return e;
}

/* `int a = 1, *b;`, or NULL for one that only declares types. */
static struct diag *decl_stmt(struct parser *p, int ctx, struct stmt **out) {
    struct pos pos = here(p);
    uint start = p->unevaluated.n;
    struct specs specs;
    *out = NULL;
    TRY(specifiers(p, ctx, &specs));
    if (is_punct(p, ";")) {
        bare_specifiers(p, &specs);
        return NULL;
    }
    struct decl d;
    TRY(declarator(p, specs.ty, ctx, &d));
    if (specs.class == CLASS_TYPEDEF) {
        return typedefs(p, &specs, &d, ctx);
    }
    /* In a block, functions are declarators too: one declaration. */
    struct vec items;
    TRY(init_declarators(p, &specs, &d, ctx, start, &items));
    struct stmt *s = stmt_new(S_DECL, pos);
    for (uint i = 0; i < items.n; i++) {
        struct item *it = items.v[i];
        vec_extend(&s->list, &it->decls);
    }
    *out = s;
    return NULL;
}

/* ---- Statements ---- */

static struct diag *expr(struct parser *p, struct expr **out);

/* `(expr)` after the keyword kw. */
static struct diag *paren_expr(struct parser *p, const char *kw, struct expr **out) {
    if (!is_punct(p, "(")) {
        return expected(p, fmt("'(' after '%s'", kw));
    }
    struct pos open = advance(p)->pos;
    TRY(expr(p, out));
    return expect_close(p, ")", open, NULL);
}

/* The `;` that ends a statement; but where its expression read a name
 * nothing declares (since `named`), which makes it no expression to clang,
 * what is left of it up to its `;` is skipped, as clang skips it. */
static struct diag *semi_or_skip(struct parser *p, const char *after, uint named) {
    if (is_punct(p, ";") || p->undeclared.n == named) {
        return expect_semi(p, after);
    }
    while (!is_punct(p, ";") && !is_punct(p, "}") && !is_eof(p)) {
        advance(p);
    }
    eat_punct(p, ";");
    return NULL;
}

/* What a label (or case or default) labels: a statement, or none at the end
 * of a block, which clang takes with a warning. */
static struct diag *labeled(struct parser *p, struct stmt **out) {
    if (is_punct(p, "}")) {
        warn(p, diag_warning(here(p), "label at end of compound statement is a C23 extension", "c23-extensions"));
        *out = stmt_new(S_EMPTY, here(p));
        return NULL;
    }
    return stmt(p, out);
}

/* After for: `(init; cond; step) body`, in the for's own scope. */
static struct diag *for_rest(struct parser *p, struct stmt *s) {
    uint start = p->unevaluated.n;
    if (!is_punct(p, "(")) {
        return expected(p, "'(' after 'for'");
    }
    struct pos open = advance(p)->pos;
    if (at_decl(p)) {
        TRY(decl_stmt(p, CTX_FOR_INIT, &s->init));
    } else if (!eat_punct(p, ";")) {
        struct pos ipos = here(p);
        struct expr *e;
        TRY(expr(p, &e));
        TRY(for_semi(p));
        s->init = stmt_new(S_EXPR, ipos);
        s->init->e = e;
    }
    if (!is_punct(p, ";")) {
        TRY(expr(p, &s->e));
    }
    TRY(for_semi(p));
    if (!is_punct(p, ")")) {
        TRY(expr(p, &s->step));
    }
    TRY(expect_close(p, ")", open, NULL));
    TRY(stmt(p, &s->body));
    s->list = vec_split_off(&p->unevaluated, start);
    return NULL;
}

static struct diag *stmt_inner(struct parser *p, struct stmt **out) {
    struct pos pos = here(p);
    /* A declaration is no statement (`if (c) int x;`). */
    if (at_decl(p)) {
        return expected(p, "expression");
    }
    if (is_punct(p, "}")) {
        return expected(p, "statement");
    }
    struct token *t = tok(p);
    struct stmt *s;
    if (tok_punct(t, "{")) {
        s = stmt_new(S_BLOCK, pos);
        TRY(block_body(p, 1, &s->list));
    } else if (tok_punct(t, ";")) {
        s = stmt_new(S_EMPTY, pos);
        s->glued = after_prev(p).line == here(p).line;
        advance(p);
    } else if (t->kind == TK_IDENT && tok_punct(peek_at(p, 1), ":")) {
        s = stmt_new(S_LABEL, pos);
        s->name = t->name;
        advance(p);
        advance(p);
        TRY(labeled(p, &s->body));
    } else if (tok_kw(t, KW_IF)) {
        advance(p);
        s = stmt_new(S_IF, pos);
        TRY(paren_expr(p, "if", &s->e));
        TRY(stmt(p, &s->body));
        int inner = p->has_trailing_else;
        struct pos inner_at = p->trailing_else;
        p->has_trailing_else = 0;
        if (is_kw(p, KW_ELSE)) {
            struct pos at = advance(p)->pos;
            TRY(stmt(p, &s->els));
            p->has_trailing_else = 1;
            p->trailing_else = at;
        } else if (inner) {
            /* `if (a) if (b) x; else y;`: the else is the inner if's, which
             * clang warns may not be what was meant. */
            warn(p, diag_warning(inner_at, "add explicit braces to avoid dangling else", "dangling-else"));
        }
        *out = s;
        return NULL;
    } else if (tok_kw(t, KW_SWITCH)) {
        advance(p);
        s = stmt_new(S_SWITCH, pos);
        TRY(paren_expr(p, "switch", &s->e));
        TRY(stmt(p, &s->body));
    } else if (tok_kw(t, KW_CASE)) {
        advance(p);
        s = stmt_new(S_CASE, pos);
        uint start = p->pos;
        TRY(cond_expr(p, &s->e));
        s->bare = p->pos == start + 1;
        /* clang's error is where the `:` should be, after the value. */
        if (!eat_punct(p, ":")) {
            return diag_error(after_prev(p), "expected ':' after 'case'");
        }
        s->id = 0;
        TRY(labeled(p, &s->body));
    } else if (tok_kw(t, KW_DEFAULT)) {
        advance(p);
        if (!eat_punct(p, ":")) {
            return diag_error(after_prev(p), "expected ':' after 'default'");
        }
        s = stmt_new(S_DEFAULT, pos);
        s->id = 0;
        TRY(labeled(p, &s->body));
    } else if (tok_kw(t, KW_WHILE)) {
        advance(p);
        s = stmt_new(S_WHILE, pos);
        TRY(paren_expr(p, "while", &s->e));
        TRY(stmt(p, &s->body));
    } else if (tok_kw(t, KW_DO)) {
        advance(p);
        s = stmt_new(S_DOWHILE, pos);
        TRY(stmt(p, &s->body));
        if (!eat_kw(p, KW_WHILE)) {
            return diag_note(expected(p, "'while' in do/while loop"), pos, "to match this 'do'");
        }
        TRY(paren_expr(p, "do/while", &s->e));
        TRY(expect_semi(p, "after do/while statement"));
    } else if (tok_kw(t, KW_FOR)) {
        advance(p);
        s = stmt_new(S_FOR, pos);
        push_scope(p, scope_new());
        struct diag *e = for_rest(p, s);
        pop_scope(p);
        if (e != NULL) {
            return e;
        }
    } else if (tok_kw(t, KW_RETURN)) {
        advance(p);
        s = stmt_new(S_RETURN, pos);
        if (!eat_punct(p, ";")) {
            uint named = p->undeclared.n;
            TRY(expr(p, &s->e));
            TRY(semi_or_skip(p, "after return statement", named));
        }
    } else if (tok_kw(t, KW_BREAK)) {
        advance(p);
        TRY(expect_semi(p, "after break statement"));
        s = stmt_new(S_BREAK, pos);
    } else if (tok_kw(t, KW_CONTINUE)) {
        advance(p);
        TRY(expect_semi(p, "after continue statement"));
        s = stmt_new(S_CONTINUE, pos);
    } else if (tok_kw(t, KW_GOTO)) {
        advance(p);
        s = stmt_new(S_GOTO, pos);
        TRY(expect_ident(p, &s->name, &s->at));
        TRY(expect_semi(p, "after goto statement"));
    } else if (t->kind == TK_IDENT && lookup_name(p, t->name) == NULL && peek_at(p, 1)->kind == TK_IDENT) {
        /* `x y` of a name nothing declares: what clang's typo correction of
         * a statement's first name takes it for. */
        return unknown_statement_name(p, t->name);
    } else {
        uint named = p->undeclared.n;
        struct expr *e;
        TRY(expr(p, &e));
        TRY(semi_or_skip(p, "after expression", named));
        s = stmt_new(S_EXPR, pos);
        s->e = e;
    }
    /* Only an if, and a statement that ends with one (a loop, a switch, a
     * label), ends with its else. */
    switch (s->kind) {
    case S_WHILE:
    case S_FOR:
    case S_SWITCH:
    case S_LABEL:
    case S_CASE:
    case S_DEFAULT:
        break;
    default:
        p->has_trailing_else = 0;
        break;
    }
    *out = s;
    return NULL;
}

static struct diag *stmt(struct parser *p, struct stmt **out) {
    TRY(parse_enter(p));
    struct diag *e = stmt_inner(p, out);
    p->depth--;
    return e;
}

/* ---- Expressions ---- */

static struct expr *binary_new(int op, struct expr *l, struct expr *r, struct pos pos) {
    struct expr *e = expr_new(E_BINARY, pos);
    e->op = (uchar)op;
    e->x = l;
    e->y = r;
    return e;
}

/* `a, b, c`: the comma operator. */
static struct diag *expr(struct parser *p, struct expr **out) {
    struct expr *e;
    TRY(assign_expr(p, &e));
    uint outer = p->depth;
    while (is_punct(p, ",")) {
        TRY(parse_enter(p));
        struct pos pos = advance(p)->pos;
        struct expr *rhs;
        TRY(assign_expr(p, &rhs));
        struct expr *c = expr_new(E_COMMA, pos);
        c->x = e;
        c->y = rhs;
        e = c;
    }
    p->depth = outer;
    *out = e;
    return NULL;
}

/* `=` (OP_NONE) or a compound assignment's operator; -1 for none. */
static int assign_op(struct token *t) {
    static const char *const ops[] = {"=", "+=", "-=", "*=", "/=", "%=", "<<=", ">>=", "&=", "|=", "^="};
    static const int bin[] = {OP_NONE, OP_ADD, OP_SUB, OP_MUL, OP_DIV, OP_REM, OP_SHL, OP_SHR, OP_BITAND, OP_BITOR, OP_BITXOR};
    if (t->kind != TK_PUNCT) {
        return -1;
    }
    for (uint i = 0; i < sizeof ops / sizeof ops[0]; i++) {
        if (strcmp(t->name, ops[i]) == 0) {
            return bin[i];
        }
    }
    return -1;
}

static struct diag *assign_inner(struct parser *p, struct expr **out) {
    struct expr *lhs;
    TRY(cond_expr(p, &lhs));
    int op = assign_op(tok(p));
    if (op >= 0) {
        struct pos pos = advance(p)->pos;
        struct expr *rhs;
        TRY(assign_expr(p, &rhs));
        struct expr *e = expr_new(E_ASSIGN, pos);
        e->op = (uchar)op;
        e->x = lhs;
        e->y = rhs;
        *out = e;
        return NULL;
    }
    *out = lhs;
    return NULL;
}

static struct diag *assign_expr(struct parser *p, struct expr **out) {
    TRY(parse_enter(p));
    struct diag *e = assign_inner(p, out);
    p->depth--;
    return e;
}

static struct diag *binary(struct parser *p, int min, struct expr **out);

static struct diag *cond_rest(struct parser *p, struct expr *c, struct expr **out) {
    struct pos pos = advance(p)->pos;
    struct expr *a;
    TRY(expr(p, &a));
    if (!eat_punct(p, ":")) {
        return diag_note(expected(p, "':'"), pos, "to match this '?'");
    }
    struct expr *b;
    TRY(cond_expr(p, &b));
    struct expr *e = expr_new(E_COND, pos);
    e->x = c;
    e->y = a;
    e->z = b;
    *out = e;
    return NULL;
}

/* `c ? a : b`; the middle operand is a full expression, as in C. */
static struct diag *cond_expr(struct parser *p, struct expr **out) {
    struct expr *c;
    TRY(binary(p, 1, &c));
    if (!is_punct(p, "?")) {
        *out = c;
        return NULL;
    }
    TRY(parse_enter(p));
    struct diag *e = cond_rest(p, c, out);
    p->depth--;
    return e;
}

/* A binary operator and its precedence; 0 for none. */
static int binop(struct token *t, int *op) {
    static const char *const ops[] = {"||", "&&", "|",  "^",  "&", "==", "!=", "<",  "<=",
                                      ">",  ">=", "<<", ">>", "+", "-",  "*",  "/",  "%"};
    static const int bin[] = {OP_LOGOR, OP_LOGAND, OP_BITOR, OP_BITXOR, OP_BITAND, OP_EQ,  OP_NE,  OP_LT,  OP_LE,
                              OP_GT,    OP_GE,     OP_SHL,   OP_SHR,    OP_ADD,    OP_SUB, OP_MUL, OP_DIV, OP_REM};
    static const int prec[] = {1, 2, 3, 4, 5, 6, 6, 7, 7, 7, 7, 8, 8, 9, 9, 10, 10, 10};
    if (t->kind != TK_PUNCT) {
        return 0;
    }
    for (uint i = 0; i < sizeof ops / sizeof ops[0]; i++) {
        if (strcmp(t->name, ops[i]) == 0) {
            *op = bin[i];
            return prec[i];
        }
    }
    return 0;
}

static struct diag *unary(struct parser *p, struct expr **out);

/* Precedence climbing: operators binding at least as tightly as min. */
static struct diag *binary(struct parser *p, int min, struct expr **out) {
    struct expr *lhs;
    TRY(unary(p, &lhs));
    uint outer = p->depth;
    for (;;) {
        int op;
        int prec = binop(tok(p), &op);
        if (prec == 0 || prec < min) {
            break;
        }
        /* Each operator puts everything parsed so far one level deeper. */
        TRY(parse_enter(p));
        struct pos pos = advance(p)->pos;
        struct expr *rhs;
        TRY(binary(p, prec + 1, &rhs));
        lhs = binary_new(op, lhs, rhs, pos);
    }
    p->depth = outer;
    *out = lhs;
    return NULL;
}

/* unary, one level deeper. */
static struct diag *nested_unary(struct parser *p, struct expr **out) {
    TRY(parse_enter(p));
    struct diag *e = unary(p, out);
    p->depth--;
    return e;
}

static struct diag *postfix_ops(struct parser *p, struct expr *e, struct expr **out);
static struct diag *primary(struct parser *p, struct expr **out);

/* `(type){ ... }`, after the `)`. */
static struct diag *compound_literal(struct parser *p, struct type *ty, struct pos pos, struct expr **out) {
    struct init *init;
    TRY(initializer(p, &init));
    struct expr *e = expr_new(E_COMPOUND, pos);
    e->init = init;
    e->ty = ty;
    *out = e;
    return NULL;
}

static struct diag *unary(struct parser *p, struct expr **out) {
    struct pos pos = here(p);
    struct token *t = tok(p);
    struct expr *e;
    if (tok_punct(t, "++") || tok_punct(t, "--")) {
        int inc = tok_punct(t, "++");
        advance(p);
        struct expr *target;
        TRY(nested_unary(p, &target));
        e = expr_new(E_INCDEC, pos);
        e->flag = (uchar)inc;
        e->prefix = 1;
        e->x = target;
    } else if (tok_punct(t, "-") || tok_punct(t, "+") || tok_punct(t, "!") || tok_punct(t, "~")) {
        int op = tok_punct(t, "-") ? UOP_NEG : tok_punct(t, "+") ? UOP_PLUS : tok_punct(t, "!") ? UOP_NOT : UOP_BITNOT;
        advance(p);
        struct expr *x;
        TRY(nested_unary(p, &x));
        e = expr_new(E_UNARY, pos);
        e->op = (uchar)op;
        e->x = x;
    } else if (tok_punct(t, "&") || tok_punct(t, "*")) {
        int kind = tok_punct(t, "&") ? E_ADDR : E_DEREF;
        advance(p);
        struct expr *x;
        TRY(nested_unary(p, &x));
        e = expr_new(kind, pos);
        e->x = x;
    } else if (tok_punct(t, "(") && type_name_follows(p)) {
        advance(p);
        struct decl d;
        struct spelling *spelled;
        TRY(type_name_decl(p, pos, 1, &d, &spelled));
        TRY(expect_close(p, ")", pos, NULL));
        int literal = is_punct(p, "{");
        type_name_folded(p, &d, literal, pos);
        if (literal) {
            struct expr *lit;
            TRY(compound_literal(p, d.ty, pos, &lit));
            return postfix_ops(p, lit, out);
        }
        struct expr *x;
        TRY(nested_unary(p, &x));
        e = expr_new(E_CAST, pos);
        e->x = x;
        e->spelled = spelled;
        /* A cast's result is a value: qualifiers mean nothing on it. */
        e->ty = ty_unqual(d.ty);
        *out = e;
        return NULL;
    } else if (tok_kw(t, KW_SIZEOF)) {
        advance(p);
        if (is_punct(p, "(") && type_name_follows(p)) {
            struct pos at = advance(p)->pos;
            struct decl d;
            struct spelling *spelled;
            TRY(type_name_decl(p, at, 1, &d, &spelled));
            TRY(expect_close(p, ")", at, NULL));
            int literal = is_punct(p, "{");
            type_name_folded(p, &d, literal, at);
            if (literal) {
                struct expr *lit;
                TRY(compound_literal(p, d.ty, at, &lit));
                struct expr *x;
                TRY(postfix_ops(p, lit, &x));
                e = expr_new(E_SIZEOF_EXPR, pos);
                e->x = x;
                e->at = at;
            } else {
                e = expr_new(E_SIZEOF_TYPE, pos);
                e->of = d.ty;
            }
        } else if (type_name_here(p)) {
            /* Where the `(` belongs, after sizeof. */
            return diag_error(after_prev(p), "expected parentheses around type name in sizeof expression");
        } else {
            struct pos at = here(p);
            struct expr *x;
            TRY(nested_unary(p, &x));
            e = expr_new(E_SIZEOF_EXPR, pos);
            e->x = x;
            e->at = at;
        }
    } else {
        struct expr *prim;
        TRY(primary(p, &prim));
        return postfix_ops(p, prim, out);
    }
    *out = e;
    return NULL;
}

/* The postfix operators after e: [i], calls, `.`, `->`, ++, --. */
static struct diag *postfix_ops(struct parser *p, struct expr *e, struct expr **out) {
    uint outer = p->depth;
    for (;;) {
        struct token *t = tok(p);
        if (tok_punct(t, "[")) {
            TRY(parse_enter(p));
            struct pos pos = advance(p)->pos;
            struct expr *index;
            TRY(expr(p, &index));
            struct pos close = here(p);
            TRY(expect_close(p, "]", pos, NULL));
            posmap_put(&p->written.brackets, pos, pos_box(close));
            /* a[i] is *(a + i). */
            struct expr *sum = binary_new(OP_ADD, e, index, pos);
            e = expr_unop(E_DEREF, sum, pos);
        } else if (tok_punct(t, "(")) {
            TRY(parse_enter(p));
            struct pos open = advance(p)->pos;
            struct expr **args = NULL;
            uint nargs = 0;
            uint cap = 0;
            if (!is_punct(p, ")")) {
                for (;;) {
                    if (nargs == MAX_PARAMS) {
                        return diag_error(here(p), fmt("too many arguments: merlin supports at most %u", MAX_PARAMS));
                    }
                    struct expr *a;
                    TRY(assign_expr(p, &a));
                    args = grow(args, &cap, nargs + 1, sizeof(struct expr *));
                    args[nargs++] = a;
                    if (is_punct(p, ")")) {
                        break;
                    }
                    if (!eat_punct(p, ",")) {
                        return expect_close(p, ")", open, NULL);
                    }
                }
            }
            struct pos close = advance(p)->pos;
            /* A call is positioned at its callee, as clang reports it. */
            struct expr *c = expr_new(E_CALL, e->pos);
            c->x = e;
            c->args = arena_alloc(nargs * sizeof(struct expr *) + 1);
            if (nargs > 0) {
                memcpy(c->args, args, nargs * sizeof(struct expr *));
            }
            heap_free(args);
            c->nargs = nargs;
            c->at = open;
            c->at2 = close;
            e = c;
        } else if (tok_punct(t, ".") || tok_punct(t, "->")) {
            int arrow = tok_punct(t, "->");
            TRY(parse_enter(p));
            struct pos pos = advance(p)->pos;
            const char *name;
            struct pos at;
            TRY(expect_ident(p, &name, &at));
            struct expr *m = expr_new(E_MEMBER, pos);
            m->x = e;
            m->name = name;
            m->flag = (uchar)arrow;
            m->at = at;
            e = m;
        } else if (tok_punct(t, "++") || tok_punct(t, "--")) {
            int inc = tok_punct(t, "++");
            TRY(parse_enter(p));
            struct pos pos = advance(p)->pos;
            struct expr *n = expr_new(E_INCDEC, pos);
            n->flag = (uchar)inc;
            n->x = e;
            e = n;
        } else {
            break;
        }
    }
    p->depth = outer;
    *out = e;
    return NULL;
}

/* __builtin_va_start(ap, last), __builtin_va_arg(ap, type),
 * __builtin_va_end(ap) and __builtin_va_copy(dst, src): clang's builtins
 * behind <stdarg.h>'s macros. They look like calls, but va_arg takes a
 * type, so the parser knows them by name. */
static struct diag *va_builtin(struct parser *p, const char *name, struct expr **out) {
    struct pos pos = advance(p)->pos;
    struct pos open;
    TRY(expect_punct(p, "(", &open));
    struct expr *first;
    TRY(assign_expr(p, &first));
    struct expr *e;
    if (strcmp(name, "__builtin_va_start") == 0 || strcmp(name, "__builtin_va_copy") == 0) {
        if (is_punct(p, ")")) {
            return diag_error(here(p), "too few arguments to function call, expected 2, have 1");
        }
        if (!eat_punct(p, ",")) {
            return expect_close(p, ")", open, NULL);
        }
        struct expr *second;
        TRY(assign_expr(p, &second));
        e = expr_new(strcmp(name, "__builtin_va_start") == 0 ? E_VA_START : E_VA_COPY, pos);
        e->x = first;
        e->y = second;
    } else if (strcmp(name, "__builtin_va_arg") == 0) {
        TRY(expect_punct(p, ",", NULL));
        struct pos at = here(p);
        struct type *ty;
        /* clang reads the type and its `)` itself, not as a bracket matched
         * with its `(` (ParseExpr.cpp): no note, and a `;` before the `)`
         * is not called out. */
        TRY(type_name(p, open, &ty));
        if (!eat_punct(p, ")")) {
            return expected(p, "')'");
        }
        e = expr_new(E_VA_ARG, pos);
        e->x = first;
        e->at = at;
        /* The value is a copy: qualifiers mean nothing on it. */
        e->ty = ty_unqual(ty);
        *out = e;
        return NULL;
    } else if (strcmp(name, "__builtin_va_end") == 0) {
        e = expr_unop(E_VA_END, first, pos);
    } else {
        return diag_error(pos, fmt("unknown builtin '%s'", name));
    }
    TRY(expect_close(p, ")", open, NULL));
    *out = e;
    return NULL;
}

static int starts_with(const char *s, const char *prefix) {
    return strncmp(s, prefix, strlen(prefix)) == 0;
}

static int is_alnum(uint c) {
    return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z');
}

static struct diag *primary(struct parser *p, struct expr **out) {
    struct pos pos = here(p);
    struct token *t = tok(p);
    if (t->kind == TK_NUM) {
        advance(p);
        uint len = (uint)strlen(t->text);
        if (len > 1 && t->text[0] == '0' && is_alnum((uchar)t->text[1])) {
            posmap_put(&p->written.bit_patterns, pos, NULL);
        }
        if (len > 0 && t->text[len - 1] == '\'') {
            posmap_put(&p->written.char_literals, pos, NULL);
        } else {
            posmap_put(&p->written.int_literals, pos, NULL);
        }
        *out = expr_num(t->value, ty_int(t->ik), pos);
        return NULL;
    }
    if (t->kind == TK_STR) {
        /* Adjacent string literals are one: "ab" "c" is "abc". */
        struct buf b;
        memset(&b, 0, sizeof b);
        while (tok(p)->kind == TK_STR) {
            buf_put(&b, tok(p)->bytes, tok(p)->len);
            advance(p);
        }
        struct expr *e = expr_new(E_STR, pos);
        e->bytes = arena_alloc(b.len + 1);
        if (b.len > 0) {
            memcpy(e->bytes, b.data, b.len);
        }
        e->len = b.len;
        e->ty = ty_array(ty_int(IK_CHAR), 1, b.len + 1);
        buf_free(&b);
        *out = e;
        return NULL;
    }
    if (t->kind == TK_IDENT && starts_with(t->name, "__builtin_va_") && tok_punct(peek_at(p, 1), "(")) {
        return va_builtin(p, t->name, out);
    }
    if (t->kind == TK_IDENT) {
        const char *name = t->name;
        struct name *n = lookup_name(p, name);
        if (n != NULL && n->kind == NAME_ENUM_CONST) {
            /* An enumeration constant is an int, which remembers its enum
             * (none inside the enum's own definition). */
            struct expr *e;
            if (n->spelled != NULL) {
                e = expr_new(E_ENUMERATOR, pos);
                e->name = name;
                e->value = n->value;
                e->spelled = n->spelled;
            } else {
                e = expr_new(E_NUM, pos);
                e->value = n->value;
            }
            advance(p);
            *out = e;
            return NULL;
        }
        if (n != NULL && n->kind == NAME_TYPEDEF) {
            return diag_error(pos, fmt("unexpected type name '%s': expected expression", name));
        }
        if (n == NULL) {
            /* `(unsinged)1`, `sizeof(myit)`: in parentheses alone, where a
             * type's name could be, clang takes it for one if that is
             * closest. */
            int has_decl;
            struct pos decl;
            if (p->pos > 0 && tok_punct(&p->toks[p->pos - 1], "(") && tok_punct(peek_at(p, 1), ")")) {
                const char *to = correct_type(p, name, &has_decl, &decl);
                if (to != NULL) {
                    struct correction *c = correct_expr(p, name);
                    if (c == NULL || strcmp(c->name, to) != 0) {
                        struct diag *e =
                            diag_error(pos, fmt("use of undeclared identifier '%s'; did you mean '%s'?", name, to));
                        if (has_decl) {
                            diag_note(e, decl, fmt("'%s' declared here", to));
                        }
                        return e;
                    }
                }
            }
            struct correction *c = correct_expr(p, name);
            if (c != NULL) {
                posmap_put(&p->written.corrections, pos, c);
            }
            struct expr *var = expr_new(E_VAR, pos);
            var->name = name;
            struct expr *e = var;
            /* Called, it is declared implicitly: clang's error of that as it
             * reads the name, which a call of no arguments gives too. */
            if (tok_punct(peek_at(p, 1), "(")) {
                struct pos open = p->toks[p->pos + 1].pos;
                e = expr_new(E_CALL, pos);
                e->x = var;
                e->args = arena_alloc(sizeof(struct expr *));
                e->at = open;
                e->at2 = open;
            }
            vec_push(&p->undeclared, e);
        }
        advance(p);
        struct expr *e = expr_new(E_VAR, pos);
        e->name = name;
        *out = e;
        return NULL;
    }
    if (tok_punct(t, "(")) {
        /* Keep the inner node; parentheses only steer the tree's shape (and
         * what a failed _Static_assert prints). */
        advance(p);
        struct expr *e;
        TRY(expr(p, &e));
        TRY(expect_close(p, ")", pos, NULL));
        if (p->in_parens) {
            struct paren *k = arena_alloc(sizeof *k);
            k->pos = e->pos;
            k->kind = e->kind;
            k->open = pos;
            vec_push(&p->parens, k);
        }
        posmap_put(&p->written.parens, e->pos, pos_box(pos));
        *out = e;
        return NULL;
    }
    return expected(p, "expression");
}

/* ---- _Static_assert ---- */

/* Prints an expression as clang's StmtPrinter does, for what a failed
 * _Static_assert says (merlin's Printer): literals by their values (0x10 is
 * 16, an unsigned one 16U), operators between spaces, the parentheses as
 * written, types as clang spells them, and names as written. The tokens
 * are the expression's, which say how a literal was written. */
struct assert_printer {
    struct token *toks;
    uint ntoks;
    struct vec *parens;
};

static struct token *pr_token(struct assert_printer *pr, struct pos pos) {
    for (uint i = 0; i < pr->ntoks; i++) {
        if (pos_eq(pr->toks[i].pos, pos)) {
            return &pr->toks[i];
        }
    }
    return NULL;
}

/* How many parentheses are around e, and where the outermost opens. */
static uint pr_parens_of(struct assert_printer *pr, struct expr *e, struct pos *open) {
    uint n = 0;
    for (uint i = 0; i < pr->parens->n; i++) {
        struct paren *k = pr->parens->v[i];
        if (pos_eq(k->pos, e->pos) && k->kind == e->kind) {
            if (n == 0 || pos_cmp(k->open, *open) < 0) {
                *open = k->open;
            }
            n++;
        }
    }
    return n;
}

static int ends_with_quote(const char *s) {
    uint n = (uint)strlen(s);
    return n > 0 && s[n - 1] == '\'';
}

/* Whether e is an integer literal (not a character one). */
static int pr_is_integer_literal(struct assert_printer *pr, struct expr *e) {
    if (e->kind != E_NUM) {
        return 0;
    }
    struct token *t = pr_token(pr, e->pos);
    return t != NULL && t->kind == TK_NUM && !ends_with_quote(t->text);
}

/* The [] of a[i], which the parser made *(a + i) at the `[`. */
static int pr_subscript(struct expr *e, struct expr **a, struct expr **i) {
    if (e->kind == E_DEREF && pos_eq(e->x->pos, e->pos) && e->x->kind == E_BINARY && e->x->op == OP_ADD) {
        *a = e->x->x;
        *i = e->x->y;
        return 1;
    }
    return 0;
}

/* Where e starts: clang's place for what is said of it. Its own parentheses
 * count unless it is the top of what is printed. */
static struct pos pr_begin(struct assert_printer *pr, struct expr *e, int top) {
    struct pos open;
    if (pr_parens_of(pr, e, &open) > 0 && !top) {
        return open;
    }
    struct expr *a;
    struct expr *i;
    if (pr_subscript(e, &a, &i)) {
        return pr_begin(pr, a, 0);
    }
    switch (e->kind) {
    case E_BINARY:
    case E_COMMA:
    case E_COND:
        return pr_begin(pr, e->x, 0);
    case E_ASSIGN:
        return pr_begin(pr, e->x, 0);
    case E_INCDEC:
        return e->prefix ? e->pos : pr_begin(pr, e->x, 0);
    case E_MEMBER:
    case E_CALL:
        return pr_begin(pr, e->x, 0);
    default:
        return e->pos;
    }
}

/* A character c of a literal quoted by quote, as clang prints it: C's
 * escapes, the printable ones as they are, the rest in hexadecimal. */
static void escaped(struct buf *b, uint c, uint quote) {
    static const char hex[] = "0123456789abcdef";
    if (c == '\\') {
        buf_str(b, "\\\\");
    } else if (c == quote) {
        buf_byte(b, '\\');
        buf_byte(b, quote);
    } else if (c == 7) {
        buf_str(b, "\\a");
    } else if (c == 8) {
        buf_str(b, "\\b");
    } else if (c == 12) {
        buf_str(b, "\\f");
    } else if (c == '\n') {
        buf_str(b, "\\n");
    } else if (c == '\r') {
        buf_str(b, "\\r");
    } else if (c == '\t') {
        buf_str(b, "\\t");
    } else if (c == 11) {
        buf_str(b, "\\v");
    } else if (c >= ' ' && c <= '~') {
        buf_byte(b, c);
    } else {
        /* \x, \u or \U, in as many hex digits as each has (Rust's
         * char::from_u32 fails for surrogates, which fall here too). */
        uint digits = c < 0x100 ? 2 : c <= 0xffff ? 4 : 8;
        buf_str(b, c < 0x100 ? "\\x" : c <= 0xffff ? "\\u" : "\\U");
        for (uint k = digits; k > 0; k--) {
            buf_byte(b, (uchar)hex[(c >> ((k - 1) * 4)) & 15]);
        }
    }
}

/* A character literal of value v, written text, as clang prints it. */
static char *char_literal(i128 v, const char *text) {
    int wide = text[0] == 'L';
    uint c = (uint)v.lo;
    /* A plain one may be sign-extended. */
    if (!wide && (c & ~0xffu) == ~0xffu) {
        c &= 0xff;
    }
    struct buf b;
    memset(&b, 0, sizeof b);
    if (wide) {
        buf_byte(&b, 'L');
    }
    buf_byte(&b, '\'');
    escaped(&b, c, '\'');
    buf_byte(&b, '\'');
    char *r = buf_arena(&b);
    buf_free(&b);
    return r;
}

static char *pr_init(struct assert_printer *pr, struct init *init);

/* e as clang prints it, without its own parentheses if top. */
static char *pr_expr(struct assert_printer *pr, struct expr *e, int top) {
    struct expr *a;
    struct expr *i;
    char *text;
    if (pr_subscript(e, &a, &i)) {
        text = fmt("%s[%s]", pr_expr(pr, a, 0), pr_expr(pr, i, 0));
    } else {
        switch (e->kind) {
        case E_NUM: {
            struct token *t = pr_token(pr, e->pos);
            if (t != NULL && ends_with_quote(t->text)) {
                text = char_literal(e->value, t->text);
            } else if (t != NULL && t->kind == TK_IDENT) {
                /* An enumeration constant inside its enum. */
                text = (char *)t->text;
            } else {
                const char *suffix = "";
                switch (ty_int_kind(e->ty)) {
                case IK_UINT:
                    suffix = "U";
                    break;
                case IK_LONG:
                    suffix = "L";
                    break;
                case IK_ULONG:
                    suffix = "UL";
                    break;
                case IK_LLONG:
                    suffix = "LL";
                    break;
                case IK_ULLONG:
                    suffix = "ULL";
                    break;
                default:
                    break;
                }
                text = fmt("%s%s", i128_str(e->value), suffix);
            }
            break;
        }
        case E_ENUMERATOR:
        case E_VAR:
            text = (char *)e->name;
            break;
        case E_STR: {
            struct buf b;
            memset(&b, 0, sizeof b);
            buf_byte(&b, '"');
            for (uint k = 0; k < e->len; k++) {
                escaped(&b, e->bytes[k], '"');
            }
            buf_byte(&b, '"');
            text = buf_arena(&b);
            buf_free(&b);
            break;
        }
        case E_BINARY:
            text = fmt("%s %s %s", pr_expr(pr, e->x, 0), binop_symbol(e->op), pr_expr(pr, e->y, 0));
            break;
        case E_COMMA:
            text = fmt("%s , %s", pr_expr(pr, e->x, 0), pr_expr(pr, e->y, 0));
            break;
        case E_UNARY:
            text = fmt("%s%s", unop_symbol(e->op), pr_expr(pr, e->x, 0));
            break;
        case E_ADDR:
            text = fmt("&%s", pr_expr(pr, e->x, 0));
            break;
        case E_DEREF:
            text = fmt("*%s", pr_expr(pr, e->x, 0));
            break;
        case E_CAST:
            text = fmt("(%s)%s", ty_str(e->ty), pr_expr(pr, e->x, 0));
            break;
        case E_SIZEOF_TYPE:
            text = fmt("sizeof(%s)", ty_str(e->of));
            break;
        case E_SIZEOF_EXPR:
            text = fmt("sizeof %s", pr_expr(pr, e->x, 0));
            break;
        case E_COND:
            text = fmt("%s ? %s : %s", pr_expr(pr, e->x, 0), pr_expr(pr, e->y, 0), pr_expr(pr, e->z, 0));
            break;
        case E_ASSIGN:
            text = fmt("%s %s= %s", pr_expr(pr, e->x, 0), e->op == OP_NONE ? "" : binop_symbol(e->op),
                       pr_expr(pr, e->y, 0));
            break;
        case E_INCDEC: {
            const char *op = e->flag ? "++" : "--";
            text = e->prefix ? fmt("%s%s", op, pr_expr(pr, e->x, 0)) : fmt("%s%s", pr_expr(pr, e->x, 0), op);
            break;
        }
        case E_MEMBER:
            text = fmt("%s%s%s", pr_expr(pr, e->x, 0), e->flag ? "->" : ".", e->name);
            break;
        case E_CALL: {
            struct buf b;
            memset(&b, 0, sizeof b);
            buf_str(&b, pr_expr(pr, e->x, 0));
            buf_byte(&b, '(');
            for (uint k = 0; k < e->nargs; k++) {
                if (k > 0) {
                    buf_str(&b, ", ");
                }
                buf_str(&b, pr_expr(pr, e->args[k], 0));
            }
            buf_byte(&b, ')');
            text = buf_arena(&b);
            buf_free(&b);
            break;
        }
        case E_COMPOUND:
            text = fmt("(%s)%s", ty_str(e->ty), pr_init(pr, e->init));
            break;
        case E_VA_START:
            text = fmt("__builtin_va_start(%s, %s)", pr_expr(pr, e->x, 0), pr_expr(pr, e->y, 0));
            break;
        case E_VA_ARG:
            text = fmt("__builtin_va_arg(%s, %s)", pr_expr(pr, e->x, 0), ty_str(e->ty));
            break;
        case E_VA_END:
            text = fmt("__builtin_va_end(%s)", pr_expr(pr, e->x, 0));
            break;
        default:
            text = fmt("__builtin_va_copy(%s, %s)", pr_expr(pr, e->x, 0), pr_expr(pr, e->y, 0));
            break;
        }
    }
    struct pos open;
    uint n = top ? 0 : pr_parens_of(pr, e, &open);
    struct buf b;
    memset(&b, 0, sizeof b);
    for (uint k = 0; k < n; k++) {
        buf_byte(&b, '(');
    }
    buf_str(&b, text);
    for (uint k = 0; k < n; k++) {
        buf_byte(&b, ')');
    }
    char *r = buf_arena(&b);
    buf_free(&b);
    return r;
}

static char *pr_init(struct assert_printer *pr, struct init *init) {
    if (init->kind == I_EXPR) {
        return pr_expr(pr, init->e, 0);
    }
    struct buf b;
    memset(&b, 0, sizeof b);
    if (init->kind == I_LIST) {
        buf_byte(&b, '{');
        for (uint k = 0; k < init->items.n; k++) {
            if (k > 0) {
                buf_str(&b, ", ");
            }
            buf_str(&b, pr_init(pr, init->items.v[k]));
        }
        buf_byte(&b, '}');
    } else {
        for (uint k = 0; k < init->nds; k++) {
            struct designator *d = init->ds[k];
            if (d->member) {
                buf_fmt(&b, ".%s", d->name);
            } else {
                buf_fmt(&b, "[%s]", pr_expr(pr, d->index, 0));
            }
        }
        buf_fmt(&b, " = %s", pr_init(pr, init->inner));
    }
    char *r = buf_arena(&b);
    buf_free(&b);
    return r;
}

/* The terms of a condition's &&s, in order. */
static void terms(struct expr *e, struct vec *out) {
    if (e->kind == E_BINARY && e->op == OP_LOGAND) {
        terms(e->x, out);
        terms(e->y, out);
    } else {
        vec_push(out, e);
    }
}

/* What a _Static_assert whose condition is 0 failed on, as clang finds it:
 * the first term of the condition's &&s that is 0 and no integer literal,
 * else the whole condition, or none (0) if that is an integer literal
 * itself. Each term as written (original) is evaluated as the parser
 * resolved it (resolved, of the same shape). */
static int failed_requirement(struct assert_printer *pr, struct expr *original, struct expr *resolved, struct expr **term,
                              struct expr **value) {
    struct vec written;
    struct vec values;
    memset(&written, 0, sizeof written);
    memset(&values, 0, sizeof values);
    terms(original, &written);
    terms(resolved, &values);
    int found = 0;
    for (uint i = 0; i < written.n && i < values.n; i++) {
        i128 v;
        if (!pr_is_integer_literal(pr, written.v[i]) && consteval_parsed_int(values.v[i], &v) && i128_is_zero(v)) {
            *term = written.v[i];
            *value = values.v[i];
            found = 1;
            break;
        }
    }
    heap_free(written.v);
    heap_free(values.v);
    if (found) {
        return 1;
    }
    if (pr_is_integer_literal(pr, original)) {
        return 0;
    }
    *term = original;
    *value = resolved;
    return 1;
}

/* Literals, or what unary operators make of them: plain to see. */
static int plain(struct assert_printer *pr, struct expr *e) {
    if (e->kind == E_UNARY) {
        return plain(pr, e->x);
    }
    if (e->kind == E_NUM) {
        struct token *t = pr_token(pr, e->pos);
        return t != NULL && t->kind == TK_NUM;
    }
    return 0;
}

/* clang's note on a failed _Static_assert of a binary operator (not && or
 * ||): what its operands evaluate to, at the operator, unless both are
 * plain to see. */
static int evaluates_to(struct assert_printer *pr, struct expr *written, struct expr *value, struct pos *at, char **note) {
    if (written->kind != E_BINARY || value->kind != E_BINARY) {
        return 0;
    }
    if (written->op == OP_LOGOR || written->op == OP_LOGAND) {
        return 0;
    }
    if (plain(pr, written->x) && plain(pr, written->y)) {
        return 0;
    }
    i128 a;
    i128 b;
    if (!consteval_parsed_int(value->x, &a) || !consteval_parsed_int(value->y, &b)) {
        return 0;
    }
    *at = value->pos;
    *note = fmt("expression evaluates to '%s %s %s'", i128_str(a), binop_symbol(written->op), i128_str(b));
    return 1;
}

/* `_Static_assert(constant, "message");` (C11, which clang accepts in C99
 * too; without the message C23's, which clang takes with a warning), at
 * file or block scope (merlin's static_assert). The parser checks the
 * assertion here; what is left of it is the condition for sema to check. A
 * failed one is clang's error, given by sema after the condition's
 * diagnostics, unless it has an error. */
static struct diag *static_assertion(struct parser *p) {
    advance(p);
    struct pos open;
    TRY(expect_punct(p, "(", &open));
    struct pos at = here(p);
    uint first = p->pos;
    p->in_parens = 1;
    memset(&p->parens, 0, sizeof p->parens);
    struct expr *cond;
    struct diag *e = cond_expr(p, &cond);
    struct vec parens = vec_take(&p->parens);
    p->in_parens = 0;
    if (e != NULL) {
        return e;
    }
    uint last = p->pos;
    /* The message, msglen bytes (a string literal may hold a NUL). */
    const char *msg = NULL;
    uint msglen = 0;
    if (eat_punct(p, ",")) {
        if (tok(p)->kind != TK_STR) {
            return expected(p, "string literal for diagnostic message in static_assert");
        }
        struct buf text;
        memset(&text, 0, sizeof text);
        while (tok(p)->kind == TK_STR) {
            buf_put(&text, tok(p)->bytes, tok(p)->len);
            advance(p);
        }
        msg = from_utf8_lossy((uchar *)text.data, text.len, &msglen);
        buf_free(&text);
    } else if (is_punct(p, ")")) {
        warn(p, diag_warning(here(p), "'_Static_assert' with no message is a C23 extension", "c23-extensions"));
    } else {
        return expect_punct(p, ",", NULL);
    }
    TRY(expect_close(p, ")", open, NULL));
    TRY(expect_semi(p, "after '_Static_assert'"));
    struct expr *original = expr_clone(cond);
    uint mark = p->unevaluated.n;
    i128 v;
    int sure;
    const_int(p, cond, at, "static assertion expression is not an integral constant expression", NULL, &v, &sure);
    /* Not sure of the value: sema reports the error. */
    if (!sure || !i128_is_zero(v)) {
        return NULL;
    }
    struct assert_printer pr;
    pr.toks = p->toks + first;
    pr.ntoks = last - first;
    pr.parens = &parens;
    uint slen = 0;
    const char *suffix = msg != NULL ? fmtn(&slen, ": %b", msg, msglen) : "";
    struct diag *failed;
    uint len;
    struct expr *term;
    struct expr *value;
    if (failed_requirement(&pr, original, cond, &term, &value)) {
        char *what = pr_expr(&pr, term, 1);
        const char *m = fmtn(&len, "static assertion failed due to requirement '%s'%b", what, suffix, slen);
        failed = diag_error_n(pr_begin(&pr, term, 1), m, len);
        struct pos note_at;
        char *note;
        if (evaluates_to(&pr, term, value, &note_at, &note)) {
            diag_note(failed, note_at, note);
        }
    } else {
        const char *m = fmtn(&len, "static assertion failed%b", suffix, slen);
        failed = diag_error_n(at, m, len);
    }
    if (p->unevaluated.n == mark) {
        /* `_Static_assert(0, "")`: of a lone literal, nothing was kept to
         * check. */
        warn(p, failed);
        return NULL;
    }
    struct unevaluated *u = vec_last(&p->unevaluated);
    u->diagnostic = failed;
    u->with = p->unevaluated.n - mark - 1;
    return NULL;
}

/* ---- The translation unit ---- */

static void take_written(struct parser *p, struct program *prog) {
    prog->warnings = vec_take(&p->warnings);
    prog->written = p->written;
    memset(&p->written, 0, sizeof p->written);
}

/* Parses a translation unit (merlin's parse_salvaging): 1 and the program,
 * or 0 and the syntax error with what came before it (the items parsed,
 * and as much of the function whose body has the error as its blocks hold
 * before it), for sema to find in them the warnings clang gives before
 * such an error. */
int parse_program(struct token *toks, uint ntoks, struct program *prog, struct diag **error) {
    struct parser *p = heap_calloc(1, sizeof(struct parser));
    p->toks = toks;
    p->ntoks = ntoks;
    /* The type clang's <stdarg.h> builds va_list from: on RISC-V a plain
     * pointer into the argument words. A typedef of no position. */
    struct scope *file = scope_new();
    struct typedef_name *va = arena_alloc(sizeof *va);
    va->name = "__builtin_va_list";
    va->ty = ty_ptr(TY_VOID_T);
    struct name *va_name = name_new(NAME_TYPEDEF, va->ty, pos_at(0, 0, 0));
    va_name->spelled = spelling_typedef(va);
    map_put(&file->names, va->name, va_name);
    push_scope(p, file);
    memset(prog, 0, sizeof *prog);
    while (!is_eof(p)) {
        uint mark = p->undeclared.n;
        uint64_t start = expr_next_id();
        struct vec items;
        struct diag *e = item(p, &items);
        if (e == NULL) {
            vec_extend(&prog->items, &items);
        } else if (p->partial != NULL) {
            vec_push(&prog->items, p->partial);
            p->partial = NULL;
        }
        uint n = prog->items.n;
        for (uint i = 0; i < p->unevaluated.n; i++) {
            struct file_unevaluated *fu = arena_alloc(sizeof *fu);
            fu->after = n;
            fu->u = p->unevaluated.v[i];
            vec_push(&prog->unevaluated, fu);
        }
        p->unevaluated.n = 0;
        if (e != NULL) {
            /* A declaration cut short: its undeclared names (a function's
             * are its blocks'). */
            struct stmt *s = cut_names(p, mark, start);
            if (s != NULL) {
                for (uint i = 0; i < s->list.n; i++) {
                    struct file_unevaluated *fu = arena_alloc(sizeof *fu);
                    fu->after = n;
                    fu->u = s->list.v[i];
                    vec_push(&prog->unevaluated, fu);
                }
            }
            take_written(p, prog);
            *error = e;
            return 0;
        }
    }
    take_written(p, prog);
    return 1;
}
