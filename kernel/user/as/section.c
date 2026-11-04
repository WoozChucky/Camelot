/* as: a section's ELF type, flags and entry size, as `.section` decides
 * them (merlin-as's section.rs, which explains LLVM's rules):
 *
 *   .section name[, "flags"[, @type[, entsize]]]
 *
 * A name LLVM knows by its prefix has flags and a type without being told
 * (section_name_defaults); the flags string adds to them and the type
 * replaces the name's; the sections LLVM makes before the first line
 * (PREDEFINED_SECTIONS, generated from merlin-as's table) keep their own.
 * The tables of flag letters and type names are generated too (tables.c). */
#include "as.h"
#include "tables.h"

/* LLVM's hasPrefix: `name` is `prefix` or starts with `prefix.` (`.text.f`
 * has the prefix `.text`, `.textual` does not). */
static int has_prefix(const char *name, const char *prefix) {
    uint n = (uint)strlen(prefix);
    return strncmp(name, prefix, n) == 0 && (name[n] == 0 || name[n] == '.');
}

void section_name_defaults(const char *name, uint *ty, uint *flags) {
    if (has_prefix(name, ".rodata") || strcmp(name, ".rodata1") == 0) {
        *flags = SHF_ALLOC;
    } else if (strcmp(name, ".fini") == 0 || strcmp(name, ".init") == 0 || has_prefix(name, ".text")) {
        *flags = SHF_ALLOC | SHF_EXECINSTR;
    } else if (has_prefix(name, ".data") || strcmp(name, ".data1") == 0 || has_prefix(name, ".bss") ||
               has_prefix(name, ".init_array") || has_prefix(name, ".fini_array") ||
               has_prefix(name, ".preinit_array")) {
        *flags = SHF_ALLOC | SHF_WRITE;
    } else if (has_prefix(name, ".tdata") || has_prefix(name, ".tbss")) {
        *flags = SHF_ALLOC | SHF_WRITE | SHF_TLS;
    } else {
        *flags = 0;
    }
    /* `.note` is a plain prefix here (`.notes` too), as in LLVM. */
    if (strncmp(name, ".note", 5) == 0) {
        *ty = SHT_NOTE;
    } else if (has_prefix(name, ".init_array")) {
        *ty = SHT_INIT_ARRAY;
    } else if (has_prefix(name, ".bss") || has_prefix(name, ".tbss")) {
        *ty = SHT_NOBITS;
    } else if (has_prefix(name, ".fini_array")) {
        *ty = SHT_FINI_ARRAY;
    } else if (has_prefix(name, ".preinit_array")) {
        *ty = SHT_PREINIT_ARRAY;
    } else {
        *ty = SHT_PROGBITS;
    }
}

int section_predefined(const char *name, struct sec_attrs *out) {
    for (int i = 0; PREDEFINED_SECTIONS[i].name; i++) {
        if (strcmp(PREDEFINED_SECTIONS[i].name, name) == 0) {
            out->ty = PREDEFINED_SECTIONS[i].ty;
            out->flags = PREDEFINED_SECTIONS[i].flags;
            out->entsize = PREDEFINED_SECTIONS[i].entsize;
            return 1;
        }
    }
    return 0;
}

void section_by_name(const char *name, struct sec_attrs *out) {
    if (section_predefined(name, out)) {
        return;
    }
    section_name_defaults(name, &out->ty, &out->flags);
    out->entsize = 0;
}

/* The value of a digit in any base up to 36, or 36 if c is not one. */
static uint digit_value(int c) {
    if (c >= '0' && c <= '9') {
        return (uint)(c - '0');
    }
    if (c >= 'a' && c <= 'z') {
        return (uint)(c - 'a' + 10);
    }
    if (c >= 'A' && c <= 'Z') {
        return (uint)(c - 'A' + 10);
    }
    return 36;
}

int section_number(const uchar *s, uint len, uint *out) {
    uint radix = 10, at = 0;
    if (len > 1 && s[0] == '0') {
        int c = s[1];
        if (c == 'x' || c == 'X') {
            radix = 16;
            at = 2;
        } else if (c == 'b' || c == 'B') {
            radix = 2;
            at = 2;
        } else if (c == 'o' || c == 'O') {
            radix = 8;
            at = 2;
        } else {
            radix = 8;
            at = 1;
        }
    }
    if (at == len) {
        return 0;
    }
    uint v = 0;
    for (; at < len; at++) {
        uint d = digit_value(s[at]);
        /* Not a digit of the base, or more than 32 bits. */
        if (d >= radix || v > (0xffffffffu - d) / radix) {
            return 0;
        }
        v = v * radix + d;
    }
    *out = v;
    return 1;
}

int section_flags(const uchar *s, uint len, uint *out, uint *bad) {
    if (section_number(s, len, out)) {
        return 1;
    }
    uint flags = 0;
    for (uint k = 0; k < len; k++) {
        int i = 0;
        while (SECTION_FLAGS[i].name && (uchar)SECTION_FLAGS[i].name[0] != s[k]) {
            i++;
        }
        if (!SECTION_FLAGS[i].name) {
            *bad = s[k];
            return 0;
        }
        flags |= SECTION_FLAGS[i].value;
    }
    *out = flags;
    return 1;
}

int section_type(const uchar *s, uint len, uint *out) {
    for (int i = 0; SECTION_TYPES[i].name; i++) {
        const char *n = SECTION_TYPES[i].name;
        if (strlen(n) == len && memcmp(n, s, len) == 0) {
            *out = SECTION_TYPES[i].value;
            return 1;
        }
    }
    return section_number(s, len, out);
}
