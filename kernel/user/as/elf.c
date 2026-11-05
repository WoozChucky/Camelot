/* as: the relocatable object (merlin-as's elf.rs), ELF64.
 *
 *   ELF header (64 bytes)
 *   section contents (.text, .data, ... then .rela.*, .symtab, .strtab, .shstrtab)
 *   section header table
 *
 * Section headers: null, the object's sections (the source's in the order it
 * named them, .riscv.attributes, then the line table's if the source did not
 * make them), a .rela.X per section with relocations, .symtab, .strtab,
 * .shstrtab. Symbols: null, the locals, then the globals, each in the order
 * they were made.
 *
 * The fields that hold an address, an offset or a size are 8 bytes (buf_u64);
 * the counts, indices and types 4 or 2 bytes. Two records are laid out for
 * that: a symbol (Elf64_Sym) puts st_info, st_other and st_shndx before
 * st_value and st_size, and a relocation (Elf64_Rela) packs its symbol and
 * type as sym << 32 | type, with the whole 64-bit addend. The tables of
 * 8-byte fields (.rela.*, .symtab, the section headers) start 8-aligned. */
#include "as.h"
#include "tables.h"

/* The types and flags of the sections a source makes are in tables.h
 * (merlin-as's section.rs); these are the writer's own sections'. */
#define SHT_SYMTAB 2
#define SHT_STRTAB 3
#define SHT_RELA 4
#define SHF_INFO_LINK 0x40
#define SHN_UNDEF 0
#define SHN_ABS 0xfff1
#define EM_RISCV 243
#define ELFCLASS64 2

/* The bytes of an address, offset or size field (also the alignment of the
 * tables made of them), of the ELF header, a section header, a symbol and a
 * relocation (merlin-as's WORD, EHSIZE, SHENTSIZE, SYM and RELA). */
#define WORD 8
#define EHSIZE 64
#define SHENTSIZE 64
#define SYM 24
#define RELA 24

const char *riscv_arch(void) {
    return RISCV_ARCH;
}

static void buf_u64(struct buf *b, uint64_t v) {
    buf_u32(b, (uint)v);
    buf_u32(b, (uint)(v >> 32));
}

/* A string table, as merlin-as builds them: "" is offset 0, and a string
 * already in it (a mapping symbol's name) is not added again. `seen` maps
 * each string to its offset. */
static uint strtab_add(struct buf *t, struct strmap *seen, const char *s) {
    if (s[0] == 0) {
        return 0;
    }
    int at;
    if (map_get(seen, s, &at)) {
        return (uint)at;
    }
    uint off = t->len;
    buf_put(t, s, (uint)strlen(s) + 1);
    map_put(seen, perm_strdup(s), (int)off);
    return off;
}

/* A section header; sh_addr is always 0 in a relocatable file. Every value
 * here fits in 32 bits (section sizes and file offsets are 32-bit), and the
 * address-sized ones are written 8 bytes wide. */
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
 * Tag_File subsubsection with Tag_RISCV_arch = riscv_arch(). Lengths count
 * themselves. */
void riscv_attributes(struct buf *out) {
    const char *name = riscv_arch();
    uint arch = (uint)strlen(name);
    uint file_len = 1 + 4 + 1 + arch + 1;
    buf_byte(out, 'A');
    buf_u32(out, 4 + 6 + file_len);
    buf_put(out, "riscv", 6);
    buf_byte(out, 1); /* Tag_File */
    buf_u32(out, file_len);
    buf_byte(out, 5); /* Tag_RISCV_arch */
    buf_put(out, name, arch + 1);
}

static int is_global(struct symbol *s) {
    /* Undefined symbols are global, but those .local names. */
    return s->global || (!s->defined && !s->has_constant && !s->local);
}

