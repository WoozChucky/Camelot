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
 * A bump allocator: each allocation is the next bytes of the current chunk,
 * 8-aligned, and a chunk with no room left for one is followed by a new
 * chunk (what little is left of the old one, less than BIG bytes, stays
 * unused). An allocation bigger than BIG gets a block of its own, which
 * leaves the current chunk as it is. The chunks and blocks come from malloc
 * (which takes its memory from sbrk) and are freed all together when the
 * compilation ends (arena_free), as merlin's memory is when a translation
 * unit is done: nothing in the arena is freed on its own. */

#define CHUNK (256 * 1024)
#define BIG (CHUNK / 4)

/* A chunk or a block of the arena: its header, then its bytes. */
struct block {
    struct block *next;
    uint size;
    uint used;
};

#define HEADER ((sizeof(struct block) + 7) & ~7u)

/* The current chunk, the chunks before it after it in the list. */
static struct block *chunks;
/* The blocks of the big allocations. */
static struct block *bigs;

static struct block *new_block(uint size, struct block *next) {
    if (size > 0xffffffffu - HEADER) {
        die("out of memory");
    }
    struct block *b = malloc(HEADER + size);
    if (b == NULL) {
        die("out of memory");
    }
    b->next = next;
    b->size = size;
    b->used = 0;
    return b;
}

void *arena_alloc(uint n) {
    if (n > 0xffffffffu - 7) {
        die("out of memory");
    }
    n = (n + 7) & ~7u;
    struct block *b;
    if (n > BIG) {
        bigs = new_block(n, bigs);
        b = bigs;
    } else {
        if (chunks == NULL || chunks->size - chunks->used < n) {
            chunks = new_block(CHUNK, chunks);
        }
        b = chunks;
    }
    void *r = (char *)b + HEADER + b->used;
    b->used += n;
    memset(r, 0, n);
    return r;
}

static void free_blocks(struct block *b) {
    while (b != NULL) {
        struct block *next = b->next;
        free(b);
        b = next;
    }
}

/* ---- The translation unit's heap ----
 * What grows or is freed early (an array, a map's table, a buffer) is
 * malloc'd, each block with a header linking it into the list of what the
 * translation unit holds: heap_free takes one out and frees it, and what is
 * still there when the translation unit is done goes with the arena. So
 * whatever a compilation keeps, the syntax tree's arrays as much as its
 * nodes, is freed when it ends, as merlin's is. */

/* A block's header, before its bytes: 16, which keeps them aligned. */
struct held {
    struct held *prev;
    struct held *next;
};

/* The blocks held, the newest first. */
static struct held *heap;

static void hold(struct held *h) {
    h->prev = NULL;
    h->next = heap;
    if (heap != NULL) {
        heap->prev = h;
    }
    heap = h;
}

static void unhold(struct held *h) {
    if (h->prev != NULL) {
        h->prev->next = h->next;
    } else {
        heap = h->next;
    }
    if (h->next != NULL) {
        h->next->prev = h->prev;
    }
}

void *heap_alloc(size_t n) {
    if (n > 0xffffffffu) {
        die("out of memory");
    }
    struct held *h = malloc(sizeof(struct held) + n);
    if (h == NULL) {
        die("out of memory");
    }
    hold(h);
    return h + 1;
}

void *heap_calloc(size_t n, size_t size) {
    if (size != 0 && n > 0xffffffffu / size) {
        die("out of memory");
    }
    void *p = heap_alloc(n * size);
    memset(p, 0, n * size);
    return p;
}

void *heap_realloc(void *p, size_t n) {
    if (p == NULL) {
        return heap_alloc(n);
    }
    if (n > 0xffffffffu) {
        die("out of memory");
    }
    struct held *h = (struct held *)p - 1;
    unhold(h);
    struct held *q = realloc(h, sizeof(struct held) + n);
    if (q == NULL) {
        die("out of memory");
    }
    hold(q);
    return q + 1;
}

void heap_free(void *p) {
    if (p == NULL) {
        return;
    }
    struct held *h = (struct held *)p - 1;
    unhold(h);
    free(h);
}

/* The identifiers interned in the arena (see intern). */
static struct map interned;

