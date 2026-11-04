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
 * section's load address; so does the section after a run of RELRO sections
 * (is_relro). Its p_align is the page, 4 KiB, or the largest alignment of
 * its sections if that is more (ld.lld's PhdrEntry::add). A NOBITS section
 * after the first of its segment, and an empty one whose segment is dropped
 * (aligned), are at the end of the file so far (ld.lld's computeFileOffset).
 *
 * The headers, as ld.lld places them for a script without PHDRS:
 *
 *   1. ld.lld sizes its program header table for every header it makes
 *      before it knows which it keeps: PT_PHDR, a PT_LOAD for the headers,
 *      one PT_LOAD per run of sections (an empty one too), a PT_GNU_RELRO
 *      per run of RELRO sections, PT_GNU_STACK, and PT_RISCV_ATTRIBUTES if
 *      it writes a .riscv.attributes section. A
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
             * sections are loaded contiguously from there. So does the
             * section after a run of RELRO sections (ld.lld's relroEnd). */
            start_new = segs[n - 1].flags != flags || i64_cmp(i64_u(s->addr), prev_end) < 0 || s->at ||
                        (is_relro(prev->name, prev->flags) && !is_relro(s->name, s->flags));
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

/* How many runs of consecutive RELRO output sections there are: ld.lld
 * gives each a PT_GNU_RELRO (not written, but counted). */
static uint relro_runs(struct layout *l) {
    uint n = 0;
    for (uint i = 0; i < l->nsections; i++) {
        struct output_section *s = &l->sections[i];
        if (is_relro(s->name, s->flags) && (i == 0 || !is_relro(s[-1].name, s[-1].flags))) {
            n++;
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
     * too), a PT_GNU_RELRO per run of RELRO sections, PT_GNU_STACK and
     * PT_RISCV_ATTRIBUTES. A read-only first section in no MEMORY region
     * goes into the headers' PT_LOAD instead (an AT on it does not keep it
     * out). */
    int joins = !l->memory && n > 0 && segs[0].flags == PF_R;
    uint count = 2 + n - (uint)joins + relro_runs(l) + 1 + (uint)(l->riscv_attributes != 0);
    h->size = 52 + 32 * count;

    /* 2. Are they loaded? Only if they fit between the start of the page of
     * the lowest section and that section ("only allocate program headers
     * if that would not add a page"). Without sections ld.lld's lowest
     * address is the top of the address space. An empty section at
     * exactly 4 GiB (addr_hi) is above every other one; if it is the
     * lowest, it starts its page and nothing fits below it. */
    uint lowest = 0xffffffffu;
    int low = 0, top = 0;
    for (uint i = 0; i < l->nsections; i++) {
        if (l->sections[i].addr_hi != 0) {
            top = 1;
            continue;
        }
        low = 1;
        lowest = l->sections[i].addr < lowest ? l->sections[i].addr : lowest;
    }
    uint page = lowest - lowest % PAGE;
    h->loaded = (low || !top) && h->size <= lowest - page;
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

/* merlin-ld's write::check: ld.lld would give the headers' PT_LOAD a load
 * address range that starts below 0, when the read-only first section
 * shares it but is loaded (AT) less far up than its distance from the page
 * the headers start. */
void write_check(struct layout *l) {
    struct segment *segs = xcalloc((l->nsections + 2) * sizeof(struct segment));
    struct headers hd;
    uint n = segments(l, segs, &hd);
    if (hd.loaded && n > 0 && segs[0].headers && segs[0].n > 0) {
        struct output_section *s = &l->sections[segs[0].first];
        uint below = s->addr - hd.addr;
        if (i64_cmp(s->lma, i64_u(below)) < 0) {
            error(fmt("the headers' load address range would start below 0: they share a segment with section %s, "
                      "loaded at 0x%x, 0x%x bytes after them",
                      s->name, s->lma.lo, below));
        }
    }
    free(segs);
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
 * index. The locals are those ld.lld keeps (merlin-ld's symtab): in a loaded
 * section, or in a debug section (at its offset there; not with -S, which
 * writes the link without them), file symbols too (first in their object's),
 * no section symbol, no `.L` label
 * of a mergeable section; one of .riscv.attributes is absolute, if ld.lld
 * would write that section. */
static uint symtab(struct layout *l, int strip, struct buf *out, struct buf *strtab) {
    for (uint i = 0; i < 16; i++) {
        buf_byte(out, 0);
    }
    for (uint oi = 0; oi < l->nobjects; oi++) {
        struct object *o = &l->objects[oi];
        for (uint si = 1; si < o->nsymbols; si++) {
            struct symbol *s = &o->symbols[si];
            if (!sym_is_local(s) || s->type == STT_SECTION || sym_is_undefined(s)) {
                continue;
            }
            struct section *in = s->shndx < SHN_LORESERVE ? &o->sections[s->shndx] : NULL;
            if (in != NULL && (in->flags & SHF_MERGE) != 0 && strncmp(s->name, ".L", 2) == 0) {
                continue;
            }
            uint info = (STB_LOCAL << 4) | s->type;
            if (in != NULL && in->type == SHT_RISCV_ATTRIBUTES) {
                if (l->riscv_attributes) {
                    sym_entry(out, strtab, s->name, s->value, s->size, info, SHN_ABS);
                }
                continue;
            }
            uint shndx, value;
            if (!shndx_of(l, oi, s->shndx, &shndx)) {
                if (in == NULL || in->placed != PLACED_DEBUG || strip) {
                    continue; /* in a section the executable does not have */
                }
                /* After the loaded sections, .symtab and .strtab. */
                shndx = l->nsections + 3 + in->debug_out;
            }
            if (!object_symbol(l, oi, si, &value)) {
                continue;
            }
            sym_entry(out, strtab, s->name, value, s->size, info, shndx);
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

    /* Segment contents, section by section, at the offsets ld.lld's
     * computeFileOffset gives them, the end of the file so far being its
     * `off`: the first section of a segment at the first offset from there
     * congruent to its address modulo the segment's alignment (the headers'
     * segment maps the start of the file); another NOBITS section at the end
     * so far; another section at the segment's offset plus its distance from
     * the segment's start; a section in no segment (an empty one whose
     * segment was dropped) at the end so far, aligned to its alignment. */
    uint *sec_offset = xcalloc((l->nsections + 1) * sizeof(uint));
    /* The headers' segment, the only one that can be without sections, is
     * at offset 0. */
    uint *seg_offset = xcalloc((nsegs + 1) * sizeof(uint));
    int *seg_of = xcalloc((l->nsections + 1) * sizeof(int));
    for (uint i = 0; i < l->nsections; i++) {
        seg_of[i] = -1;
    }
    for (uint k = 0; k < nsegs; k++) {
        for (uint i = segs[k].first; i < segs[k].first + segs[k].n; i++) {
            seg_of[i] = (int)k;
        }
    }
    for (uint i = 0; i < l->nsections; i++) {
        struct output_section *s = &l->sections[i];
        uint cur = out->len, o;
        int k = seg_of[i];
        if (k >= 0 && segs[k].n > 0 && segs[k].first == i && !segs[k].headers) {
            /* The smallest offset >= the end with offset = vaddr (mod p_align). */
            uint a = segs[k].align;
            seg_offset[k] = cur + (segs[k].vaddr % a + a - cur % a) % a;
            o = seg_offset[k];
        } else if (s->nobits) {
            o = cur;
        } else if (k < 0) {
            o = (cur + s->align - 1) / s->align * s->align;
        } else {
            o = seg_offset[k] + (s->addr - segs[k].vaddr);
        }
        sec_offset[i] = o;
        buf_pad(out, o);
        if (!s->nobits) {
            buf_put(out, images[i], s->size);
        }
    }
    free(seg_of);
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
    uint first_global = symtab(l, strip, &syms, &strtab);
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