void elf_write(struct object *obj, struct buf *out) {
    struct buf shstr, strtab, symtab;
    struct strmap shstr_seen, strtab_seen;
    memset(&shstr_seen, 0, sizeof shstr_seen);
    memset(&strtab_seen, 0, sizeof strtab_seen);
    memset(&shstr, 0, sizeof shstr);
    memset(&strtab, 0, sizeof strtab);
    memset(&symtab, 0, sizeof symtab);
    buf_byte(&shstr, 0);
    buf_byte(&strtab, 0);
    buf_zeros(out, EHSIZE); /* the header, filled in at the end */
    new_shdr();

    for (uint i = 0; i < obj->nsections; i++) {
        /* The type, flags and entry size the assembler gave it (section.c). */
        struct section *s = &obj->sections[i];
        uint align = s->align ? s->align : 1;
        buf_align(out, align);
        uint offset = out->len;
        buf_put(out, s->data.data, s->data.len);
        struct shdr *h = new_shdr();
        h->name = strtab_add(&shstr, &shstr_seen, s->name);
        h->ty = s->attrs.ty;
        h->flags = s->attrs.flags;
        h->offset = offset;
        h->size = s->size;
        h->align = align;
        h->entsize = s->attrs.entsize;
    }

    /* The symbol table: null, locals, then globals. Each symbol's index, and
     * the first index of each name, for the relocations. */
    uint *index = calloc(obj->nsymbols + 1, sizeof(uint));
    if (index == NULL) {
        die("out of memory");
    }
    buf_zeros(&symtab, SYM);
    uint n = 1;
    uint first_global = 0;
    /* The first `.file "name"`'s symbol comes before every other local, the
     * others where the source has them (LLVM's ELFObjectWriter). */
    int first_file = -1;
    for (uint i = 0; i < obj->nsymbols && first_file < 0; i++) {
        if (obj->symbols[i].ty == SYM_FILE) {
            first_file = (int)i;
        }
    }
    for (int pass = 0; pass < 2; pass++) {
        if (pass == 1) {
            first_global = n;
        }
        for (int k = pass == 0 && first_file >= 0 ? -1 : 0; k < (int)obj->nsymbols; k++) {
            uint i = k < 0 ? (uint)first_file : (uint)k;
            if (k >= 0 && (int)i == first_file) {
                continue;
            }
            struct symbol *s = &obj->symbols[i];
            if (is_global(s) != pass) {
                continue;
            }
            index[i] = n++;
            uint ty = s->ty == SYM_FUNC      ? 2
                      : s->ty == SYM_OBJECT  ? 1
                      : s->ty == SYM_SECTION ? 3
                      : s->ty == SYM_FILE    ? 4
                                             : 0;
            uint64_t value;
            uint shndx;
            if (s->ty == SYM_FILE) {
                value = 0;
                shndx = SHN_ABS;
            } else if (s->has_constant) {
                /* An `.equ` constant is an absolute symbol whose value is
                 * the constant, whole: `.equ K, -1` is 0xffffffffffffffff. */
                value = (uint64_t)s->constant;
                shndx = SHN_ABS;
            } else if (s->section >= 0) {
                value = s->value;
                shndx = (uint)s->section + 1;
            } else {
                value = 0;
                shndx = SHN_UNDEF;
            }
            /* A section symbol has no name: it stands for its section. */
            uint name = s->ty == SYM_SECTION ? 0 : strtab_add(&strtab, &strtab_seen, s->name);
            uint info = (uint)pass << 4 | ty;
            /* Elf64_Sym: st_name, st_info, st_other, st_shndx, st_value,
             * st_size. */
            buf_u32(&symtab, name);
            buf_byte(&symtab, info);
            buf_byte(&symtab, 0);
            buf_u16(&symtab, shndx);
            buf_u64(&symtab, value);
            buf_u64(&symtab, s->size);
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
        buf_align(out, WORD);
        uint offset = out->len;
        for (uint k = 0; k < s->nrelocs; k++) {
            struct reloc *r = &s->relocs[k];
            uint sym = index[r->sym];
            buf_u64(out, r->offset);
            /* ELF64_R_INFO(sym, type), and the whole 64-bit addend. */
            buf_u32(out, (uint)r->kind);
            buf_u32(out, sym);
            buf_u64(out, (uint64_t)r->addend);
        }
        struct shdr *h = new_shdr();
        h->name = strtab_add(&shstr, &shstr_seen, fmt(".rela%s", s->name));
        h->ty = SHT_RELA;
        h->flags = SHF_INFO_LINK;
        h->offset = offset;
        h->size = RELA * s->nrelocs;
        h->link = symtab_index;
        h->info = i + 1;
        h->align = WORD;
        h->entsize = RELA;
    }
    free(index);

    buf_align(out, WORD);
    struct shdr *h = new_shdr();
    h->name = strtab_add(&shstr, &shstr_seen, ".symtab");
    h->ty = SHT_SYMTAB;
    h->offset = out->len;
    h->size = symtab.len;
    h->link = symtab_index + 1;
    h->info = first_global;
    h->align = WORD;
    h->entsize = SYM;
    buf_put(out, symtab.data, symtab.len);

    h = new_shdr();
    h->name = strtab_add(&shstr, &shstr_seen, ".strtab");
    h->ty = SHT_STRTAB;
    h->offset = out->len;
    h->size = strtab.len;
    h->align = 1;
    buf_put(out, strtab.data, strtab.len);

    h = new_shdr();
    h->name = strtab_add(&shstr, &shstr_seen, ".shstrtab");
    h->ty = SHT_STRTAB;
    h->offset = out->len;
    h->size = shstr.len;
    h->align = 1;
    buf_put(out, shstr.data, shstr.len);

    /* The section headers, the address-sized fields (flags, addr, offset,
     * size, addralign, entsize) 8 bytes. */
    buf_align(out, WORD);
    uint shoff = out->len;
    for (uint i = 0; i < nshdrs; i++) {
        struct shdr *s = &shdrs[i];
        buf_u32(out, s->name);
        buf_u32(out, s->ty);
        buf_u64(out, s->flags);
        buf_u64(out, 0); /* sh_addr */
        buf_u64(out, s->offset);
        buf_u64(out, s->size);
        buf_u32(out, s->link);
        buf_u32(out, s->info);
        buf_u64(out, s->align);
        buf_u64(out, s->entsize);
    }

    /* The ELF header. */
    struct buf hdr;
    memset(&hdr, 0, sizeof hdr);
    buf_put(&hdr, "\177ELF", 4);
    buf_byte(&hdr, ELFCLASS64);
    buf_byte(&hdr, 1); /* little-endian */
    buf_byte(&hdr, 1); /* version 1 */
    buf_byte(&hdr, 0); /* SysV ABI */
    buf_zeros(&hdr, 8);
    buf_u16(&hdr, 1); /* ET_REL */
    buf_u16(&hdr, EM_RISCV);
    buf_u32(&hdr, 1); /* e_version */
    buf_u64(&hdr, 0); /* e_entry */
    buf_u64(&hdr, 0); /* e_phoff */
    buf_u64(&hdr, shoff);
    buf_u32(&hdr, 0); /* e_flags: the soft-float ABI (lp64), no RVC */
    buf_u16(&hdr, EHSIZE);
    buf_u16(&hdr, 0); /* e_phentsize */
    buf_u16(&hdr, 0); /* e_phnum */
    buf_u16(&hdr, SHENTSIZE);
    buf_u16(&hdr, nshdrs);
    buf_u16(&hdr, nshdrs - 1);
    memcpy(out->data, hdr.data, EHSIZE);
    free(hdr.data);
}
