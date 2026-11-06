/* ar: reading and writing archives, as the elf crate's archive.rs does
 * (which describes the format), and ar r's replacing of members. */
#include "ar.h"

#define MAGIC "!<arch>\n"
#define MAGIC_LEN 8
#define HEADER 60
/* The longest name a header holds itself, with the '/' after it 16. */
#define MAX_SHORT_NAME 15

int is_archive(const uchar *data, uint len) {
    return len >= MAGIC_LEN && memcmp(data, MAGIC, MAGIC_LEN) == 0;
}

/* ---- Reading ---- */

static uint field_len(const uchar *p, uint len) {
    while (len > 0 && p[len - 1] == ' ') {
        len--;
    }
    return len;
}

static int digits(const uchar *p, uint len, uint64_t *out) {
    if (len == 0) {
        return 0;
    }
    uint64_t v = 0;
    for (uint i = 0; i < len; i++) {
        if (p[i] < '0' || p[i] > '9') {
            return 0;
        }
        v = v * 10 + (uint64_t)(p[i] - '0');
    }
    *out = v;
    return 1;
}

static int field_is(const uchar *p, uint len, const char *s) {
    uint n = (uint)strlen(s);
    return len == n && memcmp(p, s, n) == 0;
}

int archive_read(uchar *data, uint len, struct member **out, uint *count, char **err) {
    struct member *members = NULL;
    uint n = 0, cap = 0;
    const uchar *names = NULL;
    uint nnames = 0;
    int has_table = 0;
    uint at = MAGIC_LEN;
    if (!is_archive(data, len)) {
        *err = "not an archive (no !<arch> magic)";
        return 0;
    }
    while (at < len) {
        if (len - at < HEADER) {
            *err = "truncated header";
            return 0;
        }
        const uchar *h = data + at;
        uint64_t size;
        if (h[58] != '`' || h[59] != '\n' || !digits(h + 48, field_len(h + 48, 10), &size)) {
            *err = "bad header";
            return 0;
        }
        uint start = at + HEADER;
        if (size > len - start) {
            *err = "a member runs past the end";
            return 0;
        }
        uint flen = field_len(h, 16);
        if (field_is(h, flen, "/") && n == 0 && !has_table && nnames == 0) {
            has_table = 1;
        } else if (field_is(h, flen, "//") && n == 0 && nnames == 0) {
            names = data + start;
            nnames = (uint)size;
        } else if (field_is(h, flen, "/SYM64/") || (flen >= 3 && memcmp(h, "#1/", 3) == 0)) {
            *err = "not a System V (GNU) archive with a 32-bit symbol table";
            return 0;
        } else {
            char *name = NULL;
            if (flen > 0 && h[0] == '/') {
                uint64_t off;
                if (digits(h + 1, flen - 1, &off) && off <= nnames) {
                    for (uint i = (uint)off; i + 1 < nnames && name == NULL; i++) {
                        if (names[i] == '/' && names[i + 1] == '\n') {
                            name = xstrndup((const char *)names + off, i - (uint)off);
                        }
                    }
                }
            } else if (flen > 0 && h[flen - 1] == '/') {
                name = xstrndup((const char *)h, flen - 1);
            }
            if (name == NULL) {
                *err = "bad member name";
                return 0;
            }
            members = grow(members, &cap, n + 1, sizeof(struct member));
            members[n].name = name;
            members[n].data = data + start;
            members[n].len = (uint)size;
            n++;
        }
        at = start + (uint)size + ((uint)size & 1);
    }
    *out = members;
    *count = n;
    return 1;
}

/* ---- Writing ---- */

/* `text` padded with spaces to `width` bytes. */
static void pad(struct buf *b, const char *text, uint width) {
    uint n = (uint)strlen(text);
    buf_put(b, text, n);
    for (; n < width; n++) {
        buf_byte(b, ' ');
    }
}

/* A header: the name, the time, ids and mode as given (blank when `ids` is
 * NULL), the size. */
static void header(struct buf *b, const char *name, const char *ids, const char *mode, uint size) {
    char num[16];
    pad(b, name, 16);
    pad(b, ids != NULL ? ids : "", 12);
    pad(b, ids != NULL ? ids : "", 6);
    pad(b, ids != NULL ? ids : "", 6);
    pad(b, mode != NULL ? mode : "", 8);
    snprintf(num, sizeof num, "%u", size);
    pad(b, num, 10);
    buf_put(b, "`\n", 2);
}

/* Whether a name goes in the long names member: longer than 15 bytes, or
 * with a '/', as llvm-ar decides. */
