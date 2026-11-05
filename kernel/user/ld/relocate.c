/* ld: the output sections' contents, with every relocation applied
 * (merlin-ld's relocate.rs). The checks that need the object (is the place
 * inside its section, is the symbol defined, is a PCREL_LO12's auipc label
 * in the same section) are here; the arithmetic is reloc.c's. Every problem
 * is collected, so one run reports them all. */
#include "ld.h"

/* The PCREL_HI20 sites by place, and the places of the sites whose symbol
 * has no value (reported once, at the HI20: its PCREL_LO12 says nothing). */
static struct reloc_context ctx;
static struct uintmap failed_hi20;

static void context(struct layout *l) {
    ctx.elf64 = l->class == FORMAT_ELF64;
    for (uint k = 0; k < l->nsections; k++) {
        struct output_section *os = &l->sections[k];
        for (uint j = 0; j < os->ninputs; j++) {
            struct input_ref *in = &os->inputs[j];
            struct section *sec = &l->objects[in->object].sections[in->section];
            for (uint n = 0; n < sec->nrelocs; n++) {
                struct rela *r = &sec->relocs[n];
                if (r->type != R_RISCV_PCREL_HI20) {
                    continue;
                }
                uint64_t place = in->addr + r->offset;
                uint64_t sym;
                char *err;
                if (reloc_symbol(l, in->object, r->sym, &sym, &err)) {
                    ctx.sites = grow(ctx.sites, &ctx.cap, ctx.nsites + 1, sizeof(struct hi_site));
                    ctx.sites[ctx.nsites].sym = sym;
                    ctx.sites[ctx.nsites].addend = r->addend;
                    umap_put(&ctx.by_place, place, (int)ctx.nsites);
                    ctx.nsites++;
                } else {
                    free(err);
                    umap_put(&failed_hi20, place, 1);
                }
            }
        }
    }
}

/* The message for a relocation reloc_apply refused. */
static char *describe(int code, struct reloc_error *e, const char *ty, const char *sym, const char *at,
                      struct section *sec, struct rela *r) {
    switch (code) {
    case RE_UNSUPPORTED:
        return fmt("unsupported relocation %s against '%s' at %s", ty, sym, at);
    case RE_ALIGN_PADDING:
        return fmt("%s at %s: merlin-ld does not delete alignment padding as ld.lld does; assemble with -mno-relax",
                   ty, at);
    case RE_OUT_OF_RANGE:
        return fmt("relocation %s out of range for '%s' at %s: %lld is not in [%lld, %lld]", ty, sym, at, e->value,
                   e->min, e->max);
    case RE_MISALIGNED:
        return fmt("relocation %s against '%s' at %s is misaligned: offset %lld is not a multiple of %lld", ty, sym,
                   at, e->value, e->min);
    case RE_TRUNCATED:
        return fmt("relocation %s against '%s' at %s patches %llu bytes, but section %s ends %llu bytes after it", ty,
                   sym, at, e->need, sec->name, e->have);
    case RE_MISSING_HI20:
        return fmt("%s at %s refers to '%s', which is not the place of an R_RISCV_PCREL_HI20", ty, at, sym);
    default:
        return fmt("%s against '%s' at %s has addend %lld; a %%pcrel_lo takes the offset of its R_RISCV_PCREL_HI20 "
                   "and must not have its own",
                   ty, sym, at, r->addend);
    }
}

/* Where a relocation is, for messages: SECTION+0xOFFSET (made only when one
 * is reported). */
static char *where(struct section *sec, struct rela *r) {
    return fmt("%s+0x%llx", sec->name, r->offset);
}

/* Reports `what` (taken) about relocation `r`: `a.o: TYPE at SECTION+0xOFF what`. */
static void report(struct object *o, struct section *sec, struct rela *r, char *what) {
    char *at = where(sec, r);
    error(fmt("%s: %s at %s %s", o->name, reloc_type_name(r->type), at, what));
    free(at);
    free(what);
}

