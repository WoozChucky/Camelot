/* ld: symbol resolution and section layout, everything up to relocation
 * (merlin-ld's layout.rs). In order:
 *
 *   1. The objects (parsed by library.c) and the script.
 *   2. The global symbol table. Two strong definitions of a global are an
 *      error; a strong one beats a weak one.
 *   3. Input sections to output sections: script order, the first matching
 *      *(pattern) wins, objects in command-line order, sections in file
 *      order. An allocated input section no rule matches is an error if it
 *      is non-empty or defines a global.
 *   4. SECTIONS walked with the location counter, each input section placed
 *      at its alignment, the script's assignments evaluated as they come.
 *      As in ld.lld, an output section is dropped only when no input section
 *      (not even an empty one) matched it and it has no assignments.
 *   5. Debug information: each .debug_* input section goes into the output
 *      section of its name, after everything loaded; a symbol in one is its
 *      offset there.
 *   6. Undefined globals, then the entry point.
 *
 * The location counter is a 64-bit number (merlin-ld's u64): a section may
 * end exactly at 4 GiB, and going past it is an error, not a wrap. */
#include "ld.h"

/* The link's state while it is laid out. */
static struct layout *L;
static struct script S;
static const char *script_name;

static void script_err(uint line, char *msg) {
    error(fmt("%s:%u: %s", script_name, line, msg));
}

static int is_alloc(struct section *s) {
    return (s->flags & SHF_ALLOC) != 0;
}

/* align_up(v, a) for v >= 0 and a > 0: the next multiple of a. */
static i64 align_up(i64 v, i64 a) {
    i64 r;
    i64_rem(v, a, &r);
    if (r.lo == 0 && r.hi == 0) {
        return v;
    }
    return i64_add(v, i64_sub(a, r));
}

static i64 four_gib(void) {
    i64 v;
    v.lo = 0;
    v.hi = 1;
    return v;
}

/* ---- Symbols ---- */

static struct section *section_of(uint obj, uint sec) {
    struct object *o = &L->objects[obj];
    return sec < o->nsections ? &o->sections[sec] : NULL;
}

/* Where an input section starts: its address if it is loaded, its offset in
 * its output section if it is debug information. */
static int section_base(uint obj, uint sec, uint *out) {
    struct section *s = section_of(obj, sec);
    if (s == NULL || s->placed == NOT_PLACED) {
        return 0;
    }
    *out = s->addr;
    return 1;
}

int object_symbol(struct layout *l, uint obj, uint sym, uint *out) {
    struct symbol *s = &l->objects[obj].symbols[sym];
    if (s->shndx == SHN_UNDEF) {
        return 0;
    }
    if (s->shndx == SHN_ABS) {
        *out = s->value;
        return 1;
    }
    uint base;
    if (!section_base(obj, s->shndx, &base)) {
        return 0;
    }
    *out = base + s->value;
    return 1;
}

static int global_def_value(struct global *g, uint *out) {
    if (g->obj < 0) {
        *out = g->value;
        return 1;
    }
    return object_symbol(L, (uint)g->obj, g->sym, out);
}

int global_value(struct layout *l, const char *name, uint *out) {
    int i;
    if (!map_get(&l->global_index, name, &i)) {
        return 0;
    }
    return global_def_value(&l->globals[i], out);
}

int in_dropped_section(struct layout *l, uint obj, uint sym) {
    struct symbol *s = &l->objects[obj].symbols[sym];
    uint base;
    return s->shndx != SHN_UNDEF && s->shndx < SHN_LORESERVE && !section_base(obj, s->shndx, &base);
}

int is_undefined_weak(struct layout *l, uint obj, uint sym) {
    struct symbol *s = &l->objects[obj].symbols[sym];
    int i;
    return sym != 0 && sym_is_weak(s) && sym_is_undefined(s) && !map_get(&l->global_index, s->name, &i);
}

