/* ld: libraries, objects linked only when a program needs them: an
 * archive's members and the objects between --start-lib and --end-lib,
 * linked as ld.lld links them (merlin-ld's library.rs, which explains the
 * rules). The files are read in command-line order with one symbol table.
 * A file that is linked goes on the list, then its definitions are
 * resolved, then its references, each of which extracts the library
 * member that lazily defines it there and then (depth first); a weak
 * reference extracts nothing. A member is only noted: what it defines
 * becomes lazy, unless the program already refers to one of its symbols,
 * which extracts it at once. After the last file, a lazy entry symbol
 * extracts its member. */
#include "ld.h"

/* What a name is in the symbol table (absent: 0). A lazy symbol is
 * LAZY + 2 * its member's index, + 1 if only weak references want it. */
#define UNDEFINED 1
#define UNDEFINED_WEAK 2
#define DEFINED 3
#define LAZY 4

static int defines(struct symbol *s) {
    return !sym_is_local(s) && !sym_is_undefined(s) && s->name[0] != 0;
}

static int refers(struct symbol *s) {
    return !sym_is_local(s) && sym_is_undefined(s) && s->name[0] != 0;
}

/* The resolution: the objects, which are still lazy, the symbol table and
 * the linked files in their order. */
static struct object *objs;
static int *still_lazy;
static struct strmap table;
static uint *linked;
static uint nlinked;

static int state(char *name) {
    int v = 0;
    map_get(&table, name, &v);
    return v;
}

static void extract(uint i);

/* Links file i: on the list, then its definitions, then its references. */
static void link_file(uint i) {
    still_lazy[i] = 0;
    linked[nlinked++] = i;
    struct object *o = &objs[i];
    for (uint k = 1; k < o->nsymbols; k++) {
        if (defines(&o->symbols[k])) {
            map_put(&table, o->symbols[k].name, DEFINED);
        }
    }
    for (uint k = 1; k < o->nsymbols; k++) {
        struct symbol *s = &o->symbols[k];
        if (!refers(s)) {
            continue;
        }
        int weak = sym_is_weak(s);
        int v = state(s->name);
        if (v == 0) {
            map_put(&table, s->name, weak ? UNDEFINED_WEAK : UNDEFINED);
        } else if (v == UNDEFINED_WEAK && !weak) {
            map_put(&table, s->name, UNDEFINED);
        } else if (v >= LAZY && weak) {
            map_put(&table, s->name, v | 1);
        } else if (v >= LAZY) {
            extract((uint)(v - LAZY) / 2);
        }
    }
}

/* Notes member i's definitions as lazy, or extracts it if one of them is
 * already wanted. */
static void note(uint i) {
    struct object *o = &objs[i];
    for (uint k = 1; k < o->nsymbols; k++) {
        struct symbol *s = &o->symbols[k];
        if (!defines(s)) {
            continue;
        }
        int v = state(s->name);
        if (v == 0) {
            map_put(&table, s->name, LAZY + 2 * (int)i);
        } else if (v == UNDEFINED_WEAK) {
            map_put(&table, s->name, LAZY + 2 * (int)i + 1);
        } else if (v == UNDEFINED) {
            extract(i);
            return;
        }
    }
}

static void extract(uint i) {
    if (still_lazy[i]) {
        link_file(i);
    }
}

struct object *parse_with_needed(struct input *inputs, int *lazy, uint n, const char *entry, uint *count) {
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
    uint members = 0;
    for (uint i = 0; i < n; i++) {
        members += lazy[i] != 0;
    }
    if (members == n) {
        error("no input files");
        return NULL;
    }
    objs = objects;
    still_lazy = xcalloc((n + 1) * sizeof(int));
    linked = xcalloc((n + 1) * sizeof(uint));
    nlinked = 0;
    memset(&table, 0, sizeof table);
    for (uint i = 0; i < n; i++) {
        still_lazy[i] = lazy[i];
    }
    for (uint i = 0; i < n; i++) {
        if (!lazy[i]) {
            link_file(i);
        } else if (still_lazy[i]) {
            note(i);
        }
    }
    if (entry != NULL) {
        int v = state((char *)entry);
        if (v >= LAZY) {
            extract((uint)(v - LAZY) / 2);
        }
    }
    struct object *all = xcalloc((n ? n : 1) * sizeof(struct object));
    for (uint k = 0; k < nlinked; k++) {
        all[k] = objects[linked[k]];
    }
    *count = nlinked;
    return all;
}