/* Reports a relocation reloc_apply refused: one error, or two for an odd
 * offset out of range, the range first, as ld.lld reports it. */
static void report_refused(struct object *o, struct section *sec, struct rela *r, int code, struct reloc_error *e) {
    char *at = where(sec, r);
    const char *ty = reloc_type_name(r->type);
    const char *sym = symbol_display_name(o, r->sym);
    char *msg = describe(code == RE_RANGE_AND_ALIGN ? RE_OUT_OF_RANGE : code, e, ty, sym, at, sec, r);
    error(fmt("%s: %s", o->name, msg));
    free(msg);
    if (code == RE_RANGE_AND_ALIGN) {
        e->min = 2;
        msg = describe(RE_MISALIGNED, e, ty, sym, at, sec, r);
        error(fmt("%s: %s", o->name, msg));
        free(msg);
    }
    free(at);
}

/* The psABI puts a PCREL_LO12's symbol, the label on its auipc, in the same
 * section as the relocation: anywhere else is a mistake. */
static char *pcrel_lo12_section_error(struct object *o, uint section, struct rela *r) {
    if (r->type != R_RISCV_PCREL_LO12_I && r->type != R_RISCV_PCREL_LO12_S) {
        return NULL;
    }
    struct symbol *s = &o->symbols[r->sym];
    if (s->shndx == section) {
        return NULL;
    }
    const char *shown = symbol_display_name(o, r->sym);
    if (s->shndx < o->nsections && s->shndx != SHN_UNDEF && s->shndx < SHN_LORESERVE) {
        return fmt("refers to '%s' in section %s, not to the label of an auipc in %s", shown,
                   o->sections[s->shndx].name, o->sections[section].name);
    }
    return fmt("refers to '%s', not to the label of an auipc in %s", shown, o->sections[section].name);
}

/* Relocations of branches, jumps and calls: S + A - P is where control goes. */
static int is_jump(uint type) {
    return type == R_RISCV_BRANCH || type == R_RISCV_JAL || type == R_RISCV_CALL || type == R_RISCV_CALL_PLT;
}

static int is_lo12(uint type) {
    return type == R_RISCV_PCREL_LO12_I || type == R_RISCV_PCREL_LO12_S;
}

static uchar **loaded(struct layout *l) {
    uchar **images = xcalloc((l->nsections + 1) * sizeof(uchar *));
    for (uint k = 0; k < l->nsections; k++) {
        struct output_section *os = &l->sections[k];
        if (os->nobits) {
            continue;
        }
        images[k] = xcalloc((uint)os->size);
        for (uint j = 0; j < os->ninputs; j++) {
            struct section *sec = &l->objects[os->inputs[j].object].sections[os->inputs[j].section];
            memcpy(images[k] + (uint)(os->inputs[j].addr - os->addr), sec->data, sec->datalen);
        }
    }
    context(l);
    for (uint k = 0; k < l->nsections; k++) {
        struct output_section *os = &l->sections[k];
        for (uint j = 0; j < os->ninputs; j++) {
            struct input_ref *in = &os->inputs[j];
            struct object *o = &l->objects[in->object];
            struct section *sec = &o->sections[in->section];
            for (uint n = 0; n < sec->nrelocs; n++) {
                struct rela *r = &sec->relocs[n];
                if (r->type == R_RISCV_NONE || r->type == R_RISCV_RELAX) {
                    /* Hints that patch nothing: merlin-ld never relaxes. */
                    continue;
                }
                if (sec->type == SHT_NOBITS || os->nobits) {
                    report(o, sec, r, xstrdup("patches a NOBITS section"));
                    continue;
                }
                if (r->offset >= sec->size) {
                    report(o, sec, r, fmt("is outside section %s (size 0x%llx)", sec->name, sec->size));
                    continue;
                }
                char *e = pcrel_lo12_section_error(o, in->section, r);
                if (e != NULL) {
                    report(o, sec, r, e);
                    continue;
                }
                uint64_t sym;
                if (!reloc_symbol(l, in->object, r->sym, &sym, &e)) {
                    report(o, sec, r, e);
                    continue;
                }
                int dummy;
                if (is_lo12(r->type) && umap_get(&failed_hi20, sym, &dummy)) {
                    /* Its PCREL_HI20 already reported the symbol. */
                    continue;
                }
                uint64_t place = in->addr + r->offset;
                if (is_jump(r->type) && is_undefined_weak(l, in->object, r->sym)) {
                    /* A jump to a weak function nobody defined: like ld.lld,
                     * aim it at the jump itself. */
                    sym = place;
                }
                /* From the place to the end of this input section. */
                uint start = (uint)(place - os->addr);
                uint end = (uint)(in->addr - os->addr) + sec->datalen;
                struct reloc_error re;
                int code = reloc_apply(r->type, place, sym, r->addend, images[k] + start, end - start, &ctx, &re);
                if (code != RE_OK) {
                    report_refused(o, sec, r, code, &re);
                }
            }
        }
    }
    return images;
}

