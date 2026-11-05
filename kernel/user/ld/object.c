/* ld: reads little-endian RISC-V relocatable objects (ET_REL), ELF64, the
 * subset clang (-c) and merlin-as write (merlin-ld's object.rs, whose
 * records the elf crate reads). Anything else is refused with a message
 * saying what was found: an ELF32 (RV32) object among them. Sections and
 * symbols keep their ELF indices, so a relocation's symbol index and a
 * symbol's st_shndx are used as they are. */
#include "ld.h"

/* The bytes of an address, offset or size field, and of a section header,
 * a symbol and a relocation (the elf crate's Class::Elf64). */
#define WORD 8
#define SHDR 64
#define SYM 24
#define RELA 24

/* The file being read, and the message of the last read that failed. */
static uchar *file;
static uint flen;
static char *rd_error;

/* bytes [off, off + len) of the file. The end is computed in 64 bits and
 * must not pass 2^64 either (an ELF64 offset plus size may), as the elf
 * crate's checked_add: then the message's end is the true sum, which
 * passes 64 bits, written as merlin-ld writes a u128. */
static uchar *rd_bytes(uint64_t off, uint64_t len) {
    uint64_t end = off + len;
    if (end < off) {
        /* off + len, past 2^64: 0x1 followed by the 64 bits of the sum. */
        rd_error = fmt("truncated file (needs bytes 0x%llx..0x1%016llx)", off, end);
        return NULL;
    }
    if (end > flen) {
        rd_error = fmt("truncated file (needs bytes 0x%llx..0x%llx)", off, end);
        return NULL;
    }
    return file + (uint)off;
}

static int rd_u8(uint64_t off, uint *v) {
    uchar *p = rd_bytes(off, 1);
    if (p == NULL) {
        return 0;
    }
    *v = p[0];
    return 1;
}

static uint le16(uchar *p) {
    return (uint)p[0] | (uint)p[1] << 8;
}

static uint le32(uchar *p) {
    return (uint)p[0] | (uint)p[1] << 8 | (uint)p[2] << 16 | (uint)p[3] << 24;
}

static uint64_t le64(uchar *p) {
    return (uint64_t)le32(p) | (uint64_t)le32(p + 4) << 32;
}

/* An address, offset or size field: 8 bytes. */
static uint64_t word_at(uchar *p) {
    return le64(p);
}

static int rd_u16(uint64_t off, uint *v) {
    uchar *p = rd_bytes(off, 2);
    if (p == NULL) {
        return 0;
    }
    *v = le16(p);
    return 1;
}

static int rd_u32(uint64_t off, uint *v) {
    uchar *p = rd_bytes(off, 4);
    if (p == NULL) {
        return 0;
    }
    *v = le32(p);
    return 1;
}

/* The string at `off` of a string table (up to a NUL or the table's end). */
static int cstr(uchar *table, uint len, uint off, char **out) {
    if (off > len) {
        rd_error = fmt("string offset 0x%x outside its string table", off);
        return 0;
    }
    for (uint i = off; i < len; i++) {
        if (table[i] == 0) {
            *out = (char *)table + off;
            return 1;
        }
    }
    *out = xstrndup((char *)table + off, len - off);
    return 1;
}

/* A section header, before its contents are interpreted, at ELF64's
 * widths. */
struct shdr {
    uint name;
    uint type;
    uint64_t flags;
    uint64_t offset;
    uint64_t size;
    uint link;
    uint info;
    uint64_t align;
    uint64_t entsize;
};

/* A section's bytes (none for NOBITS). */
static int contents(struct shdr *h, uchar **p) {
    if (h->type == SHT_NOBITS) {
        *p = file;
        return 1;
    }
    *p = rd_bytes(h->offset, h->size);
    return *p != NULL;
}

/* How many bytes contents() gave: all inside the file, so they fit in a
 * uint. */
static uint contents_len(struct shdr *h) {
    return h->type == SHT_NOBITS ? 0 : (uint)h->size;
}

int sym_is_local(struct symbol *s) {
    return s->bind == STB_LOCAL;
}

int sym_is_weak(struct symbol *s) {
    return s->bind == STB_WEAK;
}

int sym_is_undefined(struct symbol *s) {
    return s->shndx == SHN_UNDEF;
}

