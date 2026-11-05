/* ld: the linked image as a little-endian executable of the layout's class,
 * ELF32 for RV32 and ELF64 for RV64 (merlin-ld's write.rs, which explains
 * each rule at length):
 *
 *   ELF header (52 or 64 bytes)
 *   program headers (32 or 56 bytes each): PT_PHDR if the headers are
 *     loaded, one PT_LOAD per segment, in the room ld.lld leaves for its own
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
 * puts it.
 *
 * The two classes hold the same records; ELF64 widens every address,
 * offset and size to 8 bytes and reorders two records so that those stay
 * aligned: a program header's p_flags follows p_type, a symbol's st_info,
 * st_other and st_shndx come before st_value and st_size. .symtab and the
 * section header table are aligned to the size of an address. The page,
 * 4 KiB, and e_flags, 0, are the same for both. */
#include "ld.h"

#define PAGE 0x1000

/* What differs between the classes for the writer: the bytes of an
 * address, offset or size field (also the alignment of .symtab and the
 * section header table), and of the ELF header, a program header, a
 * section header and a symbol. */
struct class {
    uint ident;
    uint word;
    uint ehdr;
    uint phdr;
    uint shdr;
    uint sym;
};

static const struct class ELF32 = {1, 4, 52, 32, 40, 16};
static const struct class ELF64 = {2, 8, 64, 56, 64, 24};

/* The executable's class. */
static const struct class *class;

static void set_class(struct layout *l) {
    class = l->class == FORMAT_ELF64 ? &ELF64 : &ELF32;
}

struct segment {
    uint64_t vaddr;
    uint64_t paddr; /* the load address: its first section's */
    uint64_t filesz;
    uint64_t memsz;
    uint flags;
    uint64_t align; /* p_align: its file offset is congruent to vaddr modulo this */
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
    uint64_t addr;
};

