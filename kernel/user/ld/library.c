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

/* Parses every input, adding an error for each that is not an object. */
static struct object *parse_all(struct input *inputs, uint n) {
    struct object *objects = xcalloc((n ? n : 1) * sizeof(struct object));
    uint before = errors.n;
    for (uint i = 0; i < n; i++) {
        char *err;
        if (!object_parse(inputs[i].name, inputs[i].data, inputs[i].len, &objects[i], &err)) {
            error(fmt("%s: %s", inputs[i].name, err));
        }
    }
    return errors.n == before ? objects : NULL;
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

struct object *parse_with_needed(struct input *inputs, uint ninputs, struct input *library, uint nlibrary,
                                 int format, uint *n) {
    struct object *objects = parse_all(inputs, ninputs);
    if (objects == NULL) {
        return NULL;
    }
    struct object *members = parse_all(library, nlibrary);
    if (members == NULL) {
        return NULL;
    }
    /* Every object and every member, needed or not, must be of the
     * executable's class, the script's OUTPUT_FORMAT's if it names one, as
     * ld.lld checks its lazy objects too: merlin-ld's check_classes. The
     * objects read here are all ELF32 (object.c refuses the others, until
     * #157), so only an elf64-littleriscv format can be another class, and
     * then each of them is: all are reported, in order, and the link stops. */
    if (ninputs > 0 && format == FORMAT_ELF64) {
        for (uint i = 0; i < ninputs; i++) {
            error(fmt("%s is incompatible with %s", objects[i].name, format_name(format)));
        }
        for (uint i = 0; i < nlibrary; i++) {
            error(fmt("%s is incompatible with %s", members[i].name, format_name(format)));
        }
        return NULL;
    }
    struct strmap names;
    memset(&names, 0, sizeof names);
    for (uint i = 0; i < ninputs; i++) {
        add_defines(&names, &objects[i]);
    }
    for (uint i = 0; i < ninputs; i++) {
        add_needs(&names, &objects[i]);
    }
    uchar *pulled = xcalloc(nlibrary + 1);
    for (;;) {
        int progress = 0;
        for (uint i = 0; i < nlibrary; i++) {
            if (pulled[i]) {
                continue;
            }
            struct object *m = &members[i];
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
    uint count = ninputs;
    for (uint i = 0; i < nlibrary; i++) {
        count += pulled[i];
    }
    struct object *all = xcalloc((count ? count : 1) * sizeof(struct object));
    memcpy(all, objects, ninputs * sizeof(struct object));
    uint k = ninputs;
    for (uint i = 0; i < nlibrary; i++) {
        if (pulled[i]) {
            all[k++] = members[i];
        }
    }
    *n = count;
    return all;
}
