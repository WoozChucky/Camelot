/* ld: the linked image as an ELF32 little-endian executable (merlin-ld's
 * write.rs):
 *
 *   ELF header (52 bytes)
 *   program headers, one PT_LOAD per segment
 *   segment contents, each at a file offset congruent to its address mod 4 KiB
 *   .symtab, .strtab, the debug sections (.debug_line, ...), .shstrtab
 *   section headers: null, output sections, .symtab, .strtab, debug sections, .shstrtab
 *
 * The debug sections come after everything but the section names, so that
 * leaving them out (-S) gives exactly the file a link without them writes,
 * which is what merlin-ld's strip_debug makes of the full one.
 *
 * Segments group consecutive output sections with the same permissions (R,
 * RX, RW), as ld.lld does with a SECTIONS command. A segment does not split
 * at address gaps or after a NOBITS section: the file holds zeros for those
 * bytes. A segment of only empty sections is dropped. */
#include "ld.h"

#define PAGE 0x1000
#define PT_LOAD 1
#define PF_X 1
#define PF_W 2
#define PF_R 4

struct segment {
    uint vaddr;
    uint filesz;
    uint memsz;
    uint flags;
    uint first; /* the sections [first, first + n) of the layout */
    uint n;
};

static uint pflags(uint shflags) {
    uint f = PF_R;
    if (shflags & SHF_WRITE) {
        f |= PF_W;
    }
    if (shflags & SHF_EXECINSTR) {
        f |= PF_X;
    }
    return f;
}

/* Groups the output sections into PT_LOAD segments. Returns how many. */
static uint segments(struct layout *l, struct segment *segs) {
    uint n = 0;
    for (uint i = 0; i < l->nsections; i++) {
        struct output_section *s = &l->sections[i];
        uint flags = pflags(s->flags);
        int start_new = n == 0;
        if (!start_new) {
            struct output_section *prev = &l->sections[segs[n - 1].first + segs[n - 1].n - 1];
            i64 prev_end = i64_add(i64_u(prev->addr), i64_u(prev->size));
            /* Going backward can't be one segment. */
            start_new = segs[n - 1].flags != flags || i64_cmp(i64_u(s->addr), prev_end) < 0;
        }
        if (start_new) {
            memset(&segs[n], 0, sizeof segs[n]);
            segs[n].vaddr = s->addr;
            segs[n].flags = flags;
            segs[n].first = i;
            n++;
        }
        struct segment *seg = &segs[n - 1];
        seg->n++;
        uint end = s->addr - seg->vaddr + s->size;
        seg->memsz = end;
        if (!s->nobits) {
            seg->filesz = end;
        }
    }
    /* E.g. an empty .rodata between .text and .data. */
    uint kept = 0;
    for (uint i = 0; i < n; i++) {
        if (segs[i].memsz > 0) {
            segs[kept++] = segs[i];
        }
    }
    return kept;
}

/* A string table: names appended as they come (0 for the empty one). */
static uint str_add(struct buf *t, const char *s) {
    if (s[0] == 0) {
        return 0;
    }
    uint off = t->len;
    buf_put(t, s, (uint)strlen(s) + 1);
    return off;
}

/* The section index a symbol's section has in the executable: SHN_ABS for
 * an absolute one; 0 if its section is not loaded. */
static int shndx_of(struct layout *l, uint obj, uint sec, uint *out) {
    if (sec == SHN_ABS) {
        *out = SHN_ABS;
        return 1;
    }
    struct object *o = &l->objects[obj];
    if (sec >= o->nsections || o->sections[sec].placed != PLACED) {
        return 0;
    }
    /* Placed but without an output section: only after a layout error. */
    *out = o->sections[sec].out < 0 ? SHN_ABS : (uint)o->sections[sec].out + 1;
    return 1;
}

static void sym_entry(struct buf *out, struct buf *strtab, const char *name, uint value, uint size, uint info,
                      uint shndx) {
    buf_u32(out, str_add(strtab, name));
    buf_u32(out, value);
    buf_u32(out, size);
    buf_byte(out, info);
    buf_byte(out, 0);
    buf_u16(out, shndx);
}

