/* ar: the symbols a member defines, which the archive's symbol table
 * lists (the elf crate's archive::symbols): for an ELF object, each symbol
 * after the null one whose binding is not local and whose section is not
 * undefined, in its symbol table's order: globals and weak ones, common
 * and absolute ones too. The first SHT_SYMTAB section is the symbol table,
 * its sh_link the string table, as the elf crate reads them; both classes,
 * ELF32 and ELF64, little-endian. */
#include "ar.h"

#define SHT_SYMTAB 2
#define STB_LOCAL 0
#define SHN_UNDEF 0

static uint le16(const uchar *p) {
    return (uint)p[0] | (uint)p[1] << 8;
}

static uint le32(const uchar *p) {
    return (uint)p[0] | (uint)p[1] << 8 | (uint)p[2] << 16 | (uint)p[3] << 24;
}

static uint64_t le64(const uchar *p) {
    return (uint64_t)le32(p) | (uint64_t)le32(p + 4) << 32;
}

/* The file being read and its length. */
static const uchar *f;
static uint flen;

/* Whether bytes [off, off + n) are in the file. */
static int in_file(uint64_t off, uint64_t n) {
    return off <= flen && n <= flen - off;
}

/* A section header's fields, of either class. */
struct shdr {
    uint type;
    uint64_t offset;
    uint64_t size;
    uint link;
    uint64_t entsize;
};

static int read_shdr(int wide, uint64_t at, struct shdr *s) {
    uint need = wide ? 64 : 40;
    if (!in_file(at, need)) {
        return 0;
    }
    const uchar *p = f + at;
    s->type = le32(p + 4);
    if (wide) {
        s->offset = le64(p + 24);
        s->size = le64(p + 32);
        s->link = le32(p + 40);
        s->entsize = le64(p + 56);
    } else {
        s->offset = le32(p + 16);
        s->size = le32(p + 20);
        s->link = le32(p + 24);
        s->entsize = le32(p + 36);
    }
    return 1;
}

int defined_symbols(const uchar *data, uint len, struct buf *names, uint *count, char **err) {
    if (len < 4 || memcmp(data, "\177ELF", 4) != 0) {
        return 1;
    }
    f = data;
    flen = len;
    *err = "not an ELF object ar can read";
    if (len < 52 || data[5] != 1 || (data[4] != 1 && data[4] != 2)) {
        return 0;
    }
    int wide = data[4] == 2;
    if (wide && len < 64) {
        return 0;
    }
    uint64_t shoff = wide ? le64(data + 40) : le32(data + 32);
    uint shentsize = le16(data + (wide ? 58 : 46));
    uint shnum = le16(data + (wide ? 60 : 48));
    if (shnum != 0 && shentsize < (wide ? 64u : 40u)) {
        return 0;
    }
    struct shdr symtab;
    int found = 0;
    for (uint i = 0; i < shnum && !found; i++) {
        if (!read_shdr(wide, shoff + (uint64_t)i * shentsize, &symtab)) {
            return 0;
        }
        found = symtab.type == SHT_SYMTAB;
    }
    if (!found) {
        return 1;
    }
    struct shdr strtab;
    if (symtab.link >= shnum || !read_shdr(wide, shoff + (uint64_t)symtab.link * shentsize, &strtab) ||
        !in_file(symtab.offset, symtab.size) || !in_file(strtab.offset, strtab.size)) {
        return 0;
    }
    uint symsize = wide ? 24 : 16;
    uint64_t entsize = symtab.entsize > symsize ? symtab.entsize : symsize;
    uint nsyms = (uint)(symtab.size / entsize);
    const uchar *strings = f + strtab.offset;
    for (uint i = 1; i < nsyms; i++) {
        const uchar *s = f + symtab.offset + i * entsize;
        uint name = le32(s);
        uint info = wide ? s[4] : s[12];
        uint shndx = le16(s + (wide ? 6 : 14));
        if (info >> 4 == STB_LOCAL || shndx == SHN_UNDEF) {
            continue;
        }
        if (name >= strtab.size) {
            return 0;
        }
        uint end = name;
        while (end < strtab.size && strings[end] != 0) {
            end++;
        }
        if (end == strtab.size) {
            return 0;
        }
        buf_put(names, strings + name, end - name + 1);
        (*count)++;
    }
    return 1;
}