static int is_long(const char *name) {
    return strlen(name) > MAX_SHORT_NAME || strchr(name, '/') != NULL;
}

static void be32(struct buf *b, uint v) {
    buf_byte(b, v >> 24 & 0xff);
    buf_byte(b, v >> 16 & 0xff);
    buf_byte(b, v >> 8 & 0xff);
    buf_byte(b, v & 0xff);
}

int archive_write(struct member *m, uint n, int symtab, struct buf *out, char **err) {
    buf_put(out, MAGIC, MAGIC_LEN);
    if (n == 0) {
        return 1;
    }
    /* The long names, each once, in the order first met; where each
     * member's name is there (-1: in its header). */
    struct buf longs;
    memset(&longs, 0, sizeof longs);
    int *long_at = xmalloc(n * sizeof(int));
    for (uint i = 0; i < n; i++) {
        long_at[i] = -1;
        if (!is_long(m[i].name)) {
            continue;
        }
        for (uint j = 0; j < i && long_at[i] < 0; j++) {
            if (long_at[j] >= 0 && strcmp(m[j].name, m[i].name) == 0) {
                long_at[i] = long_at[j];
            }
        }
        if (long_at[i] < 0) {
            long_at[i] = (int)longs.len;
            buf_put(&longs, m[i].name, (uint)strlen(m[i].name));
            buf_put(&longs, "/\n", 2);
        }
    }
    if (longs.len % 2 == 1) {
        buf_byte(&longs, '\n');
    }
    /* Each member's symbols: their names in one buffer, and how many. */
    struct buf names;
    memset(&names, 0, sizeof names);
    uint *nsyms = xmalloc(n * sizeof(uint));
    uint count = 0;
    for (uint i = 0; i < n; i++) {
        nsyms[i] = 0;
        char *msg;
        if (!defined_symbols(m[i].data, m[i].len, &names, &nsyms[i], &msg)) {
            *err = msg;
            return 0;
        }
        count += nsyms[i];
    }
    if (names.len == 0) {
        /* No symbols: four NULs for names, as llvm-ar writes. */
        for (uint k = 0; k < 4; k++) {
            buf_byte(&names, 0);
        }
    }
    uint table = 4 + 4 * count + names.len;
    if (table % 2 == 1) {
        buf_byte(&names, 0);
        table++;
    }
    /* Where each member's header goes. */
    uint *offsets = xmalloc(n * sizeof(uint));
    uint at = MAGIC_LEN + (symtab ? HEADER + table : 0);
    if (longs.len != 0) {
        at += HEADER + longs.len;
    }
    for (uint i = 0; i < n; i++) {
        offsets[i] = at;
        at += HEADER + m[i].len + (m[i].len & 1);
    }
    if (symtab) {
        header(out, "/", "0", "0", table);
        be32(out, count);
        for (uint i = 0; i < n; i++) {
            for (uint k = 0; k < nsyms[i]; k++) {
                be32(out, offsets[i]);
            }
        }
        buf_put(out, names.data, names.len);
    }
    if (longs.len != 0) {
        header(out, "//", NULL, NULL, longs.len);
        buf_put(out, longs.data, longs.len);
    }
    for (uint i = 0; i < n; i++) {
        char name[32];
        uint len = (uint)strlen(m[i].name);
        char *field = name;
        if (long_at[i] >= 0) {
            snprintf(name, sizeof name, "/%u", (uint)long_at[i]);
        } else {
            field = xmalloc(len + 2);
            memcpy(field, m[i].name, len);
            field[len] = '/';
            field[len + 1] = 0;
        }
        header(out, field, "0", "644", m[i].len);
        buf_put(out, m[i].data, m[i].len);
        if (m[i].len % 2 == 1) {
            buf_byte(out, '\n');
        }
    }
    return 1;
}

struct member *archive_replace(struct member *old, uint nold, struct member *files, uint nfiles, uint *count) {
    struct member *out = xmalloc((nold + nfiles + 1) * sizeof(struct member));
    uchar *used = xmalloc(nfiles + 1);
    memset(used, 0, nfiles + 1);
    uint n = 0;
    for (uint i = 0; i < nold; i++) {
        int match = -1;
        for (uint j = 0; j < nfiles && match < 0; j++) {
            if (!used[j] && strcmp(files[j].name, old[i].name) == 0) {
                match = (int)j;
            }
        }
        if (match >= 0) {
            used[match] = 1;
            out[n++] = files[match];
        } else {
            out[n++] = old[i];
        }
    }
    for (uint j = 0; j < nfiles; j++) {
        if (!used[j]) {
            out[n++] = files[j];
        }
    }
    *count = n;
    return out;
}
