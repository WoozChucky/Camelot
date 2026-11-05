/* ld: memory, buffers, messages and maps (see ld.h). */
#include "ld.h"

typedef __builtin_va_list va_list;
#define va_start(ap, last) __builtin_va_start(ap, last)
#define va_arg(ap, type) __builtin_va_arg(ap, type)
#define va_end(ap) __builtin_va_end(ap)

void die(const char *msg) {
    fprintf(2, "ld: %s\n", msg);
    exit(2);
}

void *xmalloc(uint n) {
    void *p = malloc(n ? n : 1);
    if (p == NULL) {
        die("out of memory");
    }
    return p;
}

void *xcalloc(uint n) {
    void *p = xmalloc(n);
    memset(p, 0, n);
    return p;
}

/* The images ld builds in memory, the output sections' and the file it
 * writes, have sizes and offsets computed in 64 bits, as merlin-ld
 * computes them: on ELF64 an output section can be 4 GiB long, or a
 * section 4 GiB into its segment. merlin-ld then allocates that much and
 * writes it, or stops when the allocation fails. A Camelot process has
 * less memory than 4 GiB, so such a count is refused here the way xmalloc
 * refuses what malloc cannot give, and never cut to 32 bits, which would
 * write a file whose headers point past its end. */
uint mem_len(uint64_t n) {
    if (n > UINT32_MAX) {
        die("out of memory");
    }
    return (uint)n;
}

char *xstrndup(const char *s, uint n) {
    char *r = xmalloc(n + 1);
    memcpy(r, s, n);
    r[n] = 0;
    return r;
}

char *xstrdup(const char *s) {
    return xstrndup(s, (uint)strlen(s));
}

void *grow(void *p, uint *cap, uint need, uint size) {
    if (need <= *cap) {
        return p;
    }
    /* Double the capacity until it holds need, but stop at need itself
     * when doubling would pass 2^32 (a uint doubled past 0x80000000 is 0,
     * and the loop would never end), and refuse a byte count that does
     * not fit in 32 bits: no Camelot process has that much memory. */
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
    void *q = realloc(p, c * size);
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
    b->data = grow(b->data, &b->cap, b->len + n, 1);
    memcpy(b->data + b->len, p, n);
    b->len += n;
}

void buf_byte(struct buf *b, uint v) {
    uchar c = (uchar)v;
    buf_put(b, &c, 1);
}

void buf_u16(struct buf *b, uint v) {
    buf_byte(b, v);
    buf_byte(b, v >> 8);
}

void buf_u32(struct buf *b, uint v) {
    buf_u16(b, v);
    buf_u16(b, v >> 16);
}

void buf_pad(struct buf *b, uint len) {
    if (b->len >= len) {
        return;
    }
    b->data = grow(b->data, &b->cap, len, 1);
    memset(b->data + b->len, 0, len - b->len);
    b->len = len;
}

/* ---- Messages ---- */

static void put_number(struct buf *b, uint64_t v, uint base, uint width, int zero) {
    char digits[24];
    int n = 0;
    /* In 64 bits while the value needs them (on RV32 a 64-bit division is a
     * call to libgcc's routine), in 32 for the rest. */
    while (v > UINT32_MAX) {
        digits[n++] = "0123456789abcdef"[v % base];
        v /= base;
    }
    uint w = (uint)v;
    do {
        digits[n++] = "0123456789abcdef"[w % base];
        w /= base;
    } while (w != 0);
    while ((uint)n < width) {
        buf_byte(b, zero ? '0' : ' ');
        width--;
    }
    while (n > 0) {
        buf_byte(b, (uchar)digits[--n]);
    }
}

