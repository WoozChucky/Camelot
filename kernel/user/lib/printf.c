/* Formatted output: printf and fprintf on top of the write system call,
 * snprintf into a buffer, and puts. One formatter, vformat, does the work
 * for all three; where its characters go is a `struct out`. */
#include "../user.h"

typedef __builtin_va_list va_list;
#define va_start(ap, last) __builtin_va_start(ap, last)
#define va_arg(ap, type) __builtin_va_arg(ap, type)
#define va_end(ap) __builtin_va_end(ap)

/* Output to a descriptor is buffered on the caller's stack and written in
 * chunks, so a line usually takes one system call. */
#define OUT_SIZE 64

struct out {
    int fd;    /* where the bytes go, or -1 for `str` */
    char *str; /* snprintf's buffer, `size` bytes */
    int size;
    int len;   /* bytes in buf (to a descriptor) */
    int count; /* bytes formatted in all */
    int err;   /* the first write error, or 0 */
    char buf[OUT_SIZE];
};

static void flush(struct out *o) {
    if (o->len > 0 && o->err == 0) {
        int n = write(o->fd, o->buf, (uint)o->len);
        if (n < 0) {
            o->err = n;
        }
    }
    o->len = 0;
}

static void emit(struct out *o, char c) {
    if (o->fd < 0) {
        /* Into the string, as long as there is room for it and a NUL. */
        if (o->count < o->size - 1) {
            o->str[o->count] = c;
        }
    } else {
        if (o->len == OUT_SIZE) {
            flush(o);
        }
        o->buf[o->len] = c;
        o->len++;
    }
    o->count++;
}

static void emit_pad(struct out *o, char pad, int n) {
    while (n > 0) {
        emit(o, pad);
        n--;
    }
}

/* How a conversion is padded to its width. */
struct spec {
    int width;
    int left; /* '-': pad on the right with spaces */
    int zero; /* '0': pad with zeros after the sign */
};

/* `s` (`len` bytes), padded to the spec's width with spaces. */
static void emit_text(struct out *o, const char *s, int len, const struct spec *sp) {
    if (!sp->left) {
        emit_pad(o, ' ', sp->width - len);
    }
    for (int i = 0; i < len; i++) {
        emit(o, s[i]);
    }
    if (sp->left) {
        emit_pad(o, ' ', sp->width - len);
    }
}

/* `val` in `base` (10 or 16) with an optional minus sign. */
static void emit_number(struct out *o, uint val, uint base, int negative, const struct spec *sp) {
    char digits[10]; /* up to 10 digits (4294967295) */
    int n = 0;
    do {
        uint d = val % base;
        digits[n] = (char)(d < 10 ? '0' + d : 'a' + (d - 10));
        n++;
        val /= base;
    } while (val != 0);
    int len = n + negative;
    if (sp->zero && !sp->left) {
        if (negative) {
            emit(o, '-');
        }
        emit_pad(o, '0', sp->width - len);
    } else {
        if (!sp->left) {
            emit_pad(o, ' ', sp->width - len);
        }
        if (negative) {
            emit(o, '-');
        }
    }
    while (n > 0) {
        n--;
        emit(o, digits[n]);
    }
    if (sp->left) {
        emit_pad(o, ' ', sp->width - len);
    }
}

/* Formats `fmt` with the arguments `ap` into `o`. */
static void vformat(struct out *o, const char *fmt, va_list ap) {
    while (*fmt) {
        char c = *fmt++;
        if (c != '%') {
            emit(o, c);
            continue;
        }
        const char *start = fmt; /* for echoing an unknown conversion */
        struct spec sp;
        sp.width = 0;
        sp.left = 0;
        sp.zero = 0;
        for (;; fmt++) {
            if (*fmt == '-') {
                sp.left = 1;
            } else if (*fmt == '0') {
                sp.zero = 1;
            } else {
                break;
            }
        }
        while (*fmt >= '0' && *fmt <= '9') {
            sp.width = sp.width * 10 + (*fmt - '0');
            fmt++;
        }
        if (*fmt == 'l') {
            fmt++;
        }
        char conv = *fmt;
        if (conv == 0) {
            emit(o, '%'); /* a lone '%' at the end */
            break;
        }
        fmt++;
        if (conv == 'd') {
            int v = va_arg(ap, int);
            /* Negated as unsigned, so INT_MIN works. */
            emit_number(o, v < 0 ? 0u - (uint)v : (uint)v, 10, v < 0, &sp);
        } else if (conv == 'u') {
            emit_number(o, va_arg(ap, uint), 10, 0, &sp);
        } else if (conv == 'x') {
            emit_number(o, va_arg(ap, uint), 16, 0, &sp);
        } else if (conv == 'p') {
            struct spec hex = {8, 0, 1};
            emit(o, '0');
            emit(o, 'x');
            emit_number(o, (uint)va_arg(ap, void *), 16, 0, &hex);
        } else if (conv == 's') {
            const char *s = va_arg(ap, const char *);
            if (s == NULL) {
                s = "(null)";
            }
            emit_text(o, s, (int)strlen(s), &sp);
        } else if (conv == 'c') {
            char ch = (char)va_arg(ap, int);
            emit_text(o, &ch, 1, &sp);
        } else if (conv == '%') {
            emit(o, '%');
        } else {
            /* Unknown: print it as written. */
            emit(o, '%');
            while (start < fmt) {
                emit(o, *start++);
            }
        }
    }
}

/* Formats to descriptor `fd`: the bytes written, or the first error. */
static int vfprintf(int fd, const char *fmt, va_list ap) {
    struct out o;
    o.fd = fd;
    o.str = NULL;
    o.size = 0;
    o.len = 0;
    o.count = 0;
    o.err = 0;
    vformat(&o, fmt, ap);
    flush(&o);
    return o.err != 0 ? o.err : o.count;
}

int printf(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int n = vfprintf(STDOUT, fmt, ap);
    va_end(ap);
    return n;
}

int fprintf(int fd, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int n = vfprintf(fd, fmt, ap);
    va_end(ap);
    return n;
}

int snprintf(char *buf, size_t size, const char *fmt, ...) {
    struct out o;
    o.fd = -1;
    o.str = buf;
    o.size = size > 0x7fffffffu ? 0x7fffffff : (int)size;
    o.len = 0;
    o.count = 0;
    o.err = 0;
    va_list ap;
    va_start(ap, fmt);
    vformat(&o, fmt, ap);
    va_end(ap);
    if (o.size > 0) {
        buf[o.count < o.size ? o.count : o.size - 1] = 0;
    }
    return o.count;
}

int puts(const char *s) {
    int n = write(STDOUT, s, (uint)strlen(s));
    if (n < 0) {
        return n;
    }
    return write(STDOUT, "\n", 1);
}