/* Why input section `sec` of `obj` has no address. */
static const char *unplaced(uint obj, uint sec) {
    struct section *s = section_of(obj, sec);
    return s != NULL && s->discarded ? "was discarded by /DISCARD/" : "is not loaded";
}

int reloc_symbol(struct layout *l, uint obj, uint sym, uint *out, char **err) {
    struct object *o = &l->objects[obj];
    struct symbol *s = &o->symbols[sym];
    if (sym == 0) {
        *out = 0;
        return 1;
    }
    if (sym_is_local(s)) {
        if (s->type == STT_SECTION) {
            if (section_base(obj, s->shndx, out)) {
                return 1;
            }
            *err = fmt("refers to section '%s', which %s", symbol_display_name(o, sym), unplaced(obj, s->shndx));
            return 0;
        }
        if (object_symbol(l, obj, sym, out)) {
            return 1;
        }
        *err = fmt("refers to '%s', whose section %s", s->name, unplaced(obj, s->shndx));
        return 0;
    }
    int i;
    if (map_get(&l->global_index, s->name, &i)) {
        struct global *g = &l->globals[i];
        if (global_def_value(g, out)) {
            return 1;
        }
        const char *why =
            g->obj >= 0 ? unplaced((uint)g->obj, l->objects[g->obj].symbols[g->sym].shndx) : "is not loaded";
        *err = fmt("refers to '%s', whose section %s", s->name, why);
        return 0;
    }
    if (sym_is_weak(s)) {
        *out = 0;
        return 1;
    }
    *err = fmt("undefined reference to '%s'", s->name);
    return 0;
}

static uint capglobals;

static void add_global(char *name, int obj, uint sym, uint value) {
    map_put(&L->global_index, name, (int)L->nglobals);
    L->globals = grow(L->globals, &capglobals, L->nglobals + 1, sizeof(struct global));
    struct global *g = &L->globals[L->nglobals++];
    g->name = name;
    g->obj = obj;
    g->sym = sym;
    g->value = value;
}

/* ---- The script's expressions ---- */

static int eval(struct expr *e, i64 dot, i64 *out, char **err) {
    i64 a, b;
    int i;
    uint v;
    switch (e->kind) {
    case X_NUM:
        *out = e->num;
        return 1;
    case X_DOT:
        *out = dot;
        return 1;
    case X_SYMBOL:
        if (map_get(&L->global_index, e->name, &i)) {
            struct global *g = &L->globals[i];
            int known;
            if (g->obj < 0) {
                v = g->value;
                known = 1;
            } else {
                struct symbol *s = &L->objects[g->obj].symbols[g->sym];
                struct section *sec = section_of((uint)g->obj, s->shndx);
                known = s->shndx == SHN_ABS || (sec != NULL && sec->placed == PLACED);
                v = s->shndx == SHN_ABS ? s->value : known ? sec->addr + s->value : 0;
            }
            if (known) {
                *out = i64_u(v);
                return 1;
            }
            *err = fmt("symbol '%s' is used before its section is placed", e->name);
            return 0;
        }
        *err = fmt("undefined symbol '%s' in expression", e->name);
        return 0;
    case X_ALIGN:
        if (!eval(e->a, dot, &a, err)) {
            return 0;
        }
        if (i64_cmp(a, i64_u(0)) <= 0) {
            *err = fmt("ALIGN(%s): alignment must be positive", i64_dec(a));
            return 0;
        }
        *out = align_up(dot, a);
        return 1;
    case X_ORIGIN:
    case X_LENGTH:
        if (!S.has_memory || strcmp(S.region_name, e->name) != 0) {
            *err = fmt("unknown memory region '%s'", e->name);
            return 0;
        }
        *out = i64_u(e->kind == X_ORIGIN ? S.origin : S.length);
        return 1;
    default:
        if (!eval(e->a, dot, &a, err) || !eval(e->b, dot, &b, err)) {
            return 0;
        }
        /* checked_add / checked_sub */
        i64 r = e->kind == X_ADD ? i64_add(a, b) : i64_sub(a, b);
        int na = i64_is_neg(a), nb = i64_is_neg(b), nr = i64_is_neg(r);
        int overflow = e->kind == X_ADD ? (na == nb && nr != na) : (na != nb && nr != na);
        if (overflow) {
            *err = "arithmetic overflow";
            return 0;
        }
        *out = r;
        return 1;
    }
}