char *fmt(const char *f, ...) {
    struct buf b;
    memset(&b, 0, sizeof b);
    va_list ap;
    va_start(ap, f);
    for (; *f; f++) {
        if (*f != '%') {
            buf_byte(&b, (uchar)*f);
            continue;
        }
        f++;
        int zero = 0;
        uint width = 0;
        if (*f == '0') {
            zero = 1;
            f++;
        }
        while (*f >= '0' && *f <= '9') {
            width = width * 10 + (uint)(*f - '0');
            f++;
        }
        /* `ll`: the argument is a long long, int64_t or uint64_t. */
        int wide = 0;
        if (f[0] == 'l' && f[1] == 'l') {
            wide = 1;
            f += 2;
        }
        if (wide && *f == 'd') {
            int64_t v = va_arg(ap, int64_t);
            if (v < 0) {
                buf_byte(&b, '-');
                put_number(&b, 0 - (uint64_t)v, 10, width, zero);
            } else {
                put_number(&b, (uint64_t)v, 10, width, zero);
            }
        } else if (wide && *f == 'u') {
            put_number(&b, va_arg(ap, uint64_t), 10, width, zero);
        } else if (wide && *f == 'x') {
            put_number(&b, va_arg(ap, uint64_t), 16, width, zero);
        } else if (*f == 's') {
            const char *s = va_arg(ap, const char *);
            buf_put(&b, s, (uint)strlen(s));
        } else if (*f == 'd') {
            int v = va_arg(ap, int);
            if (v < 0) {
                buf_byte(&b, '-');
                put_number(&b, 0u - (uint)v, 10, width, zero);
            } else {
                put_number(&b, (uint)v, 10, width, zero);
            }
        } else if (*f == 'u') {
            put_number(&b, va_arg(ap, uint), 10, width, zero);
        } else if (*f == 'x') {
            put_number(&b, va_arg(ap, uint), 16, width, zero);
        } else if (*f == 'c') {
            buf_byte(&b, (uchar)va_arg(ap, int));
        } else {
            buf_byte(&b, (uchar)*f);
        }
    }
    va_end(ap);
    buf_byte(&b, 0);
    return (char *)b.data;
}

struct messages errors;
struct messages warnings;

void add_message(struct messages *m, char *msg) {
    m->msgs = grow(m->msgs, &m->cap, m->n + 1, sizeof(char *));
    m->msgs[m->n++] = msg;
}

void error(char *msg) {
    add_message(&errors, msg);
}

/* ---- Maps ---- */

static uint hash(const char *s) {
    uint h = 2166136261u;
    for (; *s; s++) {
        h = (h ^ (uchar)*s) * 16777619u;
    }
    return h;
}

int map_get(struct strmap *m, const char *key, int *value) {
    if (m->cap == 0) {
        return 0;
    }
    for (uint i = hash(key) & (m->cap - 1);; i = (i + 1) & (m->cap - 1)) {
        if (m->keys[i] == NULL) {
            return 0;
        }
        if (strcmp(m->keys[i], key) == 0) {
            *value = m->values[i];
            return 1;
        }
    }
}

static void map_grow(struct strmap *m) {
    struct strmap old = *m;
    m->cap = old.cap ? old.cap * 2 : 64;
    m->keys = xcalloc(m->cap * sizeof(char *));
    m->values = xcalloc(m->cap * sizeof(int));
    m->count = 0;
    for (uint i = 0; i < old.cap; i++) {
        if (old.keys[i] != NULL) {
            map_put(m, old.keys[i], old.values[i]);
        }
    }
    free(old.keys);
    free(old.values);
}

void map_put(struct strmap *m, char *key, int value) {
    if ((m->count + 1) * 2 > m->cap) {
        map_grow(m);
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

static uint uhash(uint64_t key) {
    uint k = (uint)key ^ (uint)(key >> 32);
    k ^= k >> 16;
    k *= 0x45d9f3bu;
    k ^= k >> 16;
    return k;
}

int umap_get(struct uintmap *m, uint64_t key, int *value) {
    if (m->cap == 0) {
        return 0;
    }
    for (uint i = uhash(key) & (m->cap - 1);; i = (i + 1) & (m->cap - 1)) {
        if (!m->used[i]) {
            return 0;
        }
        if (m->keys[i] == key) {
            *value = m->values[i];
            return 1;
        }
    }
}

void umap_put(struct uintmap *m, uint64_t key, int value) {
    if ((m->count + 1) * 2 > m->cap) {
        struct uintmap old = *m;
        m->cap = old.cap ? old.cap * 2 : 64;
        m->keys = xcalloc(m->cap * sizeof(uint64_t));
        m->values = xcalloc(m->cap * sizeof(int));
        m->used = xcalloc(m->cap);
        m->count = 0;
        for (uint i = 0; i < old.cap; i++) {
            if (old.used[i]) {
                umap_put(m, old.keys[i], old.values[i]);
            }
        }
        free(old.keys);
        free(old.values);
        free(old.used);
    }
    for (uint i = uhash(key) & (m->cap - 1);; i = (i + 1) & (m->cap - 1)) {
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
