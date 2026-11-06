/* cc1: memory, byte buffers, formatting, maps and UTF-8 (see cc.h). What
 * Rust's standard library gives merlin: String, Vec, HashMap, format!. */
#include "cc.h"

typedef __builtin_va_list va_list;
#define va_start(ap, last) __builtin_va_start(ap, last)
#define va_arg(ap, type) __builtin_va_arg(ap, type)
#define va_end(ap) __builtin_va_end(ap)

void die(const char *msg) {
    fprintf(2, "cc1: %s\n", msg);
    exit(2);
}

/* ---- The arena ----
 * A bump allocator over chunks taken from sbrk: each allocation is the
 * next bytes of the current chunk, 8-aligned; a chunk that has no room
 * left is abandoned for a new one (malloc takes from sbrk too, so two
 * chunks need not be next to each other). Nothing is freed: the
 * compilation's end, the process's exit, frees it all, as compilers that
 * allocate per translation unit do. */

#define CHUNK (256 * 1024)

static char *chunk;
static uint chunk_used;
static uint chunk_size;
static uint arena_total;

void *arena_alloc(uint n) {
    n = (n + 7) & ~7u;
    if (chunk == NULL || chunk_size - chunk_used < n) {
        uint size = n > CHUNK ? n : CHUNK;
        char *got = sbrk((long)size + 8);
        if ((long)got < 0) {
            die("out of memory");
        }
        /* sbrk's memory may start anywhere: align it. */
        chunk = (char *)(((uintptr_t)got + 7) & ~(uintptr_t)7);
        chunk_used = 0;
        chunk_size = size;
    }
    void *r = chunk + chunk_used;
    chunk_used += n;
    arena_total += n;
    memset(r, 0, n);
    return r;
}

uint arena_used(void) {
    return arena_total;
}

char *arena_strndup(const char *s, uint n) {
    char *r = arena_alloc(n + 1);
    memcpy(r, s, n);
    return r;
}

char *arena_strdup(const char *s) {
    return arena_strndup(s, (uint)strlen(s));
}

void *grow(void *p, uint *cap, uint need, uint size) {
    if (need <= *cap) {
        return p;
    }
    /* Double the capacity until it holds need (stopping at need where
     * doubling would pass 32 bits), and refuse a byte count past 32 bits:
     * no Camelot process has that much memory. */
    uint c = *cap ? *cap : 8;
    while (c < need) {
        if (c > 0xffffffffu / 2) {
            c = need;
            break;
        }
        c *= 2;
    }
    if (size != 0 && c > 0xffffffffu / size) {
        die("out of memory");
    }
    void *q = realloc(p, (size_t)c * size);
    if (q == NULL) {
        die("out of memory");
    }
    *cap = c;
    return q;
}

/* ---- Byte buffers ---- */

void buf_put(struct buf *b, const void *p, uint n) {
    if (b->len + n < b->len) {
        die("out of memory");
    }
    /* One byte more than asked, so that buf_arena and the lexer can end the
     * bytes with a NUL. */
    b->data = grow(b->data, &b->cap, b->len + n + 1, 1);
    memcpy(b->data + b->len, p, n);
    b->len += n;
    b->data[b->len] = 0;
}

void buf_str(struct buf *b, const char *s) {
    buf_put(b, s, (uint)strlen(s));
}

void buf_byte(struct buf *b, uint c) {
    char ch = (char)c;
    buf_put(b, &ch, 1);
}

char *buf_arena(struct buf *b) {
    return arena_strndup(b->len ? b->data : "", b->len);
}

void buf_free(struct buf *b) {
    free(b->data);
    b->data = NULL;
    b->len = 0;
    b->cap = 0;
}

/* v in decimal. */
static void put_decimal(struct buf *b, uint64_t v) {
    char digits[24];
    int n = 0;
    do {
        digits[n++] = (char)('0' + v % 10);
        v /= 10;
    } while (v != 0);
    while (n > 0) {
        buf_byte(b, (uchar)digits[--n]);
    }
}