/* Runs one assignment. Returns the new location counter. */
static i64 assign(struct assign *a, i64 dot) {
    i64 v;
    char *err;
    if (!eval(a->e, dot, &v, &err)) {
        script_err(a->line, err);
        return dot;
    }
    if (a->symbol == NULL) {
        i64 n = v;
        if (a->add) {
            n = i64_add(dot, v);
            /* dot >= 0: only a positive v can overflow. */
            if (!i64_is_neg(v) && i64_is_neg(n)) {
                script_err(a->line, "arithmetic overflow");
                return dot;
            }
        }
        if (i64_cmp(n, dot) < 0) {
            script_err(a->line, fmt("cannot move the location counter backward (from %s to %s)", i64_hex(dot),
                                    i64_hex(n)));
            return dot;
        }
        if (i64_cmp(n, four_gib()) > 0) {
            script_err(a->line, fmt("location counter %s is past the 4 GiB address space", i64_hex(n)));
            return dot;
        }
        return n;
    }
    if (v.hi != 0) {
        script_err(a->line, fmt("value %s of '%s' does not fit in 32 bits", i64_hex(v), a->symbol));
        return dot;
    }
    /* A script assignment overrides an object's definition (as in ld.lld). */
    int i;
    if (map_get(&L->global_index, a->symbol, &i)) {
        L->globals[i].obj = -1;
        L->globals[i].value = v.lo;
    } else {
        add_global(a->symbol, -1, 0, v.lo);
    }
    return dot;
}

/* ---- Warnings about SHF_MERGE sections ---- */

/* An entry of a mergeable section, for finding the duplicates. */
struct merge_entry {
    uint flags;
    uint entsize;
    uint align;
    uchar *p;
    uint len;
};

static uint bytes_hash(struct merge_entry *e) {
    uint h = 2166136261u ^ e->flags ^ (e->entsize << 8) ^ (e->align << 16);
    for (uint i = 0; i < e->len; i++) {
        h = (h ^ e->p[i]) * 16777619u;
    }
    return h;
}

/* merlin-ld does not merge SHF_MERGE sections (clang's .rodata.str1.1
 * strings): it places them like any other. ld.lld keeps one copy of each
 * identical string or constant, so when an output section holds duplicates
 * it is larger than ld.lld's: this says so, once per output section. */