/* The symbol table: null, locals, then globals. Returns the first global's
 * index. */
static uint symtab(struct layout *l, struct buf *out, struct buf *strtab) {
    for (uint i = 0; i < 16; i++) {
        buf_byte(out, 0);
    }
    for (uint oi = 0; oi < l->nobjects; oi++) {
        struct object *o = &l->objects[oi];
        for (uint si = 1; si < o->nsymbols; si++) {
            struct symbol *s = &o->symbols[si];
            if (!sym_is_local(s) || s->type == STT_SECTION || s->type == STT_FILE || sym_is_undefined(s)) {
                continue;
            }
            uint shndx, value;
            if (!shndx_of(l, oi, s->shndx, &shndx) || !object_symbol(l, oi, si, &value)) {
                continue; /* in a section that isn't loaded (debug info, ...) */
            }
            sym_entry(out, strtab, s->name, value, s->size, (STB_LOCAL << 4) | s->type, shndx);
        }
    }
    uint first_global = out->len / 16;
    for (uint g = 0; g < l->nglobals; g++) {
        struct global *gl = &l->globals[g];
        if (gl->obj < 0) {
            sym_entry(out, strtab, gl->name, gl->value, 0, (STB_GLOBAL << 4) | STT_NOTYPE, SHN_ABS);
            continue;
        }
        struct symbol *s = &l->objects[gl->obj].symbols[gl->sym];
        uint value, shndx;
        /* A global whose section /DISCARD/ threw away is left out. */
        if (!object_symbol(l, (uint)gl->obj, gl->sym, &value)) {
            continue;
        }
        if (!shndx_of(l, (uint)gl->obj, s->shndx, &shndx)) {
            shndx = SHN_ABS;
        }
        sym_entry(out, strtab, gl->name, value, s->size, (s->bind << 4) | s->type, shndx);
    }
    return first_global;
}

static void put_word(struct buf *b, uint at, uint v) {
    b->data[at] = (uchar)v;
    b->data[at + 1] = (uchar)(v >> 8);
    b->data[at + 2] = (uchar)(v >> 16);
    b->data[at + 3] = (uchar)(v >> 24);
}

static void shdr(struct buf *h, uint name, uint type, uint flags, uint addr, uint offset, uint size, uint link,
                 uint info, uint align, uint entsize) {
    buf_u32(h, name);
    buf_u32(h, type);
    buf_u32(h, flags);
    buf_u32(h, addr);
    buf_u32(h, offset);
    buf_u32(h, size);
    buf_u32(h, link);
    buf_u32(h, info);
    buf_u32(h, align);
    buf_u32(h, entsize);
}