/* buf_fmt's work, on an argument list. */
static void vformat(struct buf *b, const char *f, va_list ap) {
    for (; *f; f++) {
        if (*f != '%') {
            buf_byte(b, (uchar)*f);
            continue;
        }
        f++;
        /* `ll`: the argument is a long long, int64_t or uint64_t. */
        int wide = 0;
        if (f[0] == 'l' && f[1] == 'l') {
            wide = 1;
            f += 2;
        }
        if (*f == 'd') {
            int64_t v = wide ? va_arg(ap, int64_t) : (int64_t)va_arg(ap, int);
            if (v < 0) {
                buf_byte(b, '-');
                put_decimal(b, 0 - (uint64_t)v);
            } else {
                put_decimal(b, (uint64_t)v);
            }
        } else if (*f == 'u') {
            put_decimal(b, wide ? va_arg(ap, uint64_t) : (uint64_t)va_arg(ap, uint));
        } else if (*f == 's') {
            buf_str(b, va_arg(ap, const char *));
        } else if (*f == 'c') {
            buf_byte(b, (uchar)va_arg(ap, int));
        } else {
            buf_byte(b, (uchar)*f);
        }
    }
}

void buf_fmt(struct buf *b, const char *f, ...) {
    va_list ap;
    va_start(ap, f);
    vformat(b, f, ap);
    va_end(ap);
}

char *fmt(const char *f, ...) {
    struct buf b;
    memset(&b, 0, sizeof b);
    va_list ap;
    va_start(ap, f);
    vformat(&b, f, ap);
    va_end(ap);
    char *r = buf_arena(&b);
    buf_free(&b);
    return r;
}

/* ---- Maps ---- */

/* FNV-1a, over the key's bytes. */
static uint hash(const char *s) {
    uint h = 2166136261u;
    for (; *s; s++) {
        h = (h ^ (uchar)*s) * 16777619u;
    }
    return h;
}

void *map_get(struct map *m, const char *key) {
    if (m->cap == 0) {
        return NULL;
    }
    for (uint i = hash(key) & (m->cap - 1);; i = (i + 1) & (m->cap - 1)) {
        if (m->keys[i] == NULL) {
            return NULL;
        }
        if (strcmp(m->keys[i], key) == 0) {
            return m->values[i];
        }
    }
}

static void map_resize(struct map *m, uint cap) {
    struct map old = *m;
    m->cap = cap;
    m->keys = calloc(cap, sizeof(char *));
    m->values = calloc(cap, sizeof(void *));
    if (m->keys == NULL || m->values == NULL) {
        die("out of memory");
    }
    m->count = 0;
    for (uint i = 0; i < old.cap; i++) {
        if (old.keys[i] != NULL) {
            map_put(m, old.keys[i], old.values[i]);
        }
    }
    free(old.keys);
    free(old.values);
}

void map_put(struct map *m, const char *key, void *value) {
    if ((m->count + 1) * 2 > m->cap) {
        map_resize(m, m->cap ? m->cap * 2 : 64);
    }
    for (uint i = hash(key) & (m->cap - 1);; i = (i + 1) & (m->cap - 1)) {
        if (m->keys[i] == NULL) {
            m->keys[i] = key;
            m->values[i] = value;
            m->count++;
            return;
        }
        if (strcmp(m->keys[i], key) == 0) {
            m->values[i] = value;
            return;
        }
    }
}

int map_remove(struct map *m, const char *key) {
    if (m->cap == 0) {
        return 0;
    }
    uint i = hash(key) & (m->cap - 1);
    for (;; i = (i + 1) & (m->cap - 1)) {
        if (m->keys[i] == NULL) {
            return 0;
        }
        if (strcmp(m->keys[i], key) == 0) {
            break;
        }
    }
    /* Open addressing: the keys after it in its run move back where their
     * own slot lets them, so that every key stays reachable. */
    m->keys[i] = NULL;
    m->count--;
    for (uint j = (i + 1) & (m->cap - 1); m->keys[j] != NULL; j = (j + 1) & (m->cap - 1)) {
        const char *k = m->keys[j];
        void *v = m->values[j];
        m->keys[j] = NULL;
        m->count--;
        map_put(m, k, v);
    }
    return 1;
}