static void merge_warnings(void) {
    for (uint k = 0; k < L->nsections; k++) {
        struct output_section *os = &L->sections[k];
        struct merge_entry *table = NULL;
        uint cap = 0, count = 0;
        uint dup = 0;
        for (uint j = 0; j < os->ninputs; j++) {
            struct section *sec = &L->objects[os->inputs[j].object].sections[os->inputs[j].section];
            if (!(sec->flags & SHF_MERGE) || sec->type == SHT_NOBITS || sec->entsize == 0) {
                continue;
            }
            uint e = sec->entsize;
            uint at = 0;
            while (at < sec->datalen) {
                uint rest = sec->datalen - at;
                uint len;
                if (sec->flags & SHF_STRINGS) {
                    /* A string runs up to and including its all-zero entry. */
                    len = rest;
                    for (uint c = 0; c * e < rest; c++) {
                        uint n = rest - c * e < e ? rest - c * e : e;
                        uint zero = 1;
                        for (uint b = 0; b < n && zero; b++) {
                            zero = sec->data[at + c * e + b] == 0;
                        }
                        if (zero) {
                            len = (c + 1) * e < rest ? (c + 1) * e : rest;
                            break;
                        }
                    }
                } else {
                    len = e < rest ? e : rest;
                }
                struct merge_entry me;
                me.flags = sec->flags;
                me.entsize = sec->entsize;
                me.align = sec->align;
                me.p = sec->data + at;
                me.len = len;
                if ((count + 1) * 2 > cap) {
                    struct merge_entry *old = table;
                    uint oldcap = cap;
                    cap = cap ? cap * 2 : 64;
                    table = xcalloc(cap * sizeof(struct merge_entry));
                    for (uint i = 0; i < oldcap; i++) {
                        if (old[i].p != NULL) {
                            uint h = bytes_hash(&old[i]) & (cap - 1);
                            while (table[h].p != NULL) {
                                h = (h + 1) & (cap - 1);
                            }
                            table[h] = old[i];
                        }
                    }
                    free(old);
                }
                uint h = bytes_hash(&me) & (cap - 1);
                int seen = 0;
                while (table[h].p != NULL) {
                    struct merge_entry *t = &table[h];
                    if (t->flags == me.flags && t->entsize == me.entsize && t->align == me.align &&
                        t->len == me.len && memcmp(t->p, me.p, me.len) == 0) {
                        seen = 1;
                        break;
                    }
                    h = (h + 1) & (cap - 1);
                }
                if (seen) {
                    dup += len;
                } else {
                    table[h] = me;
                    count++;
                }
                at += len;
            }
        }
        free(table);
        if (dup > 0) {
            add_message(&warnings,
                        fmt("%s: mergeable (SHF_MERGE) input sections hold %u bytes of duplicate strings or "
                            "constants; merlin-ld keeps them (it does not merge), so this section is larger than "
                            "ld.lld's and the addresses after it differ",
                            os->name, dup));
        }
    }
}

/* ---- The layout ---- */

/* The input sections a `*(...)` matched, in order. */
struct pair {
    uint obj;
    uint sec;
};

struct rule {
    struct pair *in;
    uint n;
    uint cap;
};

static void rule_push(struct rule *r, uint obj, uint sec) {
    r->in = grow(r->in, &r->cap, r->n + 1, sizeof(struct pair));
    r->in[r->n].obj = obj;
    r->in[r->n].sec = sec;
    r->n++;
}

/* rules[c][i]: what item i of command c matched (empty for assignments). */
static struct rule **rules;

static int pattern_matches(struct item *it, const char *name) {
    for (uint p = 0; p < it->npatterns; p++) {
        if (glob_match(it->patterns[p], name)) {
            return 1;
        }
    }
    return 0;
}

static void assign_inputs(int have_script) {
    rules = xcalloc((S.ncommands + 1) * sizeof(struct rule *));
    for (uint c = 0; c < S.ncommands; c++) {
        struct command *cmd = &S.commands[c];
        if (!cmd->is_output) {
            continue;
        }
        rules[c] = xcalloc((cmd->out.nitems + 1) * sizeof(struct rule));
        for (uint it = 0; it < cmd->out.nitems; it++) {
            struct item *item = &cmd->out.items[it];
            if (item->is_assign) {
                continue;
            }
            for (uint oi = 0; oi < L->nobjects; oi++) {
                struct object *o = &L->objects[oi];
                for (uint si = 0; si < o->nsections; si++) {
                    struct section *sec = &o->sections[si];
                    if (!is_alloc(sec) || sec->assigned) {
                        continue;
                    }
                    if (pattern_matches(item, sec->name)) {
                        sec->assigned = 1;
                        rule_push(&rules[c][it], oi, si);
                    }
                }
            }
        }
    }
    for (uint oi = 0; oi < L->nobjects; oi++) {
        struct object *o = &L->objects[oi];
        for (uint si = 0; si < o->nsections; si++) {
            struct section *sec = &o->sections[si];
            if (!is_alloc(sec) || sec->assigned) {
                continue;
            }
            /* An empty section can be left out, unless a global lives in it. */
            int defines_global = 0;
            for (uint k = 1; k < o->nsymbols && !defines_global; k++) {
                struct symbol *s = &o->symbols[k];
                defines_global = !sym_is_local(s) && s->shndx == si && s->shndx < SHN_LORESERVE;
            }
            if (sec->size > 0 || defines_global) {
                char *by = have_script ? fmt("linker script %s", script_name)
                                       : "the default layout (use a linker script with -T)";
                error(fmt("%s: section '%s' is not placed by %s", o->name, sec->name, by));
            }
        }
    }
}