void arena_free(void) {
    free_blocks(chunks);
    free_blocks(bigs);
    chunks = NULL;
    bigs = NULL;
    /* And the heap's blocks, the interned names' table among them. */
    while (heap != NULL) {
        struct held *next = heap->next;
        free(heap);
        heap = next;
    }
    memset(&interned, 0, sizeof interned);
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
    void *q = heap_realloc(p, (size_t)c * size);
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
    heap_free(b->data);
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
        } else if (*f == 'b') {
            /* Bytes, which may hold a NUL: a pointer and a length. */
            const char *s = va_arg(ap, const char *);
            buf_put(b, s, va_arg(ap, uint));
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

char *fmtn(uint *len, const char *f, ...) {
    struct buf b;
    memset(&b, 0, sizeof b);
    va_list ap;
    va_start(ap, f);
    vformat(&b, f, ap);
    va_end(ap);
    char *r = buf_arena(&b);
    *len = b.len;
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
    m->keys = heap_calloc(cap, sizeof(char *));
    m->values = heap_calloc(cap, sizeof(void *));
    m->count = 0;
    for (uint i = 0; i < old.cap; i++) {
        if (old.keys[i] != NULL) {
            map_put(m, old.keys[i], old.values[i]);
        }
    }
    heap_free(old.keys);
    heap_free(old.values);
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

/* The interned identifiers (`interned`, above): each text once, the map's
 * value the string. */
const char *intern(const char *s, uint len) {
    /* The key to look for must be NUL-terminated: copy short ones to the
     * stack. */
    char small[64];
    char *key = small;
    if (len >= sizeof small) {
        key = heap_alloc(len + 1);
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
        heap_free(key);
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

/* Rust's String::from_utf8_lossy: the bytes as UTF-8, each maximal part of
 * an ill-formed sequence (as Unicode defines it) replaced by U+FFFD; its
 * length in *out_len. */
char *from_utf8_lossy(const uchar *s, uint n, uint *out_len) {
    struct buf b;
    memset(&b, 0, sizeof b);
    uint i = 0;
    while (i < n) {
        uint c = s[i];
        if (c < 0x80) {
            buf_byte(&b, c);
            i++;
            continue;
        }
        /* How many bytes the sequence has, and the range its second byte
         * must be in. */
        uint len = 0;
        uint lo = 0x80;
        uint hi = 0xbf;
        if (c >= 0xc2 && c <= 0xdf) {
            len = 2;
        } else if (c >= 0xe0 && c <= 0xef) {
            len = 3;
            if (c == 0xe0) {
                lo = 0xa0;
            } else if (c == 0xed) {
                hi = 0x9f;
            }
        } else if (c >= 0xf0 && c <= 0xf4) {
            len = 4;
            if (c == 0xf0) {
                lo = 0x90;
            } else if (c == 0xf4) {
                hi = 0x8f;
            }
        }
        if (len == 0) {
            buf_str(&b, "\xef\xbf\xbd");
            i++;
            continue;
        }
        uint k = 1;
        while (k < len && i + k < n) {
            uint d = s[i + k];
            uint l = k == 1 ? lo : 0x80;
            uint h = k == 1 ? hi : 0xbf;
            if (d < l || d > h) {
                break;
            }
            k++;
        }
        if (k == len) {
            buf_put(&b, s + i, len);
        } else {
            buf_str(&b, "\xef\xbf\xbd");
        }
        i += k;
    }
    char *r = buf_arena(&b);
    *out_len = b.len;
    buf_free(&b);
    return r;
}

/* ---- 128-bit integers ----
 * Two's complement in two 64-bit halves: adding, subtracting and
 * multiplying are the same for signed and unsigned numbers, done on the
 * halves with their carries; the rest look at the sign (the top bit of
 * hi). */

i128 i128_of(int64_t v) {
    i128 r;
    r.lo = (uint64_t)v;
    r.hi = v < 0 ? ~(uint64_t)0 : 0;
    return r;
}

i128 i128_of_u64(uint64_t v) {
    i128 r;
    r.lo = v;
    r.hi = 0;
    return r;
}

int i128_is_zero(i128 a) {
    return a.lo == 0 && a.hi == 0;
}

int i128_eq(i128 a, i128 b) {
    return a.lo == b.lo && a.hi == b.hi;
}

int i128_neg_p(i128 a) {
    return (a.hi >> 63) != 0;
}

int i128_cmp(i128 a, i128 b) {
    /* By the high halves as signed numbers, then the low ones unsigned. */
    int64_t ah = (int64_t)a.hi;
    int64_t bh = (int64_t)b.hi;
    if (ah != bh) {
        return ah < bh ? -1 : 1;
    }
    if (a.lo != b.lo) {
        return a.lo < b.lo ? -1 : 1;
    }
    return 0;
}

int i128_lt(i128 a, i128 b) {
    return i128_cmp(a, b) < 0;
}

int i128_le(i128 a, i128 b) {
    return i128_cmp(a, b) <= 0;
}

i128 i128_min(i128 a, i128 b) {
    return i128_lt(b, a) ? b : a;
}

i128 i128_max(i128 a, i128 b) {
    return i128_lt(a, b) ? b : a;
}

i128 i128_add(i128 a, i128 b) {
    i128 r;
    r.lo = a.lo + b.lo;
    r.hi = a.hi + b.hi + (r.lo < a.lo ? 1 : 0);
    return r;
}

i128 i128_not(i128 a) {
    i128 r;
    r.lo = ~a.lo;
    r.hi = ~a.hi;
    return r;
}

i128 i128_neg(i128 a) {
    return i128_add(i128_not(a), i128_of(1));
}

i128 i128_sub(i128 a, i128 b) {
    return i128_add(a, i128_neg(b));
}

/* The 128-bit product of two 64-bit numbers, from their 32-bit halves. */
static i128 mul_64(uint64_t a, uint64_t b) {
    uint64_t a0 = a & 0xffffffffu;
    uint64_t a1 = a >> 32;
    uint64_t b0 = b & 0xffffffffu;
    uint64_t b1 = b >> 32;
    uint64_t p00 = a0 * b0;
    uint64_t p01 = a0 * b1;
    uint64_t p10 = a1 * b0;
    uint64_t p11 = a1 * b1;
    uint64_t mid = (p00 >> 32) + (p01 & 0xffffffffu) + (p10 & 0xffffffffu);
    i128 r;
    r.lo = (p00 & 0xffffffffu) | (mid << 32);
    r.hi = p11 + (p01 >> 32) + (p10 >> 32) + (mid >> 32);
    return r;
}

i128 i128_mul(i128 a, i128 b) {
    /* The low 128 bits of the product: the high halves' cross terms only
     * reach the high half. */
    i128 r = mul_64(a.lo, b.lo);
    r.hi += a.lo * b.hi + a.hi * b.lo;
    return r;
}

i128 i128_and(i128 a, i128 b) {
    i128 r;
    r.lo = a.lo & b.lo;
    r.hi = a.hi & b.hi;
    return r;
}

i128 i128_or(i128 a, i128 b) {
    i128 r;
    r.lo = a.lo | b.lo;
    r.hi = a.hi | b.hi;
    return r;
}

i128 i128_xor(i128 a, i128 b) {
    i128 r;
    r.lo = a.lo ^ b.lo;
    r.hi = a.hi ^ b.hi;
    return r;
}

i128 i128_shl(i128 a, uint n) {
    i128 r;
    if (n == 0) {
        return a;
    }
    if (n >= 64) {
        r.hi = a.lo << (n - 64);
        r.lo = 0;
        return r;
    }
    r.hi = a.hi << n | a.lo >> (64 - n);
    r.lo = a.lo << n;
    return r;
}

i128 i128_shr(i128 a, uint n) {
    i128 r;
    uint64_t fill = i128_neg_p(a) ? ~(uint64_t)0 : 0;
    if (n == 0) {
        return a;
    }
    if (n >= 64) {
        r.lo = n == 64 ? a.hi : (a.hi >> (n - 64)) | (fill << (128 - n));
        r.hi = fill;
        return r;
    }
    r.lo = a.lo >> n | a.hi << (64 - n);
    r.hi = a.hi >> n | (fill << (64 - n));
    return r;
}

uint i128_leading_zeros(i128 a) {
    uint n = 0;
    uint64_t w = a.hi;
    if (w == 0) {
        n = 64;
        w = a.lo;
        if (w == 0) {
            return 128;
        }
    }
    while ((w >> 63) == 0) {
        w <<= 1;
        n++;
    }
    return n;
}

/* Unsigned division of a by b (not 0), by shifting and subtracting, a bit
 * at a time; the remainder in *rem. */
static i128 udivmod(i128 a, i128 b, i128 *rem) {
    i128 q = i128_of(0);
    i128 r = i128_of(0);
    for (int i = 127; i >= 0; i--) {
        r = i128_shl(r, 1);
        uint64_t bit = i >= 64 ? (a.hi >> (i - 64)) & 1 : (a.lo >> i) & 1;
        r.lo |= bit;
        /* r >= b, unsigned. */
        if (r.hi > b.hi || (r.hi == b.hi && r.lo >= b.lo)) {
            r = i128_sub(r, b);
            if (i >= 64) {
                q.hi |= (uint64_t)1 << (i - 64);
            } else {
                q.lo |= (uint64_t)1 << i;
            }
        }
    }
    *rem = r;
    return q;
}

i128 i128_div(i128 a, i128 b) {
    int na = i128_neg_p(a);
    int nb = i128_neg_p(b);
    i128 rem;
    i128 q = udivmod(na ? i128_neg(a) : a, nb ? i128_neg(b) : b, &rem);
    return na != nb ? i128_neg(q) : q;
}

i128 i128_rem(i128 a, i128 b) {
    int na = i128_neg_p(a);
    i128 rem;
    udivmod(na ? i128_neg(a) : a, i128_neg_p(b) ? i128_neg(b) : b, &rem);
    /* The remainder has the dividend's sign. */
    return na ? i128_neg(rem) : rem;
}

int i128_fits_i64(i128 a) {
    return a.hi == (((int64_t)a.lo < 0) ? ~(uint64_t)0 : 0);
}

int i128_fits_u64(i128 a) {
    return a.hi == 0;
}

int64_t i128_i64(i128 a) {
    return (int64_t)a.lo;
}

void buf_i128(struct buf *b, i128 a) {
    if (i128_neg_p(a)) {
        buf_byte(b, '-');
        a = i128_neg(a);
    }
    if (a.hi == 0) {
        put_decimal(b, a.lo);
        return;
    }
    /* Digits from the last, by dividing by 10. */
    char digits[48];
    int n = 0;
    i128 ten = i128_of(10);
    while (!i128_is_zero(a)) {
        i128 rem;
        a = udivmod(a, ten, &rem);
        digits[n++] = (char)('0' + rem.lo);
    }
    while (n > 0) {
        buf_byte(b, (uchar)digits[--n]);
    }
}

char *i128_str(i128 a) {
    struct buf b;
    memset(&b, 0, sizeof b);
    buf_i128(&b, a);
    char *r = buf_arena(&b);
    buf_free(&b);
    return r;
}

/* ---- Vectors ---- */

void vec_push(struct vec *l, void *p) {
    l->v = grow(l->v, &l->cap, l->n + 1, sizeof(void *));
    l->v[l->n++] = p;
}

void *vec_last(struct vec *l) {
    return l->v[l->n - 1];
}

void *vec_pop(struct vec *l) {
    return l->v[--l->n];
}

struct vec vec_take(struct vec *l) {
    struct vec r = *l;
    memset(l, 0, sizeof *l);
    return r;
}

void vec_extend(struct vec *l, struct vec *from) {
    for (uint i = 0; i < from->n; i++) {
        vec_push(l, from->v[i]);
    }
}

struct vec vec_split_off(struct vec *l, uint at) {
    struct vec r;
    memset(&r, 0, sizeof r);
    for (uint i = at; i < l->n; i++) {
        vec_push(&r, l->v[i]);
    }
    if (at < l->n) {
        l->n = at;
    }
    return r;
}

void vec_insert(struct vec *l, uint at, void *p) {
    vec_push(l, NULL);
    for (uint i = l->n - 1; i > at; i--) {
        l->v[i] = l->v[i - 1];
    }
    l->v[at] = p;
}

void *vec_remove(struct vec *l, uint at) {
    void *p = l->v[at];
    for (uint i = at; i + 1 < l->n; i++) {
        l->v[i] = l->v[i + 1];
    }
    l->n--;
    return p;
}

void vec_truncate(struct vec *l, uint n) {
    if (n < l->n) {
        l->n = n;
    }
}

/* ---- Position maps ---- */

static uint pos_hash(struct pos p) {
    uint h = 2166136261u;
    h = (h ^ p.file) * 16777619u;
    h = (h ^ p.line) * 16777619u;
    h = (h ^ p.col) * 16777619u;
    return h ^ (h >> 15);
}

static int posmap_slot(struct posmap *m, struct pos key) {
    if (m->cap == 0) {
        return -1;
    }
    for (uint i = pos_hash(key) & (m->cap - 1);; i = (i + 1) & (m->cap - 1)) {
        if (!m->used[i]) {
            return -1;
        }
        if (pos_eq(m->keys[i], key)) {
            return (int)i;
        }
    }
}

void *posmap_get(struct posmap *m, struct pos key) {
    int i = posmap_slot(m, key);
    return i < 0 ? NULL : m->values[i];
}

int posmap_has(struct posmap *m, struct pos key) {
    return posmap_slot(m, key) >= 0;
}

void posmap_put(struct posmap *m, struct pos key, void *value) {
    if ((m->count + 1) * 2 > m->cap) {
        struct posmap old = *m;
        uint cap = m->cap ? m->cap * 2 : 64;
        m->keys = heap_calloc(cap, sizeof(struct pos));
        m->values = heap_calloc(cap, sizeof(void *));
        m->used = heap_calloc(cap, 1);
        m->cap = cap;
        m->count = 0;
        for (uint i = 0; i < old.cap; i++) {
            if (old.used[i]) {
                posmap_put(m, old.keys[i], old.values[i]);
            }
        }
        heap_free(old.keys);
        heap_free(old.values);
        heap_free(old.used);
    }
    for (uint i = pos_hash(key) & (m->cap - 1);; i = (i + 1) & (m->cap - 1)) {
        if (!m->used[i]) {
            m->used[i] = 1;
            m->keys[i] = key;
            m->values[i] = value;
            m->count++;
            return;
        }
        if (pos_eq(m->keys[i], key)) {
            m->values[i] = value;
            return;
        }
    }
}

/* ---- Integer maps ---- */

static uint int_hash(uint64_t k) {
    k ^= k >> 33;
    k *= 0xff51afd7ed558ccdull;
    k ^= k >> 33;
    return (uint)k;
}

static int intmap_slot(struct intmap *m, uint64_t key) {
    if (m->cap == 0) {
        return -1;
    }
    for (uint i = int_hash(key) & (m->cap - 1);; i = (i + 1) & (m->cap - 1)) {
        if (!m->used[i]) {
            return -1;
        }
        if (m->keys[i] == key) {
            return (int)i;
        }
    }
}

void *intmap_get(struct intmap *m, uint64_t key) {
    int i = intmap_slot(m, key);
    return i < 0 ? NULL : m->values[i];
}

int intmap_has(struct intmap *m, uint64_t key) {
    return intmap_slot(m, key) >= 0;
}

void intmap_put(struct intmap *m, uint64_t key, void *value) {
    if ((m->count + 1) * 2 > m->cap) {
        struct intmap old = *m;
        uint cap = m->cap ? m->cap * 2 : 64;
        m->keys = heap_calloc(cap, sizeof(uint64_t));
        m->values = heap_calloc(cap, sizeof(void *));
        m->used = heap_calloc(cap, 1);
        m->cap = cap;
        m->count = 0;
        for (uint i = 0; i < old.cap; i++) {
            if (old.used[i]) {
                intmap_put(m, old.keys[i], old.values[i]);
            }
        }
        heap_free(old.keys);
        heap_free(old.values);
        heap_free(old.used);
    }
    for (uint i = int_hash(key) & (m->cap - 1);; i = (i + 1) & (m->cap - 1)) {
        if (!m->used[i]) {
            m->used[i] = 1;
            m->keys[i] = key;
            m->values[i] = value;
            m->count++;
            return;
        }
        if (m->keys[i] == key) {
            m->values[i] = value;
            return;
        }
    }
}

struct pos *pos_box(struct pos p) {
    struct pos *r = arena_alloc(sizeof *r);
    *r = p;
    return r;
}
