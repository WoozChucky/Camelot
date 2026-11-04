/* ld: symbol resolution and section layout, everything up to relocation
 * (merlin-ld's layout.rs). In order:
 *
 *   1. The objects (parsed by library.c) and the script.
 *   2. The global symbol table. Two strong definitions of a global are an
 *      error; a strong one beats a weak one.
 *   3. Input sections to output sections: script order, the first matching
 *      *(pattern) wins, objects in command-line order, sections in file
 *      order. An allocated input section no rule matches is an error if it
 *      is non-empty or defines a global; an empty one is an orphan, placed
 *      as ld.lld places one (place_orphans).
 *   4. SECTIONS walked with the location counter, each input section placed
 *      at its alignment, the script's assignments evaluated as they come.
 *      As in ld.lld, an output section is dropped only when no input section
 *      (not even an empty one) matched it and it has no assignments, and an
 *      expression does not need it (an ADDR names it, or its AT or ALIGN
 *      uses a symbol). Then the load addresses must not overlap.
 *   5. Debug information: each .debug_* input section goes into the output
 *      section of its name, after everything loaded; a symbol in one is its
 *      offset there.
 *   6. Undefined globals, then the entry point.
 *
 * The location counter is a 64-bit number (merlin-ld's u64): a section may
 * end exactly at 4 GiB, and going past it is an error, not a wrap.
 *
 * Load addresses (AT), as merlin-ld's layout.rs explains them: a section
 * runs at its address (VMA) and is loaded at its load address (LMA, its
 * segment's p_paddr), the same unless the script says `AT(lma)`. In AT, `.`
 * is the section's aligned start. The difference LMA - VMA (modulo 2^64)
 * carries on to every later section without an AT, NOLOAD and empty ones
 * too; a dropped section evaluates nothing. A section with an AT starts a
 * segment of its own (write.c). Load ranges must stay within 4 GiB and not
 * overlap. ADDR(.x) is the address of output section .x: one already
 * placed, or the one being placed (in its AT and its assignments).
 *
 * The executable's class is the objects', ELF32 (object.c reads no other,
 * until #157). A script whose OUTPUT_FORMAT says elf64-littleriscv is
 * refused before the layout, with the objects (library.c), as merlin-ld's
 * link_with_library does. */
#include "ld.h"

/* The link's state while it is laid out. */
static struct layout *L;
static struct script S;
static const char *script_name;
/* The output section being placed and its start, once that is known: what
 * ADDR of it is, in its AT and among its assignments. */
static int has_current;
static const char *current_name;
static i64 current_start;

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

/* Whether a, as 64 bits, is a power of two. */
static int is_pow2(i64 a) {
    return !i64_eq(a, i64_u(0)) && i64_eq(i64_and(a, i64_sub(a, i64_u(1))), i64_u(0));
}

/* v rounded up to a multiple of a, a power of two (2^63 too): (v + a - 1)
 * & !(a - 1) modulo 2^64, as ld.lld aligns the location counter. */