static uint capsections;

/* One output section of the walk (merlin-ld's Command::Output arm).
 * *dot, *cursor and *dot_moved are the walk's state. */
static void place_output(uint c, struct output_desc *o, i64 *dot, int has_cursor, i64 *cursor, uint *dot_moved,
                         uint *prev_flags) {
    struct rule *r = rules[c];
    uint ninputs = 0;
    int has_assign = 0;
    uint align = 1;
    for (uint it = 0; it < o->nitems; it++) {
        ninputs += r[it].n;
        has_assign |= o->items[it].is_assign;
        for (uint k = 0; k < r[it].n; k++) {
            uint a = L->objects[r[it].in[k].obj].sections[r[it].in[k].sec].align;
            align = a > align ? a : align;
        }
    }
    if (ninputs == 0 && !has_assign) {
        return;
    }
    /* `.name : ALIGN(n) { ... }`: at least n. */
    if (o->align != NULL) {
        i64 n;
        char *err;
        if (eval(o->align, *dot, &n, &err)) {
            int pow2 = !i64_is_neg(n) && n.hi == 0 && n.lo != 0 && (n.lo & (n.lo - 1)) == 0;
            if (pow2 && n.lo <= 0x80000000u) {
                align = n.lo > align ? n.lo : align;
            } else {
                script_err(o->line, fmt("ALIGN(%s) of section %s: alignment must be a power of two", i64_dec(n),
                                        o->name));
            }
        } else {
            script_err(o->line, err);
        }
    }
    i64 base = has_cursor ? *cursor : *dot;
    i64 start = align_up(base, i64_u(align));
    i64 d = start;
    struct output_section os;
    memset(&os, 0, sizeof os);
    uint flags = SHF_ALLOC;
    int nobits = 1;
    for (uint it = 0; it < o->nitems; it++) {
        if (o->items[it].is_assign) {
            d = assign(&o->items[it].assign, d);
            continue;
        }
        for (uint k = 0; k < r[it].n; k++) {
            struct section *sec = &L->objects[r[it].in[k].obj].sections[r[it].in[k].sec];
            d = align_up(d, i64_u(sec->align));
            sec->placed = PLACED;
            sec->addr = d.lo;
            sec->out = -1;
            os.inputs = grow(os.inputs, &os.capinputs, os.ninputs + 1, sizeof(struct input_ref));
            os.inputs[os.ninputs].object = r[it].in[k].obj;
            os.inputs[os.ninputs].section = r[it].in[k].sec;
            os.inputs[os.ninputs].addr = d.lo;
            os.ninputs++;
            d = i64_add(d, i64_u(sec->size));
            flags |= sec->flags & (SHF_WRITE | SHF_EXECINSTR);
            nobits &= sec->type == SHT_NOBITS;
        }
    }
    /* d may be exactly 4 GiB, but the size must fit in 32 bits. */
    i64 size = i64_sub(d, start);
    if (size.hi != 0 || i64_cmp(d, four_gib()) > 0) {
        script_err(o->line, fmt("section %s extends past the 4 GiB address space", o->name));
        return;
    }
    if (os.ninputs == 0) {
        /* Only assignments: like ld.lld, PROGBITS with the previous flags. */
        flags = *prev_flags;
        nobits = 0;
    } else {
        *prev_flags = flags;
    }
    if (o->noload) {
        nobits = 1;
    }
    if (has_cursor) {
        if (*dot_moved != 0 && !i64_eq(*dot, *cursor)) {
            script_err(*dot_moved, fmt("'.' is moved before output section %s, which is placed at the next free "
                                       "address of MEMORY region %s instead; move the assignment inside the section",
                                       o->name, S.region_name));
        }
        i64 end = i64_add(i64_u(S.origin), i64_u(S.length));
        if (i64_cmp(d, end) > 0) {
            script_err(o->line, fmt("section %s overflows MEMORY region %s by %s bytes", o->name, S.region_name,
                                    i64_dec(i64_sub(d, end))));
        }
        *cursor = d;
    }
    for (uint k = 0; k < os.ninputs; k++) {
        L->objects[os.inputs[k].object].sections[os.inputs[k].section].out = (int)L->nsections;
    }
    os.name = o->name;
    os.addr = start.lo;
    os.size = size.lo;
    os.align = align;
    os.flags = flags;
    os.nobits = nobits;
    L->sections = grow(L->sections, &capsections, L->nsections + 1, sizeof(struct output_section));
    L->sections[L->nsections++] = os;
    *dot = d;
    *dot_moved = 0;
}

