/* cc1: the statements a constant condition leaves out, as clang's code
 * generator leaves them out at -O0 (follows merlin's elide.rs; clang's
 * lib/CodeGen/CGStmt.cpp).
 *
 * clang decides a condition at compile time when it is an integer constant
 * expression (ConstantFoldsToSimpleInteger). Then `if (0) a; else b;`
 * emits only b, and `switch (2) { case 1: a(); case 2: b(); break; }` only
 * b();. Code that cannot be reached by falling into it is still emitted if
 * a jump from elsewhere can reach it: a statement with a goto label, or a
 * case or default of a switch around it, is kept (ContainsLabel). These
 * functions answer the questions clang asks of the statements and pick the
 * statements a switch on a constant runs; code generation does the rest.
 *
 * merlin's tree has no statement for a declaration that only declares types
 * (the parser leaves it out), so it counts for nothing here, where clang's
 * DeclStmt would. */
#include "cc.h"

/* The statements directly inside s, in *out (up to the block's), how many. */
static uint children(struct stmt *s, struct stmt ***out, struct stmt **two) {
    switch (s->kind) {
    case S_BLOCK:
        *out = (struct stmt **)s->list.v;
        return s->list.n;
    case S_IF:
        two[0] = s->body;
        two[1] = s->els;
        *out = two;
        return s->els != NULL ? 2 : 1;
    case S_WHILE:
    case S_DOWHILE:
    case S_SWITCH:
    case S_CASE:
    case S_DEFAULT:
    case S_LABEL:
        two[0] = s->body;
        *out = two;
        return 1;
    case S_FOR:
        if (s->init != NULL) {
            two[0] = s->init;
            two[1] = s->body;
            *out = two;
            return 2;
        }
        two[0] = s->body;
        *out = two;
        return 1;
    default:
        *out = two;
        return 0;
    }
}

/* Whether s has a label a jump from outside it can reach: a goto label, or,
 * unless ignore_cases, a case or default (those of a switch inside s are
 * that switch's). clang's ContainsLabel. */
int contains_label(struct stmt *s, int ignore_cases) {
    if (s->kind == S_LABEL) {
        return 1;
    }
    if ((s->kind == S_CASE || s->kind == S_DEFAULT) && !ignore_cases) {
        return 1;
    }
    if (s->kind == S_SWITCH) {
        return contains_label(s->body, 1);
    }
    struct stmt *two[2];
    struct stmt **k;
    uint n = children(s, &k, two);
    for (uint i = 0; i < n; i++) {
        if (contains_label(k[i], ignore_cases)) {
            return 1;
        }
    }
    return 0;
}

/* Whether s has a break that leaves the statement around it: one not
 * inside a loop or switch of its own. clang's containsBreak. */
int contains_break(struct stmt *s) {
    switch (s->kind) {
    case S_SWITCH:
    case S_WHILE:
    case S_DOWHILE:
    case S_FOR:
        return 0;
    case S_BREAK:
        return 1;
    default:
        break;
    }
    struct stmt *two[2];
    struct stmt **k;
    uint n = children(s, &k, two);
    for (uint i = 0; i < n; i++) {
        if (contains_break(k[i])) {
            return 1;
        }
    }
    return 0;
}

/* Whether s may declare a variable in the scope it is in: a declaration, or
 * a labelled one. clang's mightAddDeclToScope. */
int might_add_decl(struct stmt *s) {
    switch (s->kind) {
    case S_IF:
    case S_SWITCH:
    case S_WHILE:
    case S_DOWHILE:
    case S_FOR:
    case S_BLOCK:
        return 0;
    case S_DECL:
        return 1;
    default:
        break;
    }
    struct stmt *two[2];
    struct stmt **k;
    uint n = children(s, &k, two);
    for (uint i = 0; i < n; i++) {
        if (might_add_decl(k[i])) {
            return 1;
        }
    }
    return 0;
}

/* What collect found in a statement (clang's CSFC_Result): the statements
 * cannot be picked out; the statement is skipped or ends with the break;
 * it runs and falls through to the next. */
enum { FAILURE, SUCCESS, FALL_THROUGH };

static int collect(struct stmt *s, int has_case, uint id, int *found, struct vec *out);

