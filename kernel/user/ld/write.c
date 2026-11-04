/* ld: the linked image as an ELF32 little-endian executable (merlin-ld's
 * write.rs, which explains each rule at length):
 *
 *   ELF header (52 bytes)
 *   program headers (32 bytes each): PT_PHDR if the headers are loaded, one
 *     PT_LOAD per segment, in the room ld.lld leaves for its own
 *   segment contents, each at a file offset congruent to its address mod its p_align
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
 * bytes. A segment of only empty sections is dropped. A section with an AT
 * of its own starts a segment, and a segment's p_paddr is its first
 * section's load address. Its p_align is the page, 4 KiB, or the largest
 * alignment of its sections if that is more (ld.lld's PhdrEntry::add).
 *
 * The headers, as ld.lld places them for a script without PHDRS:
 *
 *   1. ld.lld sizes its program header table for every header it makes
 *      before it knows which it keeps: PT_PHDR, a PT_LOAD for the headers,
 *      one PT_LOAD per run of sections (an empty one too), PT_GNU_STACK, and
 *      PT_RISCV_ATTRIBUTES if it writes a .riscv.attributes section. A
 *      read-only first section in no MEMORY region joins the headers'
 *      PT_LOAD: one PT_LOAD fewer.
 *   2. The headers are loaded if that room fits between the start of the
 *      lowest section's page and the section: at that page, at file offset
 *      0, in a read-only PT_LOAD of their own or in the first section's
 *      (then moved down to the page), and a PT_PHDR covers the room after
 *      the ELF header.
 *   3. Either way the segment contents come after the room.
 *
 * merlin-ld, and so this, writes only the PT_PHDR and the PT_LOADs, with
 * zeros for the rest of the room, so that every segment is where ld.lld
 * puts it. */
#include "ld.h"

#define PAGE 0x1000

struct segment {
    uint vaddr;
    uint paddr; /* the load address: its first section's */
    uint filesz;
    uint memsz;
    uint flags;
    uint align; /* p_align: its file offset is congruent to vaddr modulo this */
    uint first; /* the sections [first, first + n) of the layout */
    uint n;
    /* It loads the ELF header and the program headers: it starts at the
     * headers' page, at file offset 0. */
    int headers;
};

/* The room the ELF header and the program header table take at the start
 * of the file, and where they are loaded, if they are. */