void write_elf(struct layout *l, uchar **images, uchar **debug, int strip, struct buf *out) {
    struct segment *segs = xcalloc((l->nsections + 1) * sizeof(struct segment));
    uint nsegs = segments(l, segs);
    uint phoff = 52;
    memset(out, 0, sizeof *out);
    buf_pad(out, phoff + 32 * nsegs);

    /* Segment contents. */
    uint *sec_offset = xcalloc((l->nsections + 1) * sizeof(uint));
    uint *seg_offset = xcalloc((nsegs + 1) * sizeof(uint));
    for (uint k = 0; k < nsegs; k++) {
        /* The smallest offset >= the end with offset = vaddr (mod PAGE). */
        uint cur = out->len;
        uint off = cur + (segs[k].vaddr % PAGE + PAGE - cur % PAGE) % PAGE;
        buf_pad(out, off);
        for (uint i = segs[k].first; i < segs[k].first + segs[k].n; i++) {
            struct output_section *s = &l->sections[i];
            uint o = off + (s->addr - segs[k].vaddr);
            sec_offset[i] = o;
            if (!s->nobits) {
                buf_pad(out, o);
                buf_put(out, images[i], s->size);
            }
        }
        seg_offset[k] = off;
    }
    for (uint k = 0; k < nsegs; k++) {
        uint at = phoff + 32 * k;
        uint ph[8];
        ph[0] = PT_LOAD;
        ph[1] = seg_offset[k];
        ph[2] = segs[k].vaddr;
        ph[3] = segs[k].vaddr;
        ph[4] = segs[k].filesz;
        ph[5] = segs[k].memsz;
        ph[6] = segs[k].flags;
        ph[7] = PAGE;
        for (uint w = 0; w < 8; w++) {
            put_word(out, at + 4 * w, ph[w]);
        }
    }

    /* Symbol and string tables, and the debug sections. */
    struct buf strtab, shstrtab, syms;
    memset(&strtab, 0, sizeof strtab);
    memset(&shstrtab, 0, sizeof shstrtab);
    memset(&syms, 0, sizeof syms);
    buf_byte(&strtab, 0);
    buf_byte(&shstrtab, 0);
    uint first_global = symtab(l, &syms, &strtab);
    buf_pad(out, (out->len + 3) & ~3u);
    uint symtab_off = out->len;
    buf_put(out, syms.data, syms.len);
    uint strtab_off = out->len;
    buf_put(out, strtab.data, strtab.len);
    uint ndebug = strip ? 0 : l->ndebug;
    uint *debug_off = xcalloc((ndebug + 1) * sizeof(uint));
    for (uint k = 0; k < ndebug; k++) {
        uint a = l->debug[k].align;
        buf_pad(out, (out->len + a - 1) / a * a);
        debug_off[k] = out->len;
        buf_put(out, debug[k], l->debug[k].size);
    }

    /* Section headers. */
    struct buf h;
    memset(&h, 0, sizeof h);
    shdr(&h, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
    for (uint i = 0; i < l->nsections; i++) {
        struct output_section *s = &l->sections[i];
        shdr(&h, str_add(&shstrtab, s->name), s->nobits ? SHT_NOBITS : SHT_PROGBITS, s->flags, s->addr,
             sec_offset[i], s->size, 0, 0, s->align, 0);
    }
    uint symtab_index = l->nsections + 1;
    shdr(&h, str_add(&shstrtab, ".symtab"), SHT_SYMTAB, 0, 0, symtab_off, syms.len, symtab_index + 1, first_global, 4,
         16);
    shdr(&h, str_add(&shstrtab, ".strtab"), SHT_STRTAB, 0, 0, strtab_off, strtab.len, 0, 0, 1, 0);
    for (uint k = 0; k < ndebug; k++) {
        struct debug_section *d = &l->debug[k];
        shdr(&h, str_add(&shstrtab, d->name), SHT_PROGBITS, d->flags, 0, debug_off[k], d->size, 0, 0, d->align,
             d->entsize);
    }
    uint shstrtab_name = str_add(&shstrtab, ".shstrtab");
    shdr(&h, shstrtab_name, SHT_STRTAB, 0, 0, out->len, shstrtab.len, 0, 0, 1, 0);
    uint nshdrs = h.len / 40;
    buf_put(out, shstrtab.data, shstrtab.len);
    buf_pad(out, (out->len + 3) & ~3u);
    uint shoff = out->len;
    buf_put(out, h.data, h.len);

    /* The ELF header. */
    static const uchar ident[16] = {0x7f, 'E', 'L', 'F', 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0};
    memcpy(out->data, ident, 16);
    uint e[9];
    e[0] = ET_EXEC | EM_RISCV << 16; /* e_type, e_machine */
    e[1] = 1;             /* e_version */
    e[2] = l->entry;
    e[3] = nsegs == 0 ? 0 : phoff;
    e[4] = shoff;
    e[5] = 0;                   /* e_flags: ilp32 soft-float, no RVC */
    e[6] = 52 | 32 << 16;       /* e_ehsize, e_phentsize */
    e[7] = nsegs | 40 << 16;    /* e_phnum, e_shentsize */
    e[8] = nshdrs | (nshdrs - 1) << 16; /* e_shnum, e_shstrndx */
    for (uint w = 0; w < 9; w++) {
        put_word(out, 16 + 4 * w, e[w]);
    }
}