static i64 align_pow2(i64 v, i64 a) {
    i64 m = i64_sub(a, i64_u(1));
    return i64_and(i64_add(v, m), i64_not(m));
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

/* Whether symbol `sym` of `obj`, as the link resolves it, has no place in
 * the executable: defined in a section not loaded or thrown away (a global
 * by its definition, in whichever object that is), or a weak reference
 * nobody defines. Debug information gets ld.lld's tombstone there. */
int in_dropped_section(struct layout *l, uint obj, uint sym) {
    struct symbol *s = &l->objects[obj].symbols[sym];
    uint base;
    if (sym != 0 && !sym_is_local(s)) {
        int i;
        if (!map_get(&l->global_index, s->name, &i)) {
            return sym_is_weak(s);
        }
        struct global *g = &l->globals[i];
        if (g->obj < 0) {
            return 0;
        }
        obj = (uint)g->obj;
        s = &l->objects[obj].symbols[g->sym];
    }
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
        /* ld.lld's checkAlignment: 0 is 1, the rest must be powers of two
         * (a negative value, read as 64 bits, is none but -2^63). */
        if (!is_pow2(i64_eq(a, i64_u(0)) ? i64_u(1) : a)) {
            *err = fmt("ALIGN(%s): alignment must be power of 2", i64_dec(a));
            return 0;
        }
        *out = align_pow2(dot, i64_eq(a, i64_u(0)) ? i64_u(1) : a);
        return 1;
    case X_ORIGIN:
    case X_LENGTH:
        if (!S.has_memory || strcmp(S.region_name, e->name) != 0) {
            *err = fmt("unknown memory region '%s'", e->name);
            return 0;
        }
        *out = i64_u(e->kind == X_ORIGIN ? S.origin : S.length);
        return 1;
    case X_ADDR:
        /* The address of an output section: one already placed, or the
         * one being placed once its start is known (in its own AT, not in
         * its ALIGN, which that start depends on). The script is walked
         * once, so a later section has no address yet: an error, where
         * ld.lld would go round again, as for symbols. */
        if (has_current && strcmp(current_name, e->name) == 0) {
            *out = current_start;
            return 1;
        }
        for (uint k = 0; k < L->nsections; k++) {
            if (strcmp(L->sections[k].name, e->name) == 0) {
                *out = i64_u(L->sections[k].addr);
                return 1;
            }
        }
        for (uint c = 0; c < S.ncommands; c++) {
            struct command *cmd = &S.commands[c];
            if (cmd->is_output && strcmp(cmd->out.name, DISCARD_NAME) != 0 && strcmp(cmd->out.name, e->name) == 0) {
                *err = fmt("section '%s' is used before it is placed", e->name);
                return 0;
            }
        }
        /* ld.lld's words. (A section of the script that ADDR names is never
         * dropped, so only an unknown name gets here.) */
        *err = fmt("undefined section %s", e->name);
        return 0;
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

/* Runs one assignment, at the top level of SECTIONS or inside an output
 * section (in_section). Returns the new location counter. */
static i64 assign(struct assign *a, i64 dot, int in_section) {
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
        /* Inside an output section `.` only moves forward (a negative value
         * is a move backward too), and as in ld.lld it still takes the new
         * value. At the top level it moves anywhere, as in ld.lld: sections
         * over each other are an error once all are placed. */
        if (in_section && i64_cmp(n, dot) < 0) {
            script_err(a->line, fmt("cannot move the location counter backward (from %s to %s)", i64_hex(dot),
                                    i64_hex(n)));
            return n;
        }
        /* 4 GiB of address space; a negative value, read as 64 bits, is
         * past it too. */
        if (i64_is_neg(n) || i64_cmp(n, four_gib()) > 0) {
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

/* merlin-ld's RELRO_NAMES: the writable sections ld.lld makes RELRO, which
 * end their PT_LOAD and have a PT_GNU_RELRO (write.c). */
static const char *const relro_names[] = {".data.rel.ro", ".bss.rel.ro", ".ctors",     ".dtors",
                                          ".jcr",         ".eh_frame",   ".fini_array", ".init_array",
                                          ".preinit_array", ".dynamic",  ".toc"};

int is_relro(const char *name, uint flags) {
    if (!(flags & SHF_WRITE)) {
        return 0;
    }
    for (uint i = 0; i < sizeof relro_names / sizeof relro_names[0]; i++) {
        if (strcmp(name, relro_names[i]) == 0) {
            return 1;
        }
    }
    return 0;
}

/* ld.lld's sort rank of an output section (getSectionRank), as merlin-ld's
 * section_rank: read-only, executable, writable and executable, RELRO,
 * writable, NOBITS last within each. Never 0 (bit 27 is always set), so 0
 * below means "no rank". */
static uint section_rank(const char *name, uint flags, int nobits) {
    uint rank = 1u << 27;
    int w = (flags & SHF_WRITE) != 0, x = (flags & SHF_EXECINSTR) != 0;
    if (!w && !x) {
        rank |= 1u << 11 | 1u << 9;
    } else if (x) {
        rank |= w ? 1u << 13 : 1u << 12;
    } else {
        rank |= 1u << 14 | 1u << 7 | (is_relro(name, flags) ? 0 : 1u << 8);
    }
    if (nobits) {
        rank |= 1u << 6;
    }
    /* RISC-V: .sdata and .sbss next to each other. */
    if (strcmp(name, ".sdata") == 0 || (nobits && strcmp(name, ".sbss") != 0)) {
        rank |= 1;
    }
    return rank;
}

/* The rank of a command if it is an output section with inputs (not
 * /DISCARD/), else 0. */
static uint command_rank(struct command *cmd, struct rule *r) {
    if (!cmd->is_output || strcmp(cmd->out.name, DISCARD_NAME) == 0) {
        return 0;
    }
    uint flags = SHF_ALLOC, ninputs = 0;
    int nobits = 1;
    for (uint it = 0; it < cmd->out.nitems; it++) {
        for (uint k = 0; k < r[it].n; k++) {
            struct section *s = &L->objects[r[it].in[k].obj].sections[r[it].in[k].sec];
            flags |= s->flags & (SHF_WRITE | SHF_EXECINSTR);
            nobits &= s->type == SHT_NOBITS;
            ninputs++;
        }
    }
    return ninputs == 0 ? 0 : section_rank(cmd->out.name, flags, cmd->out.noload || nobits);
}

static int leading_zeros(uint v) {
    int n = 0;
    while (n < 32 && !(v & (0x80000000u >> n))) {
        n++;
    }
    return n;
}

static int proximity(uint rank, uint orphan) {
    return rank == 0 ? -1 : leading_zeros(rank ^ orphan);
}

/* A symbol assignment (not `.`): an orphan goes after it. */
static int is_symbol_assign(struct command *c) {
    return !c->is_output && c->assign.symbol != NULL;
}

/* merlin-ld's orphan_position, ld.lld's findOrphanPos: where the orphan
 * cmds[orphan] goes among cmds[first..orphan]. */
static uint orphan_position(struct command *cmds, uint *rank, uint first, uint orphan, int memory) {
    uint sec = rank[orphan];
    int best = 0;
    uint i = orphan;
    for (uint j = first; j < orphan; j++) {
        int p = proximity(rank[j], sec);
        if (p > best || (p == best && rank[j] != 0 && rank[j] <= sec)) {
            best = p;
            i = j;
        }
    }
    if (i == orphan) {
        return orphan;
    }
    if (rank[i] <= sec || memory) {
        /* After it, and after the sections that follow it as close. */
        i++;
        for (uint j = i; j < orphan; j++) {
            if (rank[j] == 0) {
                continue;
            }
            if (proximity(rank[j], sec) != best) {
                break;
            }
            i = j + 1;
        }
    } else {
        /* Before it, and before the commands without inputs before it. */
        while (i != first && rank[i - 1] == 0) {
            i--;
        }
    }
    int any = 0;
    for (uint j = i; j < orphan; j++) {
        any |= rank[j] != 0;
    }
    if (!any) {
        return orphan;
    }
    while (i != orphan && is_symbol_assign(&cmds[i])) {
        i++;
    }
    return i;
}

/* Moves elements [mid, end) of an array of `size`-byte elements to `pos`,
 * the ones in [pos, mid) after them (std::rotate). */
static void rotate(void *base, uint size, uint pos, uint mid, uint end) {
    uchar *b = base;
    uchar *tmp = xcalloc((end - mid) * size + 1);
    memcpy(tmp, b + mid * size, (end - mid) * size);
    memmove(b + (pos + end - mid) * size, b + pos * size, (mid - pos) * size);
    memcpy(b + pos * size, tmp, (end - mid) * size);
    free(tmp);
}

/* merlin-ld's place_orphans: the empty orphans (input sections no rule
 * matched) placed among the script's commands as ld.lld places orphans.
 * Those of one name make one output section, or join the script's output
 * section of that name, at its end; a new one goes next to the output
 * section most like it (orphan_position). S.commands and rules are
 * replaced. */
static void place_orphans(struct pair *orphans, uint norphans) {
    if (norphans == 0) {
        return;
    }
    uint in_script = S.ncommands;
    struct command *cmds = xcalloc((in_script + norphans + 1) * sizeof(struct command));
    struct rule **r = xcalloc((in_script + norphans + 1) * sizeof(struct rule *));
    memcpy(cmds, S.commands, in_script * sizeof(struct command));
    memcpy(r, rules, in_script * sizeof(struct rule *));
    uint n = in_script;
    for (uint k = 0; k < norphans; k++) {
        char *name = L->objects[orphans[k].obj].sections[orphans[k].sec].name;
        int named = -1;
        for (uint c = 0; c < in_script && named < 0; c++) {
            if (cmds[c].is_output && strcmp(cmds[c].out.name, DISCARD_NAME) != 0 &&
                strcmp(cmds[c].out.name, name) == 0) {
                named = (int)c;
            }
        }
        if (named >= 0) {
            /* ld.lld's recordSection: into the last *(...) if the section
             * ends with one, else into a new one after its assignments. */
            struct output_desc *o = &cmds[named].out;
            if (o->nitems == 0 || o->items[o->nitems - 1].is_assign) {
                struct item *items = xcalloc((o->nitems + 1) * sizeof(struct item));
                memcpy(items, o->items, o->nitems * sizeof(struct item));
                struct rule *rr = xcalloc((o->nitems + 2) * sizeof(struct rule));
                memcpy(rr, r[named], o->nitems * sizeof(struct rule));
                o->items = items;
                r[named] = rr;
                o->nitems++;
            }
            rule_push(&r[named][o->nitems - 1], orphans[k].obj, orphans[k].sec);
            continue;
        }
        uint c = in_script;
        while (c < n && strcmp(cmds[c].out.name, name) != 0) {
            c++;
        }
        if (c == n) {
            cmds[n].is_output = 1;
            cmds[n].out.name = name;
            cmds[n].out.items = xcalloc(sizeof(struct item));
            cmds[n].out.nitems = 1;
            r[n] = xcalloc(2 * sizeof(struct rule));
            n++;
        }
        rule_push(&r[c][0], orphans[k].obj, orphans[k].sec);
    }
    uint *rank = xcalloc((n + 1) * sizeof(uint));
    for (uint c = 0; c < n; c++) {
        rank[c] = command_rank(&cmds[c], r[c]);
    }
    /* The orphans in rank order (an insertion sort: stable). */
    for (uint c = in_script + 1; c < n; c++) {
        uint j = c;
        while (j > in_script && rank[j - 1] > rank[c]) {
            j--;
        }
        rotate(cmds, sizeof *cmds, j, c, c + 1);
        rotate(r, sizeof *r, j, c, c + 1);
        rotate(rank, sizeof *rank, j, c, c + 1);
    }
    /* ld.lld's "horrible special case": a first `. = ...` before every
     * section stays first, so that every section is after it. */
    uint first = 0;
    while (first < n && is_symbol_assign(&cmds[first])) {
        first++;
    }
    if (first < n && !cmds[first].is_output) {
        first++;
    }
    /* Each orphan moved to its place, with those of the same rank after it. */
    uint next = in_script;
    while (next < n) {
        uint end = next + 1;
        while (end < n && rank[end] == rank[next]) {
            end++;
        }
        uint pos = orphan_position(cmds, rank, first, next, S.has_memory);
        rotate(cmds, sizeof *cmds, pos, next, end);
        rotate(r, sizeof *r, pos, next, end);
        rotate(rank, sizeof *rank, pos, next, end);
        next = end;
    }
    /* An orphan's errors give the line of the command before it. */
    uint line = 1;
    for (uint c = 0; c < n; c++) {
        if (!cmds[c].is_output) {
            line = cmds[c].assign.line;
        } else if (cmds[c].out.line == 0) {
            cmds[c].out.line = line;
        } else {
            line = cmds[c].out.line;
        }
    }
    free(rank);
    S.commands = cmds;
    S.ncommands = n;
    rules = r;
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
    /* The empty sections no rule matches: orphans (place_orphans). */
    struct pair *orphans = NULL;
    uint norphans = 0, caporphans = 0;
    for (uint oi = 0; oi < L->nobjects; oi++) {
        struct object *o = &L->objects[oi];
        for (uint si = 0; si < o->nsections; si++) {
            struct section *sec = &o->sections[si];
            if (!is_alloc(sec) || sec->assigned) {
                continue;
            }
            /* An empty section is an orphan, unless a global lives in it. */
            int defines_global = 0;
            for (uint k = 1; k < o->nsymbols && !defines_global; k++) {
                struct symbol *s = &o->symbols[k];
                defines_global = !sym_is_local(s) && s->shndx == si && s->shndx < SHN_LORESERVE;
            }
            if (sec->size == 0 && !defines_global) {
                orphans = grow(orphans, &caporphans, norphans + 1, sizeof(struct pair));
                orphans[norphans].obj = oi;
                orphans[norphans].sec = si;
                norphans++;
            } else {
                char *by = have_script ? fmt("linker script %s", script_name)
                                       : "the default layout (use a linker script with -T)";
                error(fmt("%s: section '%s' is not placed by %s", o->name, sec->name, by));
            }
        }
    }
    place_orphans(orphans, norphans);
    free(orphans);
}

static uint capsections;

/* Whether a section loaded at `lma` with `size` bytes is within the 4 GiB
 * address space, as its address range must be (ld.lld would truncate
 * p_paddr to 32 bits). `lma` is 64 bits: an AT may move it anywhere. */
static int load_range_fits(i64 lma, uint size) {
    if (lma.hi == 0) {
        return i64_cmp(i64_add(lma, i64_u(size)), four_gib()) <= 0;
    }
    /* At 4 GiB exactly, only an empty range ends there. */
    return lma.hi == 1 && lma.lo == 0 && size == 0;
}

/* One output section of the walk (merlin-ld's Command::Output arm).
 * *dot, *cursor, *dot_moved, *prev_flags and *lma_offset are the walk's
 * state. */
static void place_output(uint c, struct output_desc *o, i64 *dot, int has_cursor, i64 *cursor, uint *dot_moved,
                         uint *prev_flags, i64 *lma_offset) {
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
    /* Nor is a section dropped that an expression needs: one an ADDR names
     * (ld.lld's usedInExpression), or one whose AT or ALIGN uses a symbol
     * (expressionsUseSymbols). A dropped section evaluates nothing, so its
     * AT changes no load address. */
    int needed = script_names_addr(&S, o->name) || (o->at != NULL && expr_uses_symbols(o->at)) ||
                 (o->align != NULL && expr_uses_symbols(o->align));
    if (ninputs == 0 && !has_assign && !needed) {
        return;
    }
    /* `.name : ALIGN(n) { ... }`: at least n. */
    if (o->align != NULL) {
        i64 n;
        char *err;
        if (eval(o->align, *dot, &n, &err)) {
            /* ld.lld's checkAlignment (0 is 1), then at most 2^31. */
            i64 a = i64_eq(n, i64_u(0)) ? i64_u(1) : n;
            if (!is_pow2(a)) {
                script_err(o->line,
                           fmt("ALIGN(%s) of section %s: alignment must be power of 2", i64_dec(n), o->name));
            } else if (a.hi != 0 || a.lo > 0x80000000u) {
                script_err(o->line, fmt("ALIGN(%s) of section %s: alignment must be at most 2^31 (ld.lld keeps 32 "
                                        "bits of a section's alignment and would ignore this one)",
                                        i64_dec(n), o->name));
            } else {
                align = a.lo > align ? a.lo : align;
            }
        } else {
            script_err(o->line, err);
        }
    }
    i64 base = has_cursor ? *cursor : *dot;
    i64 start = align_pow2(base, i64_u(align));
    /* From here on ADDR of this section is `start`. */
    has_current = 1;
    current_name = o->name;
    current_start = start;
    /* AT(lma), with `.` at the section's aligned start (`AT(. - OFFSET)` is
     * LMA = VMA - OFFSET). It sets the offset the sections after this one
     * keep. */
    int at = 0;
    if (o->at != NULL) {
        i64 v;
        char *err;
        if (eval(o->at, start, &v, &err)) {
            *lma_offset = i64_sub(v, start);
            at = 1;
        } else {
            script_err(o->line, err);
        }
    }
    i64 lma = i64_add(start, *lma_offset);
    i64 d = start;
    struct output_section os;
    memset(&os, 0, sizeof os);
    uint flags = SHF_ALLOC;
    int nobits = 1;
    for (uint it = 0; it < o->nitems; it++) {
        if (o->items[it].is_assign) {
            d = assign(&o->items[it].assign, d, 1);
            continue;
        }
        for (uint k = 0; k < r[it].n; k++) {
            struct section *sec = &L->objects[r[it].in[k].obj].sections[r[it].in[k].sec];
            d = align_pow2(d, i64_u(sec->align));
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
    /* d may be exactly 4 GiB, but the size must fit in 32 bits; ld.lld's
     * words. The link fails, but as in ld.lld the location counter goes on
     * from the section's end, so the sections after it are placed, and
     * reported, where ld.lld places them. */
    i64 size = i64_sub(d, start);
    if (size.hi != 0 || i64_cmp(d, four_gib()) > 0) {
        script_err(o->line, fmt("section %s at %s of size %s exceeds available address space", o->name,
                                i64_hex(start), i64_hex(size)));
        *dot = d;
        if (has_cursor) {
            *cursor = d;
        }
        return;
    }
    /* The load address range must fit as the address range does. The
     * section is still placed: its addresses are right, and the sections
     * after it go where they should. */
    if (!load_range_fits(lma, size.lo)) {
        script_err(o->line, fmt("section %s load address range extends past the 4 GiB address space", o->name));
    }
    if (os.ninputs == 0) {
        /* Only assignments: like ld.lld, PROGBITS with the previous flags,
         * but only SHF_ALLOC and SHF_WRITE of them (ld.lld's
         * adjustOutputSections leaves SHF_EXECINSTR out, so that such a
         * section after the code goes with the data that follows it). */
        flags = *prev_flags & (SHF_ALLOC | SHF_WRITE);
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
    os.lma = lma;
    os.at = at;
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
    /* LMA - VMA (modulo 2^64): 0 until a section has an AT; every section
     * after it without one keeps its difference. */
    i64 lma_offset = i64_u(0);
    for (uint c = 0; c < S.ncommands; c++) {
        struct command *cmd = &S.commands[c];
        has_current = 0;
        if (!cmd->is_output) {
            i64 before = dot;
            dot = assign(&cmd->assign, dot, 0);
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
            place_output(c, &cmd->out, &dot, S.has_memory, &cursor, &dot_moved, &prev_flags, &lma_offset);
        }
    }
    has_current = 0;
}

/* ld.lld's checkOverlap: the sections in `order` (n of them) sorted by
 * where their range starts (the load address, or the address), each
 * compared with the next one, the lower one named first, with ld.lld's
 * ranges on the same line. The sort is stable: sections that start together
 * stay in script order. Every range is below 4 GiB. */
static void overlaps(const char *what, uint *order, uint n, int by_lma) {
    for (uint i = 1; i < n; i++) {
        uint k = order[i];
        uint j = i;
        while (j > 0 && (by_lma ? L->sections[order[j - 1]].lma.lo > L->sections[k].lma.lo
                                : L->sections[order[j - 1]].addr > L->sections[k].addr)) {
            order[j] = order[j - 1];
            j--;
        }
        order[j] = k;
    }
    for (uint i = 0; i + 1 < n; i++) {
        struct output_section *a = &L->sections[order[i]];
        struct output_section *b = &L->sections[order[i + 1]];
        i64 as = by_lma ? a->lma : i64_u(a->addr);
        i64 bs = by_lma ? b->lma : i64_u(b->addr);
        if (i64_cmp(bs, i64_add(as, i64_u(a->size))) < 0) {
            error(fmt("section %s %s range overlaps with %s (%s is [0x%x, 0x%x], %s is [0x%x, 0x%x])", a->name, what,
                      b->name, a->name, as.lo, as.lo + a->size - 1, b->name, bs.lo, bs.lo + b->size - 1));
        }
    }
}

/* Sections over each other in memory (ld.lld's "virtual address range
 * overlaps"): a top-level `.` may move backward, so two sections can be
 * given the same addresses. Every section with a size counts, NOBITS ones
 * too. Then sections loaded over each other ("load address range
 * overlaps"): with AT, two sections can be given load addresses that
 * overlap although their addresses do not. Only sections with bytes in the
 * file count there. */
static void address_and_load_overlaps(void) {
    uint *order = xcalloc((L->nsections + 1) * sizeof(uint));
    uint n = 0;
    for (uint k = 0; k < L->nsections; k++) {
        if (L->sections[k].size > 0) {
            order[n++] = k;
        }
    }
    overlaps("virtual address", order, n, 0);
    n = 0;
    for (uint k = 0; k < L->nsections; k++) {
        struct output_section *s = &L->sections[k];
        /* (One whose load range does not fit has been reported already.) */
        if (!s->nobits && s->size > 0 && load_range_fits(s->lma, s->size)) {
            order[n++] = k;
        }
    }
    overlaps("load address", order, n, 1);
    free(order);
}

/* Whether ld.lld would write a .riscv.attributes section: it merges every
 * input SHT_RISCV_ATTRIBUTES section (clang and merlin-as give each object
 * one) into one section of that name, which the script places by its name:
 * the first output section whose pattern matches .riscv.attributes gets it,
 * and only a /DISCARD/ there throws it away. */
static int riscv_attributes_kept(void) {
    int any = 0;
    for (uint oi = 0; oi < L->nobjects; oi++) {
        for (uint si = 0; si < L->objects[oi].nsections; si++) {
            any |= L->objects[oi].sections[si].type == SHT_RISCV_ATTRIBUTES;
        }
    }
    for (uint c = 0; c < S.ncommands; c++) {
        struct command *cmd = &S.commands[c];
        if (!cmd->is_output) {
            continue;
        }
        for (uint it = 0; it < cmd->out.nitems; it++) {
            if (!cmd->out.items[it].is_assign && pattern_matches(&cmd->out.items[it], ".riscv.attributes")) {
                return any && strcmp(cmd->out.name, DISCARD_NAME) != 0;
            }
        }
    }
    return any;
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
    address_and_load_overlaps();
    l->memory = S.has_memory;
    l->riscv_attributes = riscv_attributes_kept();
    place_debug();
    merge_warnings();
    undefined_references();
    entry_point(entry_opt);
    /* What the program headers would be (write_check). */
    if (errors.n == before) {
        write_check(l);
    }
    return errors.n == before;
}
