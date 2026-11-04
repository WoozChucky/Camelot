/* as: memory, byte buffers and errors (see as.h). */
#include "as.h"

typedef __builtin_va_list va_list;
#define va_start(ap, last) __builtin_va_start(ap, last)
#define va_arg(ap, type) __builtin_va_arg(ap, type)
#define va_end(ap) __builtin_va_end(ap)

/* v in `base`, at least `width` wide (padded with zeros or spaces), its
 * hexadecimal digits in upper case if `upper`. */
static void put_number(struct buf *b, uint v, uint base, uint width, int zero, int upper) {
    const char *names = upper ? "0123456789ABCDEF" : "0123456789abcdef";
    char digits[12];
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
        if (*f == 's') {
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
 * its own size. */
struct pool {
    char *block;
    uint used;
    uint size;
};

static struct pool perm_pool;
static struct pool tmp_pool;

#define BLOCK_SIZE (64 * 1024)
#define HEADER 8

static void *pool_alloc(struct pool *p, uint n) {
    n = (n + 7) & ~7u;
    if (p->block == NULL || p->used + n > p->size) {
        uint size = n + HEADER > BLOCK_SIZE ? n + HEADER : BLOCK_SIZE;
        char *b = malloc(size);
        if (b == NULL) {
            die("out of memory");
        }
        *(char **)b = p->block;
        *(uint *)(b + 4) = size;
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
    char *prev = *(char **)b;
    while (prev != NULL) {
        free(b);
        b = prev;
        prev = *(char **)b;
    }
    tmp_pool.block = b;
    tmp_pool.used = HEADER;
    tmp_pool.size = *(uint *)(b + 4);
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
    uint c = *cap ? *cap : 8;
    while (c < need) {
        c *= 2;
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