const char *symbol_display_name(struct object *o, uint index) {
    if (index >= o->nsymbols) {
        return "";
    }
    struct symbol *s = &o->symbols[index];
    if (s->type == STT_SECTION) {
        return s->shndx < o->nsections ? o->sections[s->shndx].name : "";
    }
    return s->name;
}

/* The checks of the ELF header; 0 with *err set. */
static int header(struct object *o, uint64_t *shoff, uint *shentsize, uint *shnum, uint *shstrndx, char **err) {
    uint v;
    if (flen < 4 || file[0] != 0x7f || file[1] != 'E' || file[2] != 'L' || file[3] != 'F') {
        *err = "not an ELF file";
        return 0;
    }
    if (!rd_u8(4, &v)) {
        return 0;
    }
    /* ELF64 only: RV64's objects. An ELF32 one is RV32 code, which no
     * Excalibur hart runs. */
    if (v == 1) {
        *err = "an ELF32 (RV32) object: merlin-ld links RV64 (ELF64) objects only";
        return 0;
    }
    if (v != 2) {
        *err = fmt("not a 64-bit ELF file (EI_CLASS is %u)", v);
        return 0;
    }
    if (!rd_u8(5, &v)) {
        return 0;
    }
    if (v != 1) {
        *err = "not a little-endian ELF file";
        return 0;
    }
    if (!rd_u8(6, &v)) {
        return 0;
    }
    if (v != 1) {
        *err = "unknown ELF version";
        return 0;
    }
    uint e_type;
    if (!rd_u16(16, &e_type)) {
        return 0;
    }
    if (e_type != ET_REL) {
        const char *what = e_type == 2   ? "an executable"
                           : e_type == 3 ? "a shared object"
                           : e_type == 4 ? "a core file"
                                         : "not a relocatable object";
        *err = fmt("%s (e_type %u), expected a relocatable object (ET_REL)", what, e_type);
        return 0;
    }
    if (!rd_u16(18, &v)) {
        return 0;
    }
    if (v != EM_RISCV) {
        *err = fmt("e_machine is %u, not RISC-V (243)", v);
        return 0;
    }
    /* e_entry, e_phoff and e_shoff are address-sized, so e_flags and the
     * fields after it are at 48. */
    uint rest = 24 + 3 * WORD;
    uint flags;
    if (!rd_u32(rest, &flags)) {
        return 0;
    }
    if (flags & 1) {
        *err = "built with compressed instructions (EF_RISCV_RVC); Excalibur has no C extension";
        return 0;
    }
    if (flags != 0) {
        /* clang's lp64 (and merlin-as's objects) have e_flags 0. */
        *err = fmt("e_flags 0x%x not supported: Excalibur objects use the lp64 soft-float ABI (e_flags 0)", flags);
        return 0;
    }
    if (!rd_u16(rest + 10, shentsize) || !rd_u16(rest + 12, shnum) || !rd_u16(rest + 14, shstrndx)) {
        return 0;
    }
    /* Then the elf crate reads the whole header, e_shoff (24 + 2 words in)
     * among it, before the section headers. */
    uchar *p = rd_bytes(24 + 2 * WORD, WORD);
    if (p == NULL) {
        return 0;
    }
    *shoff = word_at(p);
    return 1;
}

