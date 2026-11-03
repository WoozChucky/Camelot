/* as: the ELF32 relocatable object (merlin-as's elf.rs).
 *
 *   ELF header (52 bytes)
 *   section contents (.text, .data, ... then .rela.*, .symtab, .strtab, .shstrtab)
 *   section header table
 *
 * Section headers: null, the sections in the order the source named them,
 * .riscv.attributes, a .rela.X per section with relocations, .symtab, .strtab,
 * .shstrtab. Symbols: null, the locals, then the globals, each in the order
 * they were first named. */
#include "as.h"
#include "tables.h"

#define SHT_PROGBITS 1
#define SHT_SYMTAB 2
#define SHT_STRTAB 3
#define SHT_RELA 4
#define SHT_NOBITS 8
#define SHT_RISCV_ATTRIBUTES 0x70000003u
#define SHF_WRITE 1
#define SHF_ALLOC 2
#define SHF_EXECINSTR 4
#define SHF_MERGE 0x10
#define SHF_STRINGS 0x20
#define SHF_INFO_LINK 0x40
#define SHN_UNDEF 0
#define SHN_ABS 0xfff1
#define EM_RISCV 243

/* A string table: no sharing, "" is offset 0, as merlin-as builds them. */
static uint strtab_add(struct buf *t, const char *s) {
    if (s[0] == 0) {
        return 0;
    }
    uint off = t->len;
    buf_put(t, s, (uint)strlen(s) + 1);
    return off;
}

struct shdr {
    uint name, ty, flags, offset, size, link, info, align, entsize;
};

static struct shdr *shdrs;
static uint nshdrs, capshdrs;

static struct shdr *new_shdr(void) {
    shdrs = grow(shdrs, &capshdrs, nshdrs + 1, sizeof(struct shdr));
    struct shdr *h = &shdrs[nshdrs++];
    memset(h, 0, sizeof *h);
    return h;
}

/* .riscv.attributes: `A`, then the vendor subsection "riscv" holding one
 * Tag_File subsubsection with Tag_RISCV_arch = RISCV_ARCH. Lengths count
 * themselves. */
static void riscv_attributes(struct buf *out) {
    uint arch = (uint)strlen(RISCV_ARCH);
    uint file_len = 1 + 4 + 1 + arch + 1;
    buf_byte(out, 'A');
    buf_u32(out, 4 + 6 + file_len);
    buf_put(out, "riscv", 6);
    buf_byte(out, 1); /* Tag_File */
    buf_u32(out, file_len);
    buf_byte(out, 5); /* Tag_RISCV_arch */
    buf_put(out, RISCV_ARCH, arch + 1);
}

static int is_global(struct symbol *s) {
    /* Undefined symbols are always global. */
    return s->global || (!s->defined && !s->has_constant);
}

