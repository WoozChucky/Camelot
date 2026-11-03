/* ld: reads ELF32 little-endian RISC-V relocatable objects (ET_REL), the
 * subset clang (-c) and merlin-as write (merlin-ld's object.rs). Anything
 * else is refused with a message saying what was found. Sections and
 * symbols keep their ELF indices, so a relocation's symbol index and a
 * symbol's st_shndx are used as they are. */
#include "ld.h"

/* The file being read, and the message of the last read that failed. */
static uchar *file;
static uint flen;
static char *rd_error;

/* bytes [off, off + len) of the file, off as a 64-bit number (offsets are
 * computed from 32-bit fields, and must not wrap around). */
static uchar *rd_bytes(i64 off, uint len) {
    i64 end = i64_add(off, i64_u(len));
    if (off.hi != 0 || end.hi != 0 || end.lo > flen) {
        rd_error = fmt("truncated file (needs bytes %s..%s)", i64_hex(off), i64_hex(end));
        return NULL;
    }
    return file + off.lo;
}

static int rd_u8(i64 off, uint *v) {
    uchar *p = rd_bytes(off, 1);
    if (p == NULL) {
        return 0;
    }
    *v = p[0];
    return 1;
}

static int rd_u16(i64 off, uint *v) {
    uchar *p = rd_bytes(off, 2);
    if (p == NULL) {
        return 0;
    }
    *v = (uint)p[0] | (uint)p[1] << 8;
    return 1;
}

static uint le32(uchar *p) {
    return (uint)p[0] | (uint)p[1] << 8 | (uint)p[2] << 16 | (uint)p[3] << 24;
}