static void walk(void) {
    /* The location counter starts at 0, also with MEMORY (as in ld.lld):
     * only output sections go at the region's next free address. */
    i64 dot = i64_u(0);
    i64 cursor = i64_u(S.origin);
    /* Line of the last top-level `.` move since the previous output section. */
    uint dot_moved = 0;
    /* Flags of the last output section that had input sections. */
    uint prev_flags = SHF_ALLOC;
    for (uint c = 0; c < S.ncommands; c++) {
        struct command *cmd = &S.commands[c];
        if (!cmd->is_output) {
            i64 before = dot;
            dot = assign(&cmd->assign, dot);
            if (cmd->assign.symbol == NULL && !i64_eq(dot, before)) {
                dot_moved = cmd->assign.line;
            }
        } else if (strcmp(cmd->out.name, DISCARD_NAME) == 0) {
            for (uint it = 0; it < cmd->out.nitems; it++) {
                for (uint k = 0; k < rules[c][it].n; k++) {
                    L->objects[rules[c][it].in[k].obj].sections[rules[c][it].in[k].sec].discarded = 1;
                }
            }
        } else {
            place_output(c, &cmd->out, &dot, S.has_memory, &cursor, &dot_moved, &prev_flags);
        }
    }
}

static int is_debug(struct section *s) {
    return !is_alloc(s) && s->type == SHT_PROGBITS && strncmp(s->name, ".debug_", 7) == 0;
}

static void place_debug(void) {
    uint cap = 0;
    uint merge = SHF_MERGE | SHF_STRINGS;
    for (uint oi = 0; oi < L->nobjects; oi++) {
        struct object *o = &L->objects[oi];
        for (uint si = 0; si < o->nsections; si++) {
            struct section *sec = &o->sections[si];
            if (!is_debug(sec)) {
                continue;
            }
            struct debug_section *out = NULL;
            for (uint k = 0; k < L->ndebug && out == NULL; k++) {
                if (strcmp(L->debug[k].name, sec->name) == 0) {
                    out = &L->debug[k];
                }
            }
            if (out == NULL) {
                L->debug = grow(L->debug, &cap, L->ndebug + 1, sizeof(struct debug_section));
                out = &L->debug[L->ndebug++];
                memset(out, 0, sizeof *out);
                out->name = sec->name;
                out->align = 1;
                out->flags = sec->flags & merge;
                out->entsize = sec->entsize;
            }
            if ((sec->flags & merge) != out->flags || sec->entsize != out->entsize) {
                out->flags = 0;
                out->entsize = 0;
            }
            uint at = align_up(i64_u(out->size), i64_u(sec->align)).lo;
            out->align = sec->align > out->align ? sec->align : out->align;
            out->inputs = grow(out->inputs, &out->capinputs, out->ninputs + 1, sizeof(struct input_ref));
            out->inputs[out->ninputs].object = oi;
            out->inputs[out->ninputs].section = si;
            out->inputs[out->ninputs].addr = at;
            out->ninputs++;
            sec->placed = PLACED_DEBUG;
            sec->addr = at;
            out->size = at + sec->size;
        }
    }
}

