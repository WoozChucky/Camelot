/* ld: libraries, objects linked only when a program needs them, as ld links
 * an archive's members and ld.lld the objects between --start-lib and
 * --end-lib (merlin-ld's library.rs). A member is linked only if it defines
 * a global that is still undefined; the search goes round the members again
 * until a pass pulls in nothing more. A weak undefined reference pulls
 * nothing in. The members pulled in follow the program's objects in
 * library order, whatever order they were found in. */
#include "ld.h"

/* What a name is in the link so far: defined, undefined, or both bits clear. */
#define DEFINED 1
#define UNDEFINED 2

static int defines(struct symbol *s) {
    return !sym_is_local(s) && !sym_is_undefined(s) && s->name[0] != 0;
}

static int needs(struct symbol *s) {
    return !sym_is_local(s) && sym_is_undefined(s) && !sym_is_weak(s) && s->name[0] != 0;
}

static int state(struct strmap *m, char *name) {
    int v = 0;
    map_get(m, name, &v);
    return v;
}

/* Marks what `o` defines (no longer undefined). */
static void add_defines(struct strmap *names, struct object *o) {
    for (uint k = 1; k < o->nsymbols; k++) {
        struct symbol *s = &o->symbols[k];
        if (defines(s)) {
            map_put(names, s->name, (state(names, s->name) | DEFINED) & ~UNDEFINED);
        }
    }
}

/* Marks what `o` needs that nothing defines yet. */
static void add_needs(struct strmap *names, struct object *o) {
    for (uint k = 1; k < o->nsymbols; k++) {
        struct symbol *s = &o->symbols[k];
        if (needs(s) && !(state(names, s->name) & DEFINED)) {
            map_put(names, s->name, state(names, s->name) | UNDEFINED);
        }
    }
}

struct object *parse_with_needed(struct input *inputs, int *lazy, uint n, uint *count) {
    struct object *objects = xcalloc((n ? n : 1) * sizeof(struct object));
    for (uint i = 0; i < n; i++) {
        char *err;
        if (!object_parse(inputs[i].name, inputs[i].data, inputs[i].len, &objects[i], &err)) {
            error(fmt("%s: %s", inputs[i].name, err));
        }
    }
    /* The script's errors, then the files', all of them (an ELF32 one
     * among them, a member too, needed or not, as ld.lld reads its lazy
     * objects), and the link stops. */
    if (errors.n != 0) {
        return NULL;
    }
    struct strmap names;
    memset(&names, 0, sizeof names);
    for (uint i = 0; i < n; i++) {
        if (!lazy[i]) {
            add_defines(&names, &objects[i]);
        }
    }
    for (uint i = 0; i < n; i++) {
        if (!lazy[i]) {
            add_needs(&names, &objects[i]);
        }
    }
    uchar *pulled = xcalloc(n + 1);
    for (;;) {
        int progress = 0;
        for (uint i = 0; i < n; i++) {
            if (!lazy[i] || pulled[i]) {
                continue;
            }
            struct object *m = &objects[i];
            int wanted = 0;
            for (uint k = 1; k < m->nsymbols && !wanted; k++) {
                wanted = defines(&m->symbols[k]) && (state(&names, m->symbols[k].name) & UNDEFINED);
            }
            if (!wanted) {
                continue;
            }
            pulled[i] = 1;
            progress = 1;
            add_defines(&names, m);
            add_needs(&names, m);
        }
        if (!progress) {
            break;
        }
    }
    /* The files that are not members, in order, then the members needed. */
    struct object *all = xcalloc((n ? n : 1) * sizeof(struct object));
    uint k = 0;
    for (uint i = 0; i < n; i++) {
        if (!lazy[i]) {
            all[k++] = objects[i];
        }
    }
    for (uint i = 0; i < n; i++) {
        if (pulled[i]) {
            all[k++] = objects[i];
        }
    }
    *count = k;
    return all;
}