static int rd_u32(i64 off, uint *v) {
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

/* A section header, before its contents are interpreted. */
struct shdr {
    uint name;
    uint type;
    uint flags;
    uint offset;
    uint size;
    uint link;
    uint info;
    uint align;
    uint entsize;
};

/* A section's bytes (none for NOBITS). */
static int contents(struct shdr *h, uchar **p) {
    if (h->type == SHT_NOBITS) {
        *p = file;
        return 1;
    }
    *p = rd_bytes(i64_u(h->offset), h->size);
    return *p != NULL;
}

static uint contents_len(struct shdr *h) {
    return h->type == SHT_NOBITS ? 0 : h->size;
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
static int header(uint *shoff, uint *shentsize, uint *shnum, uint *shstrndx, char **err) {
    uint v;
    if (flen < 4 || file[0] != 0x7f || file[1] != 'E' || file[2] != 'L' || file[3] != 'F') {
        *err = "not an ELF file";
        return 0;
    }
    if (!rd_u8(i64_u(4), &v)) {
        return 0;
    }
    if (v != 1) {
        *err = "not a 32-bit ELF file (EI_CLASS is not ELFCLASS32)";
        return 0;
    }
    if (!rd_u8(i64_u(5), &v)) {
        return 0;
    }
    if (v != 1) {
        *err = "not a little-endian ELF file";
        return 0;
    }
    if (!rd_u8(i64_u(6), &v)) {
        return 0;
    }
    if (v != 1) {
        *err = "unknown ELF version";
        return 0;
    }
    uint e_type;
    if (!rd_u16(i64_u(16), &e_type)) {
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
    if (!rd_u16(i64_u(18), &v)) {
        return 0;
    }
    if (v != EM_RISCV) {
        *err = fmt("e_machine is %u, not RISC-V (243)", v);
        return 0;
    }
    uint flags;
    if (!rd_u32(i64_u(36), &flags)) {
        return 0;
    }
    if (flags & 1) {
        *err = "built with compressed instructions (EF_RISCV_RVC); Excalibur has no C extension";
        return 0;
    }
    if (flags != 0) {
        *err = fmt("e_flags 0x%x not supported: Excalibur objects use the ilp32 soft-float ABI (e_flags 0)", flags);
        return 0;
    }
    return rd_u32(i64_u(32), shoff) && rd_u16(i64_u(46), shentsize) && rd_u16(i64_u(48), shnum) &&
           rd_u16(i64_u(50), shstrndx);
}

static int parse(const char *name, struct object *o, char **err) {
    uint shoff, shentsize, shnum, shstrndx;
    memset(o, 0, sizeof *o);
    o->name = (char *)name;
    if (!header(&shoff, &shentsize, &shnum, &shstrndx, err)) {
        return 0;
    }
    if (shnum == 0) {
        return 1;
    }
    if (shentsize != 40) {
        *err = fmt("e_shentsize is %u, expected 40", shentsize);
        return 0;
    }
    struct shdr *hdrs = xcalloc(shnum * sizeof(struct shdr));
    for (uint i = 0; i < shnum; i++) {
        i64 h = i64_add(i64_u(shoff), i64_u(i * 40));
        struct shdr *s = &hdrs[i];
        if (!rd_u32(h, &s->name) || !rd_u32(i64_add(h, i64_u(4)), &s->type) ||
            !rd_u32(i64_add(h, i64_u(8)), &s->flags) || !rd_u32(i64_add(h, i64_u(16)), &s->offset) ||
            !rd_u32(i64_add(h, i64_u(20)), &s->size) || !rd_u32(i64_add(h, i64_u(24)), &s->link) ||
            !rd_u32(i64_add(h, i64_u(28)), &s->info) || !rd_u32(i64_add(h, i64_u(32)), &s->align) ||
            !rd_u32(i64_add(h, i64_u(36)), &s->entsize)) {
            return 0;
        }
    }
    uchar *shstrtab;
    if (shstrndx >= shnum || hdrs[shstrndx].type != SHT_STRTAB) {
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
        struct shdr *h = &hdrs[i];
        struct section *sec = &o->sections[i];
        char *sname;
        if (!cstr(shstrtab, shstrlen, h->name, &sname)) {
            return 0;
        }
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
        sec->data = file;
        if (i != 0 && !contents(h, &sec->data)) {
            return 0;
        }
        sec->datalen = i == 0 ? 0 : contents_len(h);
        sec->name = sname;
        sec->type = h->type;
        sec->flags = h->flags;
        sec->size = h->size;
        sec->align = h->align ? h->align : 1;
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
        if (h->entsize != 16) {
            *err = fmt("symbol table entry size is %u, expected 16", h->entsize);
            return 0;
        }
        uchar *data;
        if (!contents(h, &data)) {
            return 0;
        }
        uint n = contents_len(h) / 16;
        o->symbols = xcalloc((n ? n : 1) * sizeof(struct symbol));
        for (uint k = 0; k < n; k++) {
            uchar *e = data + 16 * k;
            struct symbol *sym = &o->symbols[k];
            if (!cstr(strtab, strlen_, le32(e), &sym->name)) {
                return 0;
            }
            sym->value = le32(e + 4);
            sym->size = le32(e + 8);
            sym->bind = e[12] >> 4;
            sym->type = e[12] & 0xf;
            sym->shndx = (uint)e[14] | (uint)e[15] << 8;
            o->nsymbols++;
            if (k == 0) {
                continue;
            }
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
        if (h->entsize != 12) {
            *err = fmt("relocation section '%s' has entry size %u, expected 12", rname, h->entsize);
            return 0;
        }
        uchar *data;
        if (!contents(h, &data)) {
            return 0;
        }
        struct section *target = &o->sections[h->info];
        uint n = contents_len(h) / 12;
        for (uint k = 0; k < n; k++) {
            uchar *e = data + 12 * k;
            uint info = le32(e + 4);
            if ((info >> 8) >= o->nsymbols) {
                *err = fmt("relocation in '%s' refers to symbol index %u, past the symbol table", rname, info >> 8);
                return 0;
            }
            target->relocs = grow(target->relocs, &target->caprelocs, target->nrelocs + 1, sizeof(struct rela));
            struct rela *r = &target->relocs[target->nrelocs++];
            r->offset = le32(e);
            r->type = info & 0xff;
            r->sym = info >> 8;
            r->addend = (int)le32(e + 8);
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
