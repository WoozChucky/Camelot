/* as: memory, byte buffers and errors (see as.h). */
#include "as.h"

typedef __builtin_va_list va_list;
#define va_start(ap, last) __builtin_va_start(ap, last)
#define va_arg(ap, type) __builtin_va_arg(ap, type)
#define va_end(ap) __builtin_va_end(ap)

/* v in `base`, at least `width` wide (padded with zeros or spaces), its
 * hexadecimal digits in upper case if `upper`. */
static void put_number(struct buf *b, uint64_t v, uint base, uint width, int zero, int upper) {
    const char *names = upper ? "0123456789ABCDEF" : "0123456789abcdef";
    char digits[24];
    int n = 0;
    do {
        digits[n++] = names[v % base];
        v /= base;
    } while (v != 0);
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
                put_number(&b, 0 - (uint64_t)v, 10, width, zero, 0);
            } else {
                put_number(&b, (uint64_t)v, 10, width, zero, 0);
            }
        } else if (wide && *f == 'u') {
            put_number(&b, va_arg(ap, uint64_t), 10, width, zero, 0);
        } else if (wide && (*f == 'x' || *f == 'X')) {
            put_number(&b, va_arg(ap, uint64_t), 16, width, zero, *f == 'X');
        } else if (*f == 's') {
            const char *s = va_arg(ap, const char *);
            buf_put(&b, s, (uint)strlen(s));
        } else if (*f == 'd') {
            int v = va_arg(ap, int);
            if (v < 0) {
                buf_byte(&b, '-');
                put_number(&b, 0u - (uint)v, 10, width, zero, 0);
            } else {
                put_number(&b, (uint)v, 10, width, zero, 0);
            }
        } else if (*f == 'u') {
            put_number(&b, va_arg(ap, uint), 10, width, zero, 0);
        } else if (*f == 'x' || *f == 'X') {
            put_number(&b, va_arg(ap, uint), 16, width, zero, *f == 'X');
        } else if (*f == 'c') {
            buf_byte(&b, (uchar)va_arg(ap, int));
        } else {
            buf_byte(&b, (uchar)*f);
        }
    }
    va_end(ap);
    char *r = tmp_strndup((char *)b.data, b.len);
    free(b.data);
    return r;
}

void die(const char *msg) {
    fprintf(2, "as: %s\n", msg);
    exit(2);
}

/* ---- Memory ---- */

/* An allocator's blocks: each starts with a header, the block before and
 * its own size (struct block), HEADER bytes, which keeps what follows it
 * 8-aligned. */
struct pool {
    char *block;
    uint used;
    uint size;
};

struct block {
    char *prev;
    uint size;
};

static struct pool perm_pool;
static struct pool tmp_pool;

#define BLOCK_SIZE (64 * 1024)
#define HEADER ((sizeof(struct block) + 7) & ~7u)

static void *pool_alloc(struct pool *p, uint n) {
    n = (n + 7) & ~7u;
    if (p->block == NULL || p->used + n > p->size) {
        uint size = n + HEADER > BLOCK_SIZE ? n + HEADER : BLOCK_SIZE;
        char *b = malloc(size);
        if (b == NULL) {
            die("out of memory");
        }
        ((struct block *)b)->prev = p->block;
        ((struct block *)b)->size = size;
        p->block = b;
        p->used = HEADER;
        p->size = size;
    }
    void *r = p->block + p->used;
    p->used += n;
    memset(r, 0, n);
    return r;
}

void *perm_alloc(uint n) {
    return pool_alloc(&perm_pool, n);
}

void *tmp_alloc(uint n) {
    return pool_alloc(&tmp_pool, n);
}

void tmp_reset(void) {
    /* Keep the first block (the oldest), free the rest. */
    if (tmp_pool.block == NULL) {
        return;
    }
    char *b = tmp_pool.block;
    char *prev = ((struct block *)b)->prev;
    while (prev != NULL) {
        free(b);
        b = prev;
        prev = ((struct block *)b)->prev;
    }
    tmp_pool.block = b;
    tmp_pool.used = HEADER;
    tmp_pool.size = ((struct block *)b)->size;
}

char *perm_strdup(const char *s) {
    uint n = (uint)strlen(s);
    char *r = perm_alloc(n + 1);
    memcpy(r, s, n);
    return r;
}

char *tmp_strndup(const char *s, uint n) {
    char *r = tmp_alloc(n + 1);
    memcpy(r, s, n);
    return r;
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

void buf_zeros(struct buf *b, uint n) {
    b->data = grow(b->data, &b->cap, b->len + n, 1);
    memset(b->data + b->len, 0, n);
    b->len += n;
}

void buf_align(struct buf *b, uint a) {
    while (b->len % a != 0) {
        buf_byte(b, 0);
    }
}

/* ---- Errors ---- */

static struct error *errors;
static uint nerrors;
static uint caperrors;
static int phase_now;

void error_phase(int phase) {
    phase_now = phase;
}

void error_at(uint line, const char *msg) {
    errors = grow(errors, &caperrors, nerrors + 1, sizeof(struct error));
    errors[nerrors].line = line;
    errors[nerrors].msg = perm_strdup(msg);
    errors[nerrors].phase = phase_now;
    nerrors++;
}

uint error_count(int phase) {
    uint n = 0;
    for (uint i = 0; i < nerrors; i++) {
        if (errors[i].phase == phase) {
            n++;
        }
    }
    return n;
}

void errors_print(const char *file, int phase) {
    /* Only this phase's. */
    uint kept = 0;
    for (uint i = 0; i < nerrors; i++) {
        if (errors[i].phase == phase) {
            errors[kept++] = errors[i];
        }
    }
    nerrors = kept;
    /* Insertion sort: stable, and there are few. */
    for (uint i = 1; i < nerrors; i++) {
        struct error e = errors[i];
        uint j = i;
        while (j > 0 && errors[j - 1].line > e.line) {
            errors[j] = errors[j - 1];
            j--;
        }
        errors[j] = e;
    }
    for (uint i = 0; i < nerrors; i++) {
        if (errors[i].line > 0) {
            fprintf(2, "%s:%u: error: %s\n", file, errors[i].line, errors[i].msg);
        } else {
            fprintf(2, "%s: error: %s\n", file, errors[i].msg);
        }
    }
}

/* ---- A string -> int map (open addressing) ---- */

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
    m->cap = old.cap ? old.cap * 2 : 256;
    m->keys = calloc(m->cap, sizeof(char *));
    m->values = calloc(m->cap, sizeof(int));
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

/* Sets key to value; `key` must stay alive (perm memory). */
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