void elf_write(struct object *obj, struct buf *out) {
    struct buf shstr, strtab, symtab;
    memset(&shstr, 0, sizeof shstr);
    memset(&strtab, 0, sizeof strtab);
    memset(&symtab, 0, sizeof symtab);
    buf_byte(&shstr, 0);
    buf_byte(&strtab, 0);
    buf_zeros(out, 52); /* the header, filled in at the end */
    new_shdr();

    for (uint i = 0; i < obj->nsections; i++) {
        struct section *s = &obj->sections[i];
        uint ty = SHT_PROGBITS, flags;
        if (s->kind == SEC_TEXT) {
            flags = SHF_ALLOC | SHF_EXECINSTR;
        } else if (s->kind == SEC_DATA) {
            flags = SHF_ALLOC | SHF_WRITE;
        } else if (s->kind == SEC_RODATA) {
            flags = SHF_ALLOC;
        } else if (s->kind == SEC_BSS) {
            ty = SHT_NOBITS;
            flags = SHF_ALLOC | SHF_WRITE;
        } else if (strcmp(s->name, ".debug_str") == 0 || strcmp(s->name, ".debug_line_str") == 0) {
            flags = SHF_MERGE | SHF_STRINGS;
        } else {
            flags = 0;
        }
        uint align = s->align ? s->align : 1;
        buf_align(out, align);
        uint offset = out->len;
        buf_put(out, s->data.data, s->data.len);
        struct shdr *h = new_shdr();
        h->name = strtab_add(&shstr, s->name);
        h->ty = ty;
        h->flags = flags;
        h->offset = offset;
        h->size = s->size;
        h->align = align;
        h->entsize = flags & SHF_STRINGS ? 1 : 0;
    }

    struct shdr *attr = new_shdr();
    attr->name = strtab_add(&shstr, ".riscv.attributes");
    attr->ty = SHT_RISCV_ATTRIBUTES;
    attr->offset = out->len;
    riscv_attributes(out);
    attr->size = out->len - attr->offset;
    attr->align = 1;

    /* The symbol table: null, locals, then globals. Each symbol's index, and
     * the first index of each name, for the relocations. */
    uint *index = calloc(obj->nsymbols + 1, sizeof(uint));
    if (index == NULL) {
        die("out of memory");
    }
    buf_zeros(&symtab, 16);
    uint n = 1;
    uint first_global = 0;
    for (int pass = 0; pass < 2; pass++) {
        if (pass == 1) {
            first_global = n;
        }
        for (uint i = 0; i < obj->nsymbols; i++) {
            struct symbol *s = &obj->symbols[i];
            if (is_global(s) != pass) {
                continue;
            }
            index[i] = n++;
            uint ty = s->ty == SYM_FUNC ? 2 : s->ty == SYM_OBJECT ? 1 : s->ty == SYM_SECTION ? 3 : 0;
            uint value, shndx;
            if (s->has_constant) {
                value = s->constant.lo;
                shndx = SHN_ABS;
            } else if (s->section >= 0) {
                value = s->value;
                shndx = (uint)s->section + 1;
            } else {
                value = 0;
                shndx = SHN_UNDEF;
            }
            /* A section symbol has no name: it stands for its section. */
            buf_u32(&symtab, s->ty == SYM_SECTION ? 0 : strtab_add(&strtab, s->name));
            buf_u32(&symtab, value);
            buf_u32(&symtab, s->size);
            buf_byte(&symtab, (uint)pass << 4 | ty);
            buf_byte(&symtab, 0);
            buf_u16(&symtab, shndx);
        }
    }

    uint with_relocs = 0;
    for (uint i = 0; i < obj->nsections; i++) {
        if (obj->sections[i].nrelocs > 0) {
            with_relocs++;
        }
    }
    uint symtab_index = nshdrs + with_relocs;

    /* A relocation names its symbol: the first one of that name (asm_finish
     * found it). */
    for (uint i = 0; i < obj->nsections; i++) {
        struct section *s = &obj->sections[i];
        if (s->nrelocs == 0) {
            continue;
        }
        buf_align(out, 4);
        uint offset = out->len;
        for (uint k = 0; k < s->nrelocs; k++) {
            struct reloc *r = &s->relocs[k];
            uint sym = index[r->sym];
            buf_u32(out, r->offset);
            buf_u32(out, sym << 8 | (uint)r->kind);
            buf_u32(out, r->addend.lo);
        }
        struct shdr *h = new_shdr();
        h->name = strtab_add(&shstr, fmt(".rela%s", s->name));
        h->ty = SHT_RELA;
        h->flags = SHF_INFO_LINK;
        h->offset = offset;
        h->size = 12 * s->nrelocs;
        h->link = symtab_index;
        h->info = i + 1;
        h->align = 4;
        h->entsize = 12;
    }
    free(index);

    buf_align(out, 4);
    struct shdr *h = new_shdr();
    h->name = strtab_add(&shstr, ".symtab");
    h->ty = SHT_SYMTAB;
    h->offset = out->len;
    h->size = symtab.len;
    h->link = symtab_index + 1;
    h->info = first_global;
    h->align = 4;
    h->entsize = 16;
    buf_put(out, symtab.data, symtab.len);

    h = new_shdr();
    h->name = strtab_add(&shstr, ".strtab");
    h->ty = SHT_STRTAB;
    h->offset = out->len;
    h->size = strtab.len;
    h->align = 1;
    buf_put(out, strtab.data, strtab.len);

    h = new_shdr();
    h->name = strtab_add(&shstr, ".shstrtab");
    h->ty = SHT_STRTAB;
    h->offset = out->len;
    h->size = shstr.len;
    h->align = 1;
    buf_put(out, shstr.data, shstr.len);

    buf_align(out, 4);
    uint shoff = out->len;
    for (uint i = 0; i < nshdrs; i++) {
        struct shdr *s = &shdrs[i];
        uint v[10] = {s->name, s->ty, s->flags, 0, s->offset, s->size, s->link, s->info, s->align, s->entsize};
        for (int k = 0; k < 10; k++) {
            buf_u32(out, v[k]);
        }
    }

    /* The ELF header. */
    struct buf hdr;
    memset(&hdr, 0, sizeof hdr);
    buf_put(&hdr, "\177ELF", 4);
    buf_byte(&hdr, 1); /* ELFCLASS32 */
    buf_byte(&hdr, 1); /* little-endian */
    buf_byte(&hdr, 1); /* version 1 */
    buf_byte(&hdr, 0); /* SysV ABI */
    buf_zeros(&hdr, 8);
    buf_u16(&hdr, 1); /* ET_REL */
    buf_u16(&hdr, EM_RISCV);
    buf_u32(&hdr, 1); /* e_version */
    buf_u32(&hdr, 0); /* e_entry */
    buf_u32(&hdr, 0); /* e_phoff */
    buf_u32(&hdr, shoff);
    buf_u32(&hdr, 0); /* e_flags: soft-float ABI, no RVC */
    buf_u16(&hdr, 52);
    buf_u16(&hdr, 0);
    buf_u16(&hdr, 0);
    buf_u16(&hdr, 40);
    buf_u16(&hdr, nshdrs);
    buf_u16(&hdr, nshdrs - 1);
    memcpy(out->data, hdr.data, 52);
}
