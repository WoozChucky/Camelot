/* ld: reads ar archives (libc.a), the System V format with GNU's long
 * names that GNU ar, llvm-ar and /bin/ar write (the elf crate's
 * archive.rs, which merlin-ld reads them with, describes it): the magic,
 * then members, each after a 60-byte header of text fields and on an even
 * offset; "/" is the symbol table, "//" the long names. The symbol table
 * is not read: as ld.lld does, the linker goes by each member's own
 * symbols (library.c), so a table out of date changes nothing (the elf
 * crate's parse_members). Messages are the elf crate's. */
#include "ld.h"

#define MAGIC_LEN 8
#define HEADER 60

int is_archive(const uchar *data, uint len) {
    return len >= MAGIC_LEN && memcmp(data, "!<arch>\n", MAGIC_LEN) == 0;
}

int is_thin_archive(const uchar *data, uint len) {
    return len >= MAGIC_LEN && memcmp(data, "!<thin>\n", MAGIC_LEN) == 0;
}

/* The length of a header field without the spaces after it. */
static uint field_len(const uchar *f, uint len) {
    while (len > 0 && f[len - 1] == ' ') {
        len--;
    }
    return len;
}

/* A decimal number of `len` digits (at least one, nothing else). */
static int digits(const uchar *f, uint len, uint64_t *out) {
    if (len == 0) {
        return 0;
    }
    uint64_t v = 0;
    for (uint i = 0; i < len; i++) {
        if (f[i] < '0' || f[i] > '9') {
            return 0;
        }
        v = v * 10 + (uint64_t)(f[i] - '0');
    }
    *out = v;
    return 1;
}

static int field_is(const uchar *f, uint len, const char *s) {
    uint n = (uint)strlen(s);
    return len == n && memcmp(f, s, n) == 0;
}

/* A member's name from its header's name field (`f`, `len` bytes): `name/`,
 * or `/OFFSET` into the long names. NULL with *err set. */
static char *member_name(const uchar *f, uint len, const uchar *names, uint nnames, uint at, char **err) {
    if (len > 0 && f[0] == '/') {
        uint64_t off;
        if (!digits(f + 1, len - 1, &off) || off > nnames) {
            *err = fmt("archive member at %u: bad long name", at);
            return NULL;
        }
        for (uint i = (uint)off; i + 1 < nnames; i++) {
            if (names[i] == '/' && names[i + 1] == '\n') {
                return xstrndup((const char *)names + off, i - (uint)off);
            }
        }
        *err = fmt("archive member at %u: unterminated long name", at);
        return NULL;
    }
    if (len == 0 || f[len - 1] != '/') {
        *err = fmt("archive member at %u: name without a /", at);
        return NULL;
    }
    return xstrndup((const char *)f, len - 1);
}

int archive_parse(uchar *data, uint len, struct ar_member **out, uint *count, char **err) {
    struct ar_member *members = NULL;
    uint n = 0, cap = 0;
    const uchar *names = NULL;
    uint nnames = 0;
    int has_table = 0;
    uint at = MAGIC_LEN;
    while (at < len) {
        if (len - at < HEADER) {
            *err = fmt("archive member at %u: truncated header", at);
            return 0;
        }
        const uchar *h = data + at;
        if (h[58] != '`' || h[59] != '\n') {
            *err = fmt("archive member at %u: bad header end", at);
            return 0;
        }
        uint64_t size;
        if (!digits(h + 48, field_len(h + 48, 10), &size)) {
            *err = fmt("archive member at %u: bad size", at);
            return 0;
        }
        uint start = at + HEADER;
        if (size > len - start) {
            *err = fmt("archive member at %u: %llu bytes run past the end", at, size);
            return 0;
        }
        uint flen = field_len(h, 16);
        if (field_is(h, flen, "/") && n == 0 && !has_table && nnames == 0) {
            has_table = 1;
        } else if (field_is(h, flen, "//") && n == 0 && nnames == 0) {
            names = data + start;
            nnames = (uint)size;
        } else if (field_is(h, flen, "/SYM64/")) {
            *err = "a 64-bit archive symbol table (/SYM64/) is not supported";
            return 0;
        } else if (flen >= 3 && memcmp(h, "#1/", 3) == 0) {
            *err = "BSD archives are not supported";
            return 0;
        } else {
            char *name = member_name(h, flen, names, nnames, at, err);
            if (name == NULL) {
                return 0;
            }
            members = grow(members, &cap, n + 1, sizeof(struct ar_member));
            members[n].name = name;
            members[n].offset = at;
            members[n].data = data + start;
            members[n].len = (uint)size;
            n++;
        }
        /* The next header is on an even offset. */
        at = start + (uint)size + ((uint)size & 1);
    }
    *out = members;
    *count = n;
    return 1;
}