static void globals(void) {
    for (uint oi = 0; oi < L->nobjects; oi++) {
        struct object *o = &L->objects[oi];
        for (uint si = 1; si < o->nsymbols; si++) {
            struct symbol *s = &o->symbols[si];
            if (sym_is_local(s) || sym_is_undefined(s)) {
                continue;
            }
            if (s->shndx != SHN_ABS && !is_alloc(&o->sections[s->shndx])) {
                error(fmt("%s: global symbol '%s' is defined in section '%s', which is not loaded", o->name, s->name,
                          o->sections[s->shndx].name));
                continue;
            }
            int gi;
            if (!map_get(&L->global_index, s->name, &gi)) {
                add_global(s->name, (int)oi, si, 0);
                continue;
            }
            struct global *g = &L->globals[gi];
            struct object *first = &L->objects[g->obj];
            if (sym_is_weak(s)) {
                continue;
            }
            if (sym_is_weak(&first->symbols[g->sym])) {
                g->obj = (int)oi;
                g->sym = si;
            } else {
                error(fmt("%s: duplicate symbol '%s' (first defined in %s)", o->name, s->name, first->name));
            }
        }
    }
}

static void undefined_references(void) {
    for (uint oi = 0; oi < L->nobjects; oi++) {
        struct object *o = &L->objects[oi];
        struct strmap seen;
        memset(&seen, 0, sizeof seen);
        for (uint si = 1; si < o->nsymbols; si++) {
            struct symbol *s = &o->symbols[si];
            int dummy;
            if (!sym_is_local(s) && sym_is_undefined(s) && !sym_is_weak(s) &&
                !map_get(&L->global_index, s->name, &dummy) && !map_get(&seen, s->name, &dummy)) {
                map_put(&seen, s->name, 1);
                error(fmt("%s: undefined reference to '%s'", o->name, s->name));
            }
        }
        free(seen.keys);
        free(seen.values);
    }
}

/* -e, else ENTRY(...), else _start. */
static void entry_point(const char *entry_opt) {
    int explicit = entry_opt != NULL;
    const char *entry = entry_opt != NULL ? entry_opt : S.entry != NULL ? S.entry : "_start";
    i64 n;
    uint v;
    if (parse_number(entry, &n)) {
        if (n.hi == 0) {
            L->entry = n.lo;
        } else {
            error(fmt("entry address %s does not fit in 32 bits", entry));
        }
    } else if (global_value(L, entry, &v)) {
        L->entry = v;
    } else if (explicit) {
        error(fmt("entry symbol '%s' is not defined", entry));
    } else {
        uint fallback = 0;
        int found = 0;
        for (uint k = 0; k < L->nsections && !found; k++) {
            if (L->sections[k].flags & SHF_EXECINSTR) {
                fallback = L->sections[k].addr;
                found = 1;
            }
        }
        if (!found && L->nsections > 0) {
            fallback = L->sections[0].addr;
        }
        L->entry = fallback;
        add_message(&warnings, fmt("cannot find entry symbol '%s'; defaulting to 0x%08x", entry, fallback));
    }
}

int layout_build(struct object *objects, uint nobjects, const char *name, const char *text, const char *entry_opt,
                 struct layout *l) {
    memset(l, 0, sizeof *l);
    L = l;
    l->objects = objects;
    l->nobjects = nobjects;
    if (nobjects == 0) {
        error("no input files");
        return 0;
    }
    int have_script = name != NULL;
    script_name = have_script ? name : "<default layout>";
    uint line;
    char *msg;
    if (!script_parse(have_script ? text : DEFAULT_SCRIPT, &S, &line, &msg)) {
        error(fmt("%s:%u: %s", script_name, line, msg));
        return 0;
    }
    uint before = errors.n;
    globals();
    assign_inputs(have_script);
    walk();
    place_debug();
    merge_warnings();
    undefined_references();
    entry_point(entry_opt);
    return errors.n == before;
}