/* Whether the statements from i on may be skipped: none has a label. */
static int skippable(struct vec *body, uint i) {
    for (; i < body->n; i++) {
        if (contains_label(body->v[i], 1)) {
            return FAILURE;
        }
    }
    return SUCCESS;
}

/* collect of a block: the statements before the case are skipped (they must
 * have no label, and no declaration the rest could use), those from it on
 * run until the break, and those after it are skipped. */
static int collect_block(struct stmt *s, struct vec *body, int has_case, uint id, int *found, struct vec *out) {
    int started_live = *found;
    uint start = out->n;
    uint i = 0;
    if (has_case) {
        int skipped_decl = 0;
        while (has_case && i < body->n) {
            skipped_decl |= might_add_decl(body->v[i]);
            int r = collect(body->v[i], has_case, id, found, out);
            if (r == FAILURE) {
                return FAILURE;
            }
            if (r == SUCCESS && *found) {
                /* The case and its break both in that statement: the rest is
                 * skipped. */
                return skipped_decl ? FAILURE : skippable(body, i + 1);
            }
            if (r == FALL_THROUGH) {
                /* The case found: the statements after it run. */
                has_case = 0;
                if (skipped_decl) {
                    return FAILURE;
                }
            }
            i++;
        }
        if (!*found) {
            return SUCCESS;
        }
    }
    int any_decls = 0;
    while (i < body->n) {
        any_decls |= might_add_decl(body->v[i]);
        int r = collect(body->v[i], 0, 0, found, out);
        if (r == FAILURE) {
            return FAILURE;
        }
        if (r == SUCCESS) {
            return skippable(body, i + 1);
        }
        i++;
    }
    /* Falling out of the block: its declarations' scope must stay, so the
     * block runs whole, if all of it runs and no break is in it. */
    if (any_decls) {
        if (started_live && !contains_break(s)) {
            vec_truncate(out, start);
            vec_push(out, s);
        } else {
            return FAILURE;
        }
    }
    return FALL_THROUGH;
}

/* clang's CollectStatementsForCase: looking for the case with id `id`
 * (has_case 0 once found, in the statements that run), adds to out what
 * runs of s. */
static int collect(struct stmt *s, int has_case, uint id, int *found, struct vec *out) {
    /* The case looked for: what it labels runs. Any other is skipped over,
     * its statement looked into. */
    if (s->kind == S_CASE || s->kind == S_DEFAULT) {
        if (has_case && (uint)s->id == id) {
            *found = 1;
            return collect(s->body, 0, 0, found, out);
        }
        return collect(s->body, has_case, id, found, out);
    }
    /* The break that ends what runs. */
    if (s->kind == S_BREAK && !has_case) {
        return SUCCESS;
    }
    if (s->kind == S_BLOCK) {
        return collect_block(s, &s->list, has_case, id, found, out);
    }
    /* Any other statement: skipped if it has no label, or run whole if it
     * has no break of the switch. */
    if (has_case) {
        return contains_label(s, 1) ? FAILURE : SUCCESS;
    }
    if (contains_break(s)) {
        return FAILURE;
    }
    vec_push(out, s);
    return FALL_THROUGH;
}

/* The statements a switch with the constant condition `value` runs, in
 * order, when they can be picked out of its body (clang's
 * FindCaseStatementsForValue): 1 and them in *out (none when no case
 * matches and there is no default), or 0 when they cannot be emitted
 * alone, and the whole switch is generated as usual. */
int case_statements(struct stmt *sw, i128 value, struct vec *out) {
    memset(out, 0, sizeof *out);
    int has = 0;
    uint id = 0;
    for (uint i = 0; i < sw->ncases; i++) {
        if (i128_eq(sw->cases[i].value, value)) {
            has = 1;
            id = sw->cases[i].id;
            break;
        }
    }
    if (!has && sw->id >= 0) {
        has = 1;
        id = (uint)sw->id;
    }
    if (!has) {
        /* Nothing runs: the body can go if nothing can jump into it. */
        return !contains_label(sw->body, 1);
    }
    int found = 0;
    if (collect(sw->body, 1, id, &found, out) == FAILURE) {
        return 0;
    }
    return found;
}
