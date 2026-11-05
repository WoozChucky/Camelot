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
 * The two classes, as merlin-ld's layout.rs has them: addresses and sizes
 * are uint64_t for both. ELF64 (RV64) has the whole 64-bit address space,
 * and its script arithmetic is modulo 2^64, as ld.lld's; a section may not
 * run past 2^64. ELF32 (RV32, until #159) has 4 GiB: the location counter,
 * script symbols, section ends, the MEMORY region and the entry point must
 * stay within it (a section may end exactly at 4 GiB), and script
 * arithmetic is exact, the values read as i64s, an overflow an error. At
 * the top level of SECTIONS the location counter moves anywhere, backward
 * and round 2^64 on ELF64 too; only inside an output section must it move
 * forward.
 *
 * Load addresses (AT), as merlin-ld's layout.rs explains them: a section
 * runs at its address (VMA) and is loaded at its load address (LMA, its
 * segment's p_paddr), the same unless the script says `AT(lma)`. In AT, `.`
 * is the section's aligned start. The difference LMA - VMA (modulo 2^64)
 * carries on to every later section without an AT, NOLOAD and empty ones
 * too; a dropped section evaluates nothing. A section with an AT starts a
 * segment of its own (write.c). Load ranges must stay within the address
 * space and not overlap. ADDR(.x) is the address of output section .x: one
 * already placed, or the one being placed (in its AT and its assignments).
 *
 * The executable's class is the objects' (library.c checks they all have
 * it, the script's OUTPUT_FORMAT's if it names one). */
#include "ld.h"

/* The link's state while it is laid out. */
static struct layout *L;
static struct script S;
static const char *script_name;
/* The executable is ELF64 (RV64), else ELF32 (RV32). */
static int elf64;
/* The output section being placed and its start, once that is known: what
 * ADDR of it is, in its AT and among its assignments. */
static int has_current;
static const char *current_name;
static uint64_t current_start;

static void script_err(uint line, char *msg) {
    error(fmt("%s:%u: %s", script_name, line, msg));
}

static int is_alloc(struct section *s) {
    return (s->flags & SHF_ALLOC) != 0;
}

/* Whether a is a power of two. */
static int is_pow2(uint64_t a) {
    return a != 0 && (a & (a - 1)) == 0;
}

/* v rounded up to a multiple of a, a power of two (2^63 too): (v + a - 1)
 * & !(a - 1) modulo 2^64, ld.lld's alignToPowerOf2, which the location
 * counter goes through, so an ELF64 address near the top aligns to 0. */
static uint64_t align_pow2(uint64_t v, uint64_t a) {
    uint64_t m = a - 1;
    return (v + m) & ~m;
}

#define FOUR_GIB ((uint64_t)1 << 32)

/* The address of a symbol `value` bytes into a section at `base`. ELF32:
 * addresses are 32 bits and wrap at 4 GiB, as they always have in
 * merlin-ld (a symbol defined far past its section's end). */
static uint64_t symbol_address(uint64_t base, uint64_t value) {
    return elf64 ? base + value : (uint)(base + value);
}

/* A script value as a message shows it in decimal: signed on ELF32, where
 * values are i64s, unsigned on ELF64. */
static char *show(uint64_t v) {
    return elf64 ? fmt("%llu", v) : fmt("%lld", (int64_t)v);
}

/* ---- Symbols ---- */

static struct section *section_of(uint obj, uint sec) {
    struct object *o = &L->objects[obj];
    return sec < o->nsections ? &o->sections[sec] : NULL;
}

/* Where an input section starts: its address if it is loaded, its offset in
 * its output section if it is debug information. */
static int section_base(uint obj, uint sec, uint64_t *out) {
    struct section *s = section_of(obj, sec);
    if (s == NULL || s->placed == NOT_PLACED) {
        return 0;
    }
    *out = s->addr;
    return 1;
}

int object_symbol(struct layout *l, uint obj, uint sym, uint64_t *out) {
    struct symbol *s = &l->objects[obj].symbols[sym];
    if (s->shndx == SHN_UNDEF) {
        return 0;
    }
    if (s->shndx == SHN_ABS) {
        *out = s->value;
        return 1;
    }
    uint64_t base;
    if (!section_base(obj, s->shndx, &base)) {
        return 0;
    }
    *out = symbol_address(base, s->value);
    return 1;
}

static int global_def_value(struct global *g, uint64_t *out) {
    if (g->obj < 0) {
        *out = g->value;
        return 1;
    }
    return object_symbol(L, (uint)g->obj, g->sym, out);
}

int global_value(struct layout *l, const char *name, uint64_t *out) {
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
    uint64_t base;
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

int reloc_symbol(struct layout *l, uint obj, uint sym, uint64_t *out, char **err) {
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

static void add_global(char *name, int obj, uint sym, uint64_t value) {
    map_put(&L->global_index, name, (int)L->nglobals);
    L->globals = grow(L->globals, &capglobals, L->nglobals + 1, sizeof(struct global));
    struct global *g = &L->globals[L->nglobals++];
    g->name = name;
    g->obj = obj;
    g->sym = sym;
    g->value = value;
}

/* ---- The script's expressions ---- */

/* The value of a script expression with the location counter at `dot`: a
 * u64 on both classes. ELF64 reads it as one, a 64-bit address; ELF32 reads
 * its bits as an i64, so `0 - 1` is -1, which the uses then refuse. */
static int eval(struct expr *e, uint64_t dot, uint64_t *out, char **err) {
    uint64_t a, b, v;
    int i;
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
                v = s->shndx == SHN_ABS ? s->value : known ? symbol_address(sec->addr, s->value) : 0;
            }
            if (known) {
                *out = v;
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
         * (on ELF32 a negative value, read as 64 bits, is none but -2^63).
         * Past 2^64 it wraps to 0, as ld.lld's does. */
        if (!is_pow2(a == 0 ? 1 : a)) {
            *err = fmt("ALIGN(%s): alignment must be power of 2", show(a));
            return 0;
        }
        *out = align_pow2(dot, a == 0 ? 1 : a);
        return 1;
    case X_ORIGIN:
    case X_LENGTH:
        if (!S.has_memory || strcmp(S.region_name, e->name) != 0) {
            *err = fmt("unknown memory region '%s'", e->name);
            return 0;
        }
        *out = e->kind == X_ORIGIN ? S.origin : S.length;
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
                *out = L->sections[k].addr;
                return 1;
            }
        }
        for (uint c = 0; c < S.ncommands; c++) {
            struct command *cmd = &S.commands[c];
            if (cmd->is_output && strcmp(cmd->out.name, e->name) == 0) {
                /* /DISCARD/ has no address: 0, as in ld.lld. */
                if (strcmp(e->name, DISCARD_NAME) == 0) {
                    *out = 0;
                    return 1;
                }
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
        /* ELF64 modulo 2^64, ELF32 exact (script_add). */
        if (!script_add(a, b, e->kind == X_SUB, elf64, out)) {
            *err = "arithmetic overflow";
            return 0;
        }
        return 1;
    }
}

/* Runs one assignment, at the top level of SECTIONS or inside an output
 * section (in_section). Returns the new location counter. */
static uint64_t assign(struct assign *a, uint64_t dot, int in_section) {
    uint64_t v;
    char *err;
    if (!eval(a->e, dot, &v, &err)) {
        script_err(a->line, err);
        return dot;
    }
    if (a->symbol == NULL) {
        uint64_t n = v;
        if (a->add && !script_add(dot, v, 0, elf64, &n)) {
            script_err(a->line, "arithmetic overflow");
            return dot;
        }
        /* Inside an output section `.` only moves forward, and as in ld.lld
         * it still takes the new value. ELF32: compared as i64s, so a
         * negative value is a move backward. ELF64: a `. += n` or an ALIGN
         * that wraps past 2^64 comes out below `dot`, backward too. At the
         * top level it moves anywhere, as in ld.lld: sections over each
         * other are an error once all are placed. */
        int backward = elf64 ? n < dot : (int64_t)n < (int64_t)dot;
        if (in_section && backward) {
            script_err(a->line,
                       fmt("cannot move the location counter backward (from 0x%llx to 0x%llx)", dot, n));
            return n;
        }
        /* ELF32: 4 GiB of address space, which `.` may reach exactly; a
         * negative value, read as 64 bits, is past it too. */
        if (!elf64 && n > FOUR_GIB) {
            script_err(a->line, fmt("location counter 0x%llx is past the 4 GiB address space", n));
            return dot;
        }
        return n;
    }
    /* ELF32: a symbol's value is 32 bits (ld.lld would truncate it), read
     * as an i64, so -1 does not fit either. */
    if (!elf64 && v > UINT32_MAX) {
        script_err(a->line, fmt("value 0x%llx of '%s' does not fit in 32 bits", v, a->symbol));
        return dot;
    }
    /* A script assignment overrides an object's definition (as in ld.lld). */
    int i;
    if (map_get(&L->global_index, a->symbol, &i)) {
        L->globals[i].obj = -1;
        L->globals[i].value = v;
    } else {
        add_global(a->symbol, -1, 0, v);
    }
    return dot;
}

/* ---- Warnings about SHF_MERGE sections ---- */

/* An entry of a mergeable section, for finding the duplicates. */
struct merge_entry {
    uint64_t flags;
    uint64_t entsize;
    uint align;
    uchar *p;
    uint len;
};

static uint bytes_hash(struct merge_entry *e) {
    uint h = 2166136261u ^ (uint)e->flags ^ ((uint)e->entsize << 8) ^ (e->align << 16);
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
            uint e = sec->entsize > sec->datalen ? sec->datalen : (uint)sec->entsize;
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

int is_relro(const char *name, uint64_t flags) {
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
static uint section_rank(const char *name, uint64_t flags, int nobits) {
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
    uint64_t flags = SHF_ALLOC;
    uint ninputs = 0;
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

/* Whether a section loaded at `lma` with `size` bytes fits the class's
 * address space, as its address range must: ELF32 within 4 GiB, which
 * ld.lld does not check (it truncates p_paddr to 32 bits); ELF64 below
 * 2^64, which ld.lld does not check either (p_paddr + p_memsz would wrap),
 * and merlin-ld says so rather than write a segment that ends before it
 * starts. */
static int load_range_fits(uint64_t lma, uint64_t size) {
    uint64_t end = lma + size;
    if (end < lma) {
        return 0;
    }
    return elf64 || end <= FOUR_GIB;
}

/* One output section of the walk (merlin-ld's Command::Output arm).
 * *dot, *cursor, *dot_moved, *prev_flags and *lma_offset are the walk's
 * state. */
static void place_output(uint c, struct output_desc *o, uint64_t *dot, int has_cursor, uint64_t *cursor,
                         uint *dot_moved, uint64_t *prev_flags, uint64_t *lma_offset) {
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
        uint64_t n;
        char *err;
        if (eval(o->align, *dot, &n, &err)) {
            /* ld.lld's checkAlignment (0 is 1), then at most 2^31 on both
             * classes: ld.lld keeps a section's alignment in 32 bits on
             * RV64 too. */
            uint64_t a = n == 0 ? 1 : n;
            if (!is_pow2(a)) {
                script_err(o->line,
                           fmt("ALIGN(%s) of section %s: alignment must be power of 2", show(n), o->name));
            } else if (a > 0x80000000u) {
                script_err(o->line, fmt("ALIGN(%s) of section %s: alignment must be at most 2^31 (ld.lld keeps 32 "
                                        "bits of a section's alignment and would ignore this one)",
                                        show(n), o->name));
            } else {
                align = (uint)a > align ? (uint)a : align;
            }
        } else {
            script_err(o->line, err);
        }
    }
    /* Aligned as ld.lld aligns the location counter, modulo 2^64: an ELF64
     * section at 0xffff_ffff_ffff_fff8 that wants 16 goes at 0, and one
     * whose inputs run past 2^64 is reported below, with its size. */
    uint64_t base = has_cursor ? *cursor : *dot;
    uint64_t start = align_pow2(base, align);
    /* From here on ADDR of this section is `start`. */
    has_current = 1;
    current_name = o->name;
    current_start = start;
    /* AT(lma), with `.` at the section's aligned start (`AT(. - OFFSET)` is
     * LMA = VMA - OFFSET). It sets the offset the sections after this one
     * keep. */
    int at = 0;
    if (o->at != NULL) {
        uint64_t v;
        char *err;
        if (eval(o->at, start, &v, &err)) {
            *lma_offset = v - start;
            at = 1;
        } else {
            script_err(o->line, err);
        }
    }
    uint64_t lma = start + *lma_offset;
    uint64_t d = start;
    struct output_section os;
    memset(&os, 0, sizeof os);
    uint64_t flags = SHF_ALLOC;
    int nobits = 1;
    for (uint it = 0; it < o->nitems; it++) {
        if (o->items[it].is_assign) {
            d = assign(&o->items[it].assign, d, 1);
            continue;
        }
        for (uint k = 0; k < r[it].n; k++) {
            struct section *sec = &L->objects[r[it].in[k].obj].sections[r[it].in[k].sec];
            d = align_pow2(d, sec->align);
            sec->placed = PLACED;
            sec->addr = d;
            sec->out = -1;
            os.inputs = grow(os.inputs, &os.capinputs, os.ninputs + 1, sizeof(struct input_ref));
            os.inputs[os.ninputs].object = r[it].in[k].obj;
            os.inputs[os.ninputs].section = r[it].in[k].sec;
            os.inputs[os.ninputs].addr = d;
            os.ninputs++;
            /* Wrapping, as ld.lld's location counter: an ELF64 section that
             * runs past 2^64 is reported once its size is known, below. */
            d += sec->size;
            flags |= sec->flags & (SHF_WRITE | SHF_EXECINSTR);
            nobits &= sec->type == SHT_NOBITS;
        }
    }
    /* The address space, as ld.lld's checkSections sees it, with its words
     * on both classes. ELF32: d may be exactly 4 GiB, but the size must fit
     * in 32 bits. ELF64: the section must end below 2^64. The link fails,
     * but as in ld.lld the location counter goes on from the section's end
     * (wrapped on ELF64), so the sections after it are placed, and
     * reported, where ld.lld places them. */
    uint64_t size = d - start;
    int past = elf64 ? start + size < start : size > UINT32_MAX || d > FOUR_GIB;
    if (past) {
        script_err(o->line, fmt("section %s at 0x%llx of size 0x%llx exceeds available address space", o->name,
                                start, size));
        *dot = d;
        if (has_cursor) {
            *cursor = d;
        }
        return;
    }
    /* The load address range must fit as the address range does. The
     * section is still placed: its addresses are right, and the sections
     * after it go where they should. */
    if (!load_range_fits(lma, size)) {
        script_err(o->line, fmt("section %s load address range extends past %s address space", o->name,
                                elf64 ? "the 64-bit" : "the 4 GiB"));
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
        if (*dot_moved != 0 && *dot != *cursor) {
            script_err(*dot_moved, fmt("'.' is moved before output section %s, which is placed at the next free "
                                       "address of MEMORY region %s instead; move the assignment inside the section",
                                       o->name, S.region_name));
        }
        /* script.c made sure this does not overflow. */
        uint64_t end = S.origin + S.length;
        if (d > end) {
            script_err(o->line, fmt("section %s overflows MEMORY region %s by %llu bytes", o->name, S.region_name,
                                    d - end));
        }
        *cursor = d;
    }
    for (uint k = 0; k < os.ninputs; k++) {
        L->objects[os.inputs[k].object].sections[os.inputs[k].section].out = (int)L->nsections;
    }
    os.name = o->name;
    os.addr = start;
    os.size = size;
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
    uint64_t dot = 0;
    uint64_t cursor = S.origin;
    /* Line of the last top-level `.` move since the previous output section. */
    uint dot_moved = 0;
    /* Flags of the last output section that had input sections. */
    uint64_t prev_flags = SHF_ALLOC;
    /* LMA - VMA (modulo 2^64): 0 until a section has an AT; every section
     * after it without one keeps its difference. */
    uint64_t lma_offset = 0;
    for (uint c = 0; c < S.ncommands; c++) {
        struct command *cmd = &S.commands[c];
        has_current = 0;
        if (!cmd->is_output) {
            uint64_t before = dot;
            dot = assign(&cmd->assign, dot, 0);
            if (cmd->assign.symbol == NULL && dot != before) {
                dot_moved = cmd->assign.line;
            }
        } else if (strcmp(cmd->out.name, DISCARD_NAME) == 0) {
            /* ld.lld ignores a /DISCARD/'s AT and NOLOAD, but its ALIGN
             * goes through checkAlignment. */
            if (cmd->out.align != NULL) {
                uint64_t n;
                char *err;
                if (!eval(cmd->out.align, dot, &n, &err)) {
                    script_err(cmd->out.line, err);
                } else if (!is_pow2(n == 0 ? 1 : n)) {
                    script_err(cmd->out.line,
                               fmt("ALIGN(%s) of section %s: alignment must be power of 2", show(n), DISCARD_NAME));
                }
            }
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
 * stay in script order. Every range ends below 2^64: start + size does not
 * overflow. */
static void overlaps(const char *what, uint *order, uint n, int by_lma) {
    for (uint i = 1; i < n; i++) {
        uint k = order[i];
        uint j = i;
        while (j > 0 && (by_lma ? L->sections[order[j - 1]].lma > L->sections[k].lma
                                : L->sections[order[j - 1]].addr > L->sections[k].addr)) {
            order[j] = order[j - 1];
            j--;
        }
        order[j] = k;
    }
    for (uint i = 0; i + 1 < n; i++) {
        struct output_section *a = &L->sections[order[i]];
        struct output_section *b = &L->sections[order[i + 1]];
        uint64_t as = by_lma ? a->lma : a->addr;
        uint64_t bs = by_lma ? b->lma : b->addr;
        if (bs < as + a->size) {
            error(fmt("section %s %s range overlaps with %s (%s is [0x%llx, 0x%llx], %s is [0x%llx, 0x%llx])",
                      a->name, what, b->name, a->name, as, as + a->size - 1, b->name, bs, bs + b->size - 1));
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
    uint64_t merge = SHF_MERGE | SHF_STRINGS;
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
            /* Debug sections hold file contents: far below 2^64. */
            uint64_t at = align_pow2(out->size, sec->align);
            out->align = sec->align > out->align ? sec->align : out->align;
            out->inputs = grow(out->inputs, &out->capinputs, out->ninputs + 1, sizeof(struct input_ref));
            out->inputs[out->ninputs].object = oi;
            out->inputs[out->ninputs].section = si;
            out->inputs[out->ninputs].addr = at;
            out->ninputs++;
            sec->placed = PLACED_DEBUG;
            sec->addr = at;
            sec->debug_out = (uint)(out - L->debug);
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
    uint64_t n, v;
    if (parse_number(entry, &n, elf64)) {
        /* ELF32: e_entry is 32 bits (ld.lld would truncate it). ELF64
         * takes any address, `-e 0xffffffff80000000` too. */
        if (elf64 || n <= UINT32_MAX) {
            L->entry = n;
        } else {
            error(fmt("entry address %s does not fit in 32 bits", entry));
        }
    } else if (global_value(L, entry, &v)) {
        L->entry = v;
    } else if (explicit) {
        error(fmt("entry symbol '%s' is not defined", entry));
    } else {
        uint64_t fallback = 0;
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
        /* An address as wide as the class's: 8 or 16 hex digits. */
        add_message(&warnings, fmt(elf64 ? "cannot find entry symbol '%s'; defaulting to 0x%016llx"
                                         : "cannot find entry symbol '%s'; defaulting to 0x%08llx",
                                   entry, fallback));
    }
}

int layout_build(struct object *objects, uint nobjects, int class, const char *name, const char *text,
                 const char *entry_opt, struct layout *l) {
    memset(l, 0, sizeof *l);
    L = l;
    l->class = class;
    elf64 = class == FORMAT_ELF64;
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
    /* Read again for the class: main.c read it for ELF64, for ld.lld's
     * errors; an ELF32 link reads it for its exact arithmetic and 4 GiB. */
    if (!script_parse(have_script ? text : DEFAULT_SCRIPT, &S, &line, &msg, elf64)) {
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