static uint pflags(uint64_t shflags) {
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
            /* No overflow: every section ends below 2^64 (the layout refuses
             * one that would not). */
            uint64_t prev_end = prev->addr + prev->size;
            /* Going backward can't be one segment. A section with its own
             * load address starts one: a segment has one p_paddr, and its
             * sections are loaded contiguously from there. So does the
             * section after a run of RELRO sections (ld.lld's relroEnd). */
            start_new = segs[n - 1].flags != flags || s->addr < prev_end || s->at ||
                        (is_relro(prev->name, prev->flags) && !is_relro(s->name, s->flags));
        }
        if (start_new) {
            memset(&segs[n], 0, sizeof segs[n]);
            segs[n].vaddr = s->addr;
            segs[n].paddr = s->lma;
            segs[n].flags = flags;
            segs[n].align = PAGE;
            segs[n].first = i;
            n++;
        }
        struct segment *seg = &segs[n - 1];
        seg->n++;
        /* Every section counts, an empty or NOBITS one too (ld.lld). */
        seg->align = s->align > seg->align ? s->align : seg->align;
        uint64_t end = s->addr - seg->vaddr + s->size;
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
    h->size = class->ehdr + class->phdr * count;

    /* 2. Are they loaded? Only if they fit between the start of the page of
     * the lowest section and that section ("only allocate program headers
     * if that would not add a page"). Without sections ld.lld's lowest
     * address is the top of the address space (its `min` starts at
     * UINT64_MAX, which an ELF32 field cuts to 32 bits). */
    uint64_t lowest = class == &ELF64 ? UINT64_MAX : UINT32_MAX;
    for (uint i = 0; i < l->nsections; i++) {
        lowest = l->sections[i].addr < lowest || i == 0 ? l->sections[i].addr : lowest;
    }
    uint64_t page = lowest - lowest % PAGE;
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
            uint64_t below = segs[0].vaddr - page;
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
    set_class(l);
    struct segment *segs = xcalloc((l->nsections + 2) * sizeof(struct segment));
    struct headers hd;
    uint n = segments(l, segs, &hd);
    if (hd.loaded && n > 0 && segs[0].headers && segs[0].n > 0) {
        struct output_section *s = &l->sections[segs[0].first];
        uint64_t below = s->addr - hd.addr;
        if (s->lma < below) {
            error(fmt("the headers' load address range would start below 0: they share a segment with section %s, "
                      "loaded at 0x%llx, 0x%llx bytes after them",
                      s->name, s->lma, below));
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

static void buf_u64(struct buf *b, uint64_t v) {
    buf_u32(b, (uint)v);
    buf_u32(b, (uint)(v >> 32));
}

/* An address, offset or size field: 4 bytes in ELF32, 8 in ELF64. An ELF32
 * value fits in 4 bytes: the layout keeps an ELF32 link below 4 GiB. */
static void buf_word(struct buf *b, uint64_t v) {
    if (class->word == 8) {
        buf_u64(b, v);
    } else {
        buf_u32(b, (uint)v);
    }
}

/* A symbol table entry: Elf32_Sym is name, value, size, info, other,
 * shndx; Elf64_Sym moves the three narrow fields before the two 8-byte
 * ones. */
static void sym_entry(struct buf *out, struct buf *strtab, const char *name, uint64_t value, uint64_t size, uint info,
                      uint shndx) {
    buf_u32(out, str_add(strtab, name));
    if (class->word == 8) {
        buf_byte(out, info);
        buf_byte(out, 0);
        buf_u16(out, shndx);
        buf_u64(out, value);
        buf_u64(out, size);
    } else {
        buf_u32(out, (uint)value);
        buf_u32(out, (uint)size);
        buf_byte(out, info);
        buf_byte(out, 0);
        buf_u16(out, shndx);
    }
}

/* The symbol table: null, locals, then globals. Returns the first global's
 * index. The locals are those ld.lld keeps (merlin-ld's symtab): in a loaded
 * section, or in a debug section (at its offset there; not with -S, which
 * writes the link without them), file symbols too (first in their object's),
 * no section symbol, no `.L` label
 * of a mergeable section; one of .riscv.attributes is absolute, if ld.lld
 * would write that section. */
static uint symtab(struct layout *l, int strip, struct buf *out, struct buf *strtab) {
    for (uint i = 0; i < class->sym; i++) {
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
            uint shndx;
            uint64_t value;
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
    uint first_global = out->len / class->sym;
    for (uint g = 0; g < l->nglobals; g++) {
        struct global *gl = &l->globals[g];
        if (gl->obj < 0) {
            sym_entry(out, strtab, gl->name, gl->value, 0, (STB_GLOBAL << 4) | STT_NOTYPE, SHN_ABS);
            continue;
        }
        struct symbol *s = &l->objects[gl->obj].symbols[gl->sym];
        uint shndx;
        uint64_t value;
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

/* A section header: the same fields in the same order in both classes,
 * sh_flags, sh_addr, sh_offset, sh_size, sh_addralign and sh_entsize
 * address-sized. */
static void shdr(struct buf *h, uint name, uint type, uint64_t flags, uint64_t addr, uint64_t offset, uint64_t size,
                 uint link, uint info, uint64_t align, uint64_t entsize) {
    buf_u32(h, name);
    buf_u32(h, type);
    buf_word(h, flags);
    buf_word(h, addr);
    buf_word(h, offset);
    buf_word(h, size);
    buf_u32(h, link);
    buf_u32(h, info);
    buf_word(h, align);
    buf_word(h, entsize);
}

/* A program header, PT_LOAD or PT_PHDR. ELF64 moves p_flags up to follow
 * p_type, so that the six address-sized fields after it are 8-aligned. */
static void phdr(struct buf *b, uint type, uint flags, uint64_t offset, uint64_t vaddr, uint64_t paddr,
                 uint64_t filesz, uint64_t memsz, uint64_t align) {
    buf_u32(b, type);
    if (class->word == 8) {
        buf_u32(b, flags);
    }
    buf_word(b, offset);
    buf_word(b, vaddr);
    buf_word(b, paddr);
    buf_word(b, filesz);
    buf_word(b, memsz);
    if (class->word == 4) {
        buf_u32(b, flags);
    }
    buf_word(b, align);
}

/* `len` rounded up to a multiple of `a`, in 64 bits as merlin-ld's
 * next_multiple_of (an alignment can be 2^31). */
static uint64_t align_len(uint64_t len, uint a) {
    return (len + a - 1) / a * a;
}

void write_elf(struct layout *l, uchar **images, uchar **debug, int strip, struct buf *out) {
    set_class(l);
    struct segment *segs = xcalloc((l->nsections + 2) * sizeof(struct segment));
    struct headers hd;
    uint nsegs = segments(l, segs, &hd);
    uint phoff = class->ehdr;
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
     * segment was dropped) at the end so far, aligned to its alignment.
     * The offsets are uint64_t, as merlin-ld's are: an ELF32 link stays
     * below 4 GiB, but on ELF64 a section can be 4 GiB or more into its
     * segment. The file is built in memory, though, so the end it is padded
     * to and every section put there must fit a uint (mem_len), or ld stops
     * out of memory, as merlin-ld does when it cannot allocate the file. */
    uint64_t *sec_offset = xcalloc((l->nsections + 1) * sizeof(uint64_t));
    /* The headers' segment, the only one that can be without sections, is
     * at offset 0. */
    uint64_t *seg_offset = xcalloc((nsegs + 1) * sizeof(uint64_t));
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
        uint64_t cur = out->len, o;
        int k = seg_of[i];
        if (k >= 0 && segs[k].n > 0 && segs[k].first == i && !segs[k].headers) {
            /* The smallest offset >= the end with offset = vaddr (mod p_align). */
            uint64_t a = segs[k].align;
            seg_offset[k] = cur + (segs[k].vaddr % a + a - cur % a) % a;
            o = seg_offset[k];
        } else if (s->nobits) {
            o = cur;
        } else if (k < 0) {
            o = align_len(cur, s->align);
        } else {
            o = seg_offset[k] + (s->addr - segs[k].vaddr);
        }
        sec_offset[i] = o;
        buf_pad(out, mem_len(o));
        if (!s->nobits) {
            buf_put(out, images[i], mem_len(s->size));
        }
    }
    free(seg_of);
    /* Program headers. With the headers loaded, a PT_PHDR first: the table
     * in the headers' segment (the first one), all of the room after the
     * ELF header, at the address and load address it has there; its
     * alignment the size of an address. */
    struct buf ph;
    memset(&ph, 0, sizeof ph);
    uint nphdrs = 0;
    if (hd.loaded) {
        uint room = hd.size - phoff;
        phdr(&ph, PT_PHDR, PF_R, phoff, hd.addr + phoff, segs[0].paddr + phoff, room, room, class->word);
        nphdrs++;
    }
    for (uint k = 0; k < nsegs; k++) {
        phdr(&ph, PT_LOAD, segs[k].flags, seg_offset[k], segs[k].vaddr, segs[k].paddr, segs[k].filesz, segs[k].memsz,
             segs[k].align);
        nphdrs++;
    }
    memcpy(out->data + phoff, ph.data, ph.len);
    free(ph.data);

    /* Symbol and string tables, and the debug sections. .symtab is aligned
     * to its entries' widest field, an address. */
    struct buf strtab, shstrtab, syms;
    memset(&strtab, 0, sizeof strtab);
    memset(&shstrtab, 0, sizeof shstrtab);
    memset(&syms, 0, sizeof syms);
    buf_byte(&strtab, 0);
    buf_byte(&shstrtab, 0);
    uint first_global = symtab(l, strip, &syms, &strtab);
    buf_pad(out, mem_len(align_len(out->len, class->word)));
    uint symtab_off = out->len;
    buf_put(out, syms.data, syms.len);
    uint strtab_off = out->len;
    buf_put(out, strtab.data, strtab.len);
    uint ndebug = strip ? 0 : l->ndebug;
    uint *debug_off = xcalloc((ndebug + 1) * sizeof(uint));
    for (uint k = 0; k < ndebug; k++) {
        buf_pad(out, mem_len(align_len(out->len, l->debug[k].align)));
        debug_off[k] = out->len;
        buf_put(out, debug[k], mem_len(l->debug[k].size));
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
    shdr(&h, str_add(&shstrtab, ".symtab"), SHT_SYMTAB, 0, 0, symtab_off, syms.len, symtab_index + 1, first_global,
         class->word, class->sym);
    shdr(&h, str_add(&shstrtab, ".strtab"), SHT_STRTAB, 0, 0, strtab_off, strtab.len, 0, 0, 1, 0);
    for (uint k = 0; k < ndebug; k++) {
        struct debug_section *d = &l->debug[k];
        shdr(&h, str_add(&shstrtab, d->name), SHT_PROGBITS, d->flags, 0, debug_off[k], d->size, 0, 0, d->align,
             d->entsize);
    }
    uint shstrtab_name = str_add(&shstrtab, ".shstrtab");
    shdr(&h, shstrtab_name, SHT_STRTAB, 0, 0, out->len, shstrtab.len, 0, 0, 1, 0);
    uint nshdrs = h.len / class->shdr;
    buf_put(out, shstrtab.data, shstrtab.len);
    buf_pad(out, mem_len(align_len(out->len, class->word)));
    uint shoff = out->len;
    buf_put(out, h.data, h.len);

    /* The ELF header: class, little-endian, version 1, System V ABI. */
    struct buf e;
    memset(&e, 0, sizeof e);
    buf_put(&e, "\177ELF", 4);
    buf_byte(&e, class->ident);
    buf_byte(&e, 1);
    buf_byte(&e, 1);
    buf_byte(&e, 0);
    buf_pad(&e, 16);
    buf_u16(&e, ET_EXEC);
    buf_u16(&e, EM_RISCV);
    buf_u32(&e, 1); /* e_version */
    buf_word(&e, l->entry);
    buf_word(&e, nphdrs == 0 ? 0 : phoff);
    buf_word(&e, shoff);
    buf_u32(&e, 0); /* e_flags: ilp32 or lp64 soft-float, no RVC */
    buf_u16(&e, class->ehdr);
    buf_u16(&e, class->phdr);
    buf_u16(&e, nphdrs);
    buf_u16(&e, class->shdr);
    buf_u16(&e, nshdrs);
    buf_u16(&e, nshdrs - 1); /* e_shstrndx: .shstrtab, last */
    memcpy(out->data, e.data, e.len);
    free(e.data);
}