/* The interned identifiers: each text once, the map's value the string. */
static struct map interned;

const char *intern(const char *s, uint len) {
    /* The key to look for must be NUL-terminated: copy short ones to the
     * stack. */
    char small[64];
    char *key = small;
    if (len >= sizeof small) {
        key = malloc(len + 1);
        if (key == NULL) {
            die("out of memory");
        }
    }
    memcpy(key, s, len);
    key[len] = 0;
    const char *found = map_get(&interned, key);
    if (found == NULL) {
        char *copy = arena_strndup(s, len);
        map_put(&interned, copy, copy);
        found = copy;
    }
    if (key != small) {
        free(key);
    }
    return found;
}

/* ---- Positions ---- */

struct pos pos_at(uint file, uint line, uint col) {
    struct pos p;
    p.file = file;
    p.line = line;
    p.col = col;
    return p;
}

int pos_eq(struct pos a, struct pos b) {
    return a.file == b.file && a.line == b.line && a.col == b.col;
}

int pos_cmp(struct pos a, struct pos b) {
    if (a.file != b.file) {
        return a.file < b.file ? -1 : 1;
    }
    if (a.line != b.line) {
        return a.line < b.line ? -1 : 1;
    }
    if (a.col != b.col) {
        return a.col < b.col ? -1 : 1;
    }
    return 0;
}

/* ---- UTF-8 ---- */

uint utf8_len(uint c) {
    if (c >= 0xf0 && c < 0xf8) {
        return 4;
    }
    if (c >= 0xe0 && c < 0xf0) {
        return 3;
    }
    if (c >= 0xc0 && c < 0xe0) {
        return 2;
    }
    return 1;
}

uint utf8_decode(const uchar *s, uint n, uint *len) {
    uint l = utf8_len(s[0]);
    if (l > n) {
        l = 1;
    }
    *len = l;
    if (l == 1) {
        return s[0];
    }
    uint v = s[0] & (0x7f >> l);
    for (uint k = 1; k < l; k++) {
        v = v << 6 | (s[k] & 0x3f);
    }
    return v;
}

void utf8_put(struct buf *b, uint v) {
    if (v < 0x80) {
        buf_byte(b, v);
    } else if (v < 0x800) {
        buf_byte(b, 0xc0 | v >> 6);
        buf_byte(b, 0x80 | (v & 0x3f));
    } else if (v < 0x10000) {
        buf_byte(b, 0xe0 | v >> 12);
        buf_byte(b, 0x80 | (v >> 6 & 0x3f));
        buf_byte(b, 0x80 | (v & 0x3f));
    } else {
        buf_byte(b, 0xf0 | v >> 18);
        buf_byte(b, 0x80 | (v >> 12 & 0x3f));
        buf_byte(b, 0x80 | (v >> 6 & 0x3f));
        buf_byte(b, 0x80 | (v & 0x3f));
    }
}

int valid_utf8(const uchar *s, uint n) {
    uint i = 0;
    while (i < n) {
        uint c = s[i];
        uint len, min;
        if (c < 0x80) {
            i++;
            continue;
        } else if ((c & 0xe0) == 0xc0) {
            len = 2;
            min = 0x80;
            c &= 0x1f;
        } else if ((c & 0xf0) == 0xe0) {
            len = 3;
            min = 0x800;
            c &= 0x0f;
        } else if ((c & 0xf8) == 0xf0) {
            len = 4;
            min = 0x10000;
            c &= 0x07;
        } else {
            return 0;
        }
        if (i + len > n) {
            return 0;
        }
        for (uint k = 1; k < len; k++) {
            if ((s[i + k] & 0xc0) != 0x80) {
                return 0;
            }
            c = c << 6 | (s[i + k] & 0x3f);
        }
        if (c < min || c > 0x10ffff || (c >= 0xd800 && c <= 0xdfff)) {
            return 0;
        }
        i += len;
    }
    return 1;
}