struct headers {
    uint size;
    int loaded;
    uint addr;
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

/* Groups the output sections into runs, the PT_LOADs they need, empty ones
 * included. Returns how many. */
static uint runs(struct layout *l, struct segment *segs) {
    uint n = 0;
    for (uint i = 0; i < l->nsections; i++) {
        struct output_section *s = &l->sections[i];
        uint flags = pflags(s->flags);
        int start_new = n == 0;
        if (!start_new) {
            struct output_section *prev = &l->sections[segs[n - 1].first + segs[n - 1].n - 1];
            i64 prev_end = i64_add(i64_u(prev->addr), i64_u(prev->size));
            /* Going backward can't be one segment. A section with its own
             * load address starts one: a segment has one p_paddr, and its
             * sections are loaded contiguously from there. */
            start_new = segs[n - 1].flags != flags || i64_cmp(i64_u(s->addr), prev_end) < 0 || s->at;
        }
        if (start_new) {
            memset(&segs[n], 0, sizeof segs[n]);
            segs[n].vaddr = s->addr;
            /* Below 4 GiB: the layout checks every load range. */
            segs[n].paddr = s->lma.lo;
            segs[n].flags = flags;
            segs[n].align = PAGE;
            segs[n].first = i;
            n++;
        }
        struct segment *seg = &segs[n - 1];
        seg->n++;
        /* Every section counts, an empty or NOBITS one too (ld.lld). */
        seg->align = s->align > seg->align ? s->align : seg->align;
        uint end = s->addr - seg->vaddr + s->size;
        seg->memsz = end;
        if (!s->nobits) {
            seg->filesz = end;
        }
    }
    return n;
}

/* The PT_LOAD segments into `out` (room for one more than the layout has
 * sections), the headers' one first if they are loaded, and where the
 * headers go into *h. Returns how many segments. */
static uint segments(struct layout *l, struct segment *out, struct headers *h) {
    /* The runs go after a free first entry, where the headers' own segment
     * may go. */
    struct segment *segs = out + 1;
    uint n = runs(l, segs);

    /* 1. ld.lld's program header table, sized for every header it makes:
     * PT_PHDR, the headers' PT_LOAD, one PT_LOAD per run (an empty one
     * too), PT_GNU_STACK and PT_RISCV_ATTRIBUTES. A read-only first section
     * in no MEMORY region goes into the headers' PT_LOAD instead (an AT on
     * it does not keep it out). */
    int joins = !l->memory && n > 0 && segs[0].flags == PF_R;
    uint count = 2 + n - (uint)joins + 1 + (uint)(l->riscv_attributes != 0);
    h->size = 52 + 32 * count;

    /* 2. Are they loaded? Only if they fit between the start of the page of
     * the lowest section and that section ("only allocate program headers
     * if that would not add a page"). Without sections ld.lld's lowest
     * address is the top of the address space. */
    uint lowest = 0xffffffffu;
    for (uint i = 0; i < l->nsections; i++) {
        lowest = l->sections[i].addr < lowest ? l->sections[i].addr : lowest;
    }
    uint page = lowest - lowest % PAGE;
    h->loaded = h->size <= lowest - page;
    h->addr = page;
    struct segment *first = segs;
    uint total = n;
    if (h->loaded) {
        if (joins) {
            /* The first segment, moved down to the page: the headers, the
             * gap up to its first section, its sections. Its file size runs
             * to its last section with file contents, or to the end of the
             * headers if none has any; its load address moves down too. */
            uint below = segs[0].vaddr - page;
            int file = 0;
            for (uint i = segs[0].first; i < segs[0].first + segs[0].n; i++) {
                file |= !l->sections[i].nobits;
            }
            segs[0].vaddr = page;
            segs[0].paddr -= below;
            segs[0].filesz = file ? below + segs[0].filesz : h->size;
            segs[0].memsz += below;
            segs[0].headers = 1;
        } else {
            /* A read-only segment of the headers alone, at their address. */
            first = out;
            total = n + 1;
            memset(&out[0], 0, sizeof out[0]);
            out[0].vaddr = page;
            out[0].paddr = page;
            out[0].filesz = h->size;
            out[0].memsz = h->size;
            out[0].flags = PF_R;
            out[0].align = PAGE;
            out[0].headers = 1;
        }
    }
    /* E.g. an empty .rodata between .text and .data (ld.lld removes empty
     * PT_LOADs too, after it has sized the table). The headers' segment is
     * never empty. Moved down to out[0..], in order. */
    uint kept = 0;
    for (uint i = 0; i < total; i++) {
        if (first[i].memsz > 0) {
            out[kept++] = first[i];
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
    struct segment *segs = xcalloc((l->nsections + 2) * sizeof(struct segment));
    struct headers hd;
    uint nsegs = segments(l, segs, &hd);
    uint phoff = 52;
    memset(out, 0, sizeof *out);
    /* The room ld.lld leaves for the headers, which holds ours. */
    buf_pad(out, hd.size);

    /* Segment contents. */
    uint *sec_offset = xcalloc((l->nsections + 1) * sizeof(uint));
    uint *seg_offset = xcalloc((nsegs + 1) * sizeof(uint));
    for (uint k = 0; k < nsegs; k++) {
        uint off = 0;
        if (!segs[k].headers) {
            /* The smallest offset >= the end with offset = vaddr (mod
             * p_align). The headers' segment maps the start of the file;
             * its sections, if any, are past the room. */
            uint cur = out->len, a = segs[k].align;
            off = cur + (segs[k].vaddr % a + a - cur % a) % a;
        }
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
    /* Program headers. With the headers loaded, a PT_PHDR first: the table
     * in the headers' segment (the first one), all of the room after the
     * ELF header, at the address and load address it has there. */
    uint nphdrs = 0;
    if (hd.loaded) {
        uint ph[8];
        ph[0] = PT_PHDR;
        ph[1] = phoff;
        ph[2] = hd.addr + phoff;
        ph[3] = segs[0].paddr + phoff;
        ph[4] = hd.size - phoff;
        ph[5] = hd.size - phoff;
        ph[6] = PF_R;
        ph[7] = 4; /* the size of an address */
        for (uint w = 0; w < 8; w++) {
            put_word(out, phoff + 4 * w, ph[w]);
        }
        nphdrs++;
    }
    for (uint k = 0; k < nsegs; k++) {
        uint at = phoff + 32 * nphdrs;
        uint ph[8];
        ph[0] = PT_LOAD;
        ph[1] = seg_offset[k];
        ph[2] = segs[k].vaddr;
        ph[3] = segs[k].paddr;
        ph[4] = segs[k].filesz;
        ph[5] = segs[k].memsz;
        ph[6] = segs[k].flags;
        ph[7] = segs[k].align;
        for (uint w = 0; w < 8; w++) {
            put_word(out, at + 4 * w, ph[w]);
        }
        nphdrs++;
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
    e[3] = nphdrs == 0 ? 0 : phoff;
    e[4] = shoff;
    e[5] = 0;                   /* e_flags: ilp32 soft-float, no RVC */
    e[6] = 52 | 32 << 16;       /* e_ehsize, e_phentsize */
    e[7] = nphdrs | 40 << 16;   /* e_phnum, e_shentsize */
    e[8] = nshdrs | (nshdrs - 1) << 16; /* e_shnum, e_shstrndx */
    for (uint w = 0; w < 9; w++) {
        put_word(out, 16 + 4 * w, e[w]);
    }
}