static int parse(const char *name, struct object *o, char **err) {
    uint64_t shoff;
    uint shentsize, shnum, shstrndx;
    memset(o, 0, sizeof *o);
    o->name = (char *)name;
    if (!header(o, &shoff, &shentsize, &shnum, &shstrndx, err)) {
        return 0;
    }
    if (shnum == 0) {
        return 1;
    }
    if (shentsize != SHDR) {
        *err = fmt("e_shentsize is %u, expected %u", shentsize, SHDR);
        return 0;
    }
    /* Each section header is read whole, as the elf crate merlin-ld reads
     * objects with does: a file cut inside one needs all of its bytes.
     * sh_flags, sh_addr, sh_offset, sh_size, sh_addralign and sh_entsize
     * are address-sized, sh_name, sh_type, sh_link and sh_info 4 bytes. */
    uint w = WORD;
    struct shdr *hdrs = xcalloc(shnum * sizeof(struct shdr));
    for (uint i = 0; i < shnum; i++) {
        /* Saturating, as the elf crate computes where a header is: one past
         * 2^64 is at the last byte of the address space. */
        uint64_t at = shoff + (uint64_t)i * SHDR;
        if (at < shoff) {
            at = UINT64_MAX;
        }
        uchar *p = rd_bytes(at, SHDR);
        if (p == NULL) {
            return 0;
        }
        struct shdr *s = &hdrs[i];
        uchar *after = p + 8 + 4 * w; /* sh_link */
        s->name = le32(p);
        s->type = le32(p + 4);
        s->flags = word_at(p + 8);
        s->offset = word_at(p + 8 + 2 * w);
        s->size = word_at(p + 8 + 3 * w);
        s->link = le32(after);
        s->info = le32(after + 4);
        s->align = word_at(after + 8);
        s->entsize = word_at(after + 8 + w);
    }
    /* Then every section's name, from the section header string table
     * (SHN_UNDEF, 0, would mean there is none), before any section is
     * looked at. */
    uchar *shstrtab;
    if (shstrndx == 0 || shstrndx >= shnum || hdrs[shstrndx].type != SHT_STRTAB) {
        *err = fmt("e_shstrndx %u is not a string table", shstrndx);
        return 0;
    }
    if (!contents(&hdrs[shstrndx], &shstrtab)) {
        return 0;
    }
    uint shstrlen = contents_len(&hdrs[shstrndx]);
    o->sections = xcalloc(shnum * sizeof(struct section));
    o->nsections = shnum;
    for (uint i = 0; i < shnum; i++) {
        if (!cstr(shstrtab, shstrlen, hdrs[i].name, &o->sections[i].name)) {
            return 0;
        }
    }

    for (uint i = 0; i < shnum; i++) {
        struct shdr *h = &hdrs[i];
        struct section *sec = &o->sections[i];
        char *sname = sec->name;
        if (h->type == SHT_REL) {
            *err = fmt("section '%s' is SHT_REL; RISC-V objects use SHT_RELA", sname);
            return 0;
        }
        if (h->type == SHT_GROUP) {
            *err = fmt("section group '%s' (COMDAT) is not supported", sname);
            return 0;
        }
        if (h->type == SHT_SYMTAB_SHNDX) {
            *err = "extended section indices (SHT_SYMTAB_SHNDX) are not supported";
            return 0;
        }
        if ((h->flags & SHF_ALLOC) && h->type != SHT_PROGBITS && h->type != SHT_NOBITS) {
            *err = fmt("allocated section '%s' has type 0x%x; only PROGBITS and NOBITS are supported", sname,
                       h->type);
            return 0;
        }
        if (h->flags & SHF_TLS) {
            *err = fmt("thread-local section '%s' is not supported", sname);
            return 0;
        }
        if (h->flags & SHF_GROUP) {
            *err = fmt("section '%s' is in a section group (COMDAT), not supported", sname);
            return 0;
        }
        /* The alignment of a section that holds code or data (ld.lld checks
         * no other), a power of two; 0 means 1. Only ELF64's sh_addralign
         * can be too large for 32 bits. */
        uint align = 1;
        if (h->type != SHT_NULL && h->type != SHT_SYMTAB && h->type != SHT_STRTAB && h->type != SHT_RELA) {
            if (h->align > UINT32_MAX) {
                *err = fmt("section '%s' has sh_addralign 0x%llx, too large", sname, h->align);
                return 0;
            }
            align = h->align ? (uint)h->align : 1;
            if (align & (align - 1)) {
                *err = fmt("section '%s' has sh_addralign 0x%x, not a power of 2", sname, align);
                return 0;
            }
        }
        sec->data = file;
        if (i != 0 && !contents(h, &sec->data)) {
            return 0;
        }
        sec->datalen = i == 0 ? 0 : contents_len(h);
        sec->type = h->type;
        sec->flags = h->flags;
        sec->size = h->size;
        sec->align = align;
        sec->entsize = h->entsize;
        sec->out = -1;
    }

    /* The symbol table. */
    int symtab = -1;
    for (uint i = 0; i < shnum; i++) {
        if (hdrs[i].type == SHT_SYMTAB) {
            if (symtab >= 0) {
                *err = "more than one symbol table";
                return 0;
            }
            symtab = (int)i;
        }
    }
    if (symtab >= 0) {
        struct shdr *h = &hdrs[symtab];
        if (h->link >= shnum || hdrs[h->link].type != SHT_STRTAB) {
            *err = "symbol table's sh_link is not a string table";
            return 0;
        }
        uchar *strtab;
        if (!contents(&hdrs[h->link], &strtab)) {
            return 0;
        }
        uint strlen_ = contents_len(&hdrs[h->link]);
        uint size = SYM;
        if (h->entsize != size) {
            *err = fmt("symbol table entry size is %llu, expected %u", h->entsize, size);
            return 0;
        }
        if (h->size % size) {
            *err = fmt("section '%s' has sh_size 0x%llx, not a multiple of its sh_entsize (%u)",
                       o->sections[symtab].name, h->size, size);
            return 0;
        }
        uchar *data;
        if (!contents(h, &data)) {
            return 0;
        }
        /* Every entry is read (its name too) before any is checked, as the
         * elf crate reads them for merlin-ld. An Elf64_Sym is st_name,
         * st_info, st_other, st_shndx, then st_value and st_size. */
        uint n = contents_len(h) / size;
        o->symbols = xcalloc((n ? n : 1) * sizeof(struct symbol));
        for (uint k = 0; k < n; k++) {
            uchar *e = data + size * k;
            struct symbol *sym = &o->symbols[k];
            if (!cstr(strtab, strlen_, le32(e), &sym->name)) {
                return 0;
            }
            uint info = e[4];
            sym->shndx = le16(e + 6);
            sym->value = le64(e + 8);
            sym->size = le64(e + 16);
            sym->bind = info >> 4;
            sym->type = info & 0xf;
        }
        o->nsymbols = n;
        for (uint k = 1; k < n; k++) {
            struct symbol *sym = &o->symbols[k];
            if (sym->shndx == SHN_COMMON) {
                *err = fmt("common symbol '%s' is not supported (define it in .bss, or compile with -fno-common)",
                           sym->name);
                return 0;
            }
            if (sym->type == STT_TLS) {
                *err = fmt("thread-local symbol '%s' is not supported", sym->name);
                return 0;
            }
            if (sym->bind > STB_WEAK) {
                *err = fmt("symbol '%s' has unsupported binding %u", sym->name, sym->bind);
                return 0;
            }
            if (sym->shndx != SHN_ABS && sym->shndx != SHN_UNDEF &&
                (sym->shndx >= SHN_LORESERVE || sym->shndx >= shnum)) {
                *err = fmt("symbol '%s' has unsupported section index 0x%x", sym->name, sym->shndx);
                return 0;
            }
        }
    }

    /* Relocations, attached to the section they patch. */
    for (uint i = 0; i < shnum; i++) {
        struct shdr *h = &hdrs[i];
        if (h->type != SHT_RELA) {
            continue;
        }
        char *rname = o->sections[i].name;
        if (symtab < 0 || h->link != (uint)symtab) {
            *err = fmt("relocation section '%s' does not use the symbol table", rname);
            return 0;
        }
        if (h->info == 0 || h->info >= shnum) {
            *err = fmt("relocation section '%s' patches section index %u", rname, h->info);
            return 0;
        }
        uint size = RELA;
        if (h->entsize != size) {
            *err = fmt("relocation section '%s' has entry size %llu, expected %u", rname, h->entsize, size);
            return 0;
        }
        uchar *data;
        if (!contents(h, &data)) {
            return 0;
        }
        /* r_info is sym << 32 | type; every entry is read before any is
         * checked, as the elf crate reads them. */
        struct section *target = &o->sections[h->info];
        uint n = contents_len(h) / size;
        uint first = target->nrelocs;
        for (uint k = 0; k < n; k++) {
            uchar *e = data + size * k;
            target->relocs = grow(target->relocs, &target->caprelocs, target->nrelocs + 1, sizeof(struct rela));
            struct rela *r = &target->relocs[target->nrelocs++];
            r->offset = le64(e);
            r->type = le32(e + 8);
            r->sym = le32(e + 12);
            r->addend = (int64_t)le64(e + 16);
        }
        for (uint k = first; k < target->nrelocs; k++) {
            if (target->relocs[k].sym >= o->nsymbols) {
                *err = fmt("relocation in '%s' refers to symbol index %u, past the symbol table", rname,
                           target->relocs[k].sym);
                return 0;
            }
        }
    }
    free(hdrs);
    return 1;
}

int object_parse(const char *name, uchar *data, uint len, struct object *out, char **err) {
    file = data;
    flen = len;
    rd_error = NULL;
    *err = NULL;
    if (parse(name, out, err)) {
        return 1;
    }
    if (*err == NULL) {
        *err = rd_error;
    }
    return 0;
}