/* The debug sections: their inputs one after another, relocated. A
 * relocation against a symbol that is not in the executable (code /DISCARD/
 * threw away or not loaded, a weak reference nobody defines) writes ld.lld's
 * tombstone: 0, or 1 in .debug_loc and .debug_ranges, where a pair of zeros
 * ends a DWARF 4 list. */
static uchar **debug(struct layout *l) {
    /* No PCREL_HI20 sites; R_RISCV_64 as in a section not loaded. */
    struct reloc_context none;
    memset(&none, 0, sizeof none);
    none.elf64 = l->class == FORMAT_ELF64;
    none.debug = 1;
    uchar **images = xcalloc((l->ndebug + 1) * sizeof(uchar *));
    for (uint k = 0; k < l->ndebug; k++) {
        struct debug_section *ds = &l->debug[k];
        uint64_t tombstone = strcmp(ds->name, ".debug_loc") == 0 || strcmp(ds->name, ".debug_ranges") == 0;
        images[k] = xcalloc((uint)ds->size);
        for (uint j = 0; j < ds->ninputs; j++) {
            struct input_ref *in = &ds->inputs[j];
            struct object *o = &l->objects[in->object];
            struct section *sec = &o->sections[in->section];
            memcpy(images[k] + (uint)in->addr, sec->data, sec->datalen);
            for (uint n = 0; n < sec->nrelocs; n++) {
                struct rela *r = &sec->relocs[n];
                if (r->type == R_RISCV_NONE || r->type == R_RISCV_RELAX) {
                    continue;
                }
                if (r->offset >= sec->size) {
                    report(o, sec, r, fmt("is outside section %s (size 0x%llx)", sec->name, sec->size));
                    continue;
                }
                uint64_t sym;
                char *e;
                if (in_dropped_section(l, in->object, r->sym)) {
                    /* S = tombstone - A, modulo 2^64, so that S + A, the
                     * value written, is the tombstone for R_RISCV_32 (an
                     * offset) and R_RISCV_64 (an address) alike, whatever
                     * the addend. Any other type (an ADD/SUB half of a
                     * label difference) gets S = 0, as in ld.lld. */
                    sym = 0;
                    if (r->type == R_RISCV_32 || r->type == R_RISCV_64) {
                        sym = tombstone - (uint64_t)r->addend;
                    }
                } else if (!reloc_symbol(l, in->object, r->sym, &sym, &e)) {
                    report(o, sec, r, e);
                    continue;
                }
                uint64_t place = in->addr + r->offset;
                struct reloc_error re;
                int code = reloc_apply(r->type, place, sym, r->addend, images[k] + (uint)place,
                                       in->addr + sec->datalen - place, &none, &re);
                if (code != RE_OK) {
                    report_refused(o, sec, r, code, &re);
                }
            }
        }
    }
    return images;
}

int relocate(struct layout *l, uchar ***sections, uchar ***debug_images) {
    uint before = errors.n;
    *sections = loaded(l);
    *debug_images = debug(l);
    return errors.n == before;
}
