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
        int n = (int)write(o->fd, o->buf, (size_t)o->len);
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

/* The digit for `d`, 0 to 15. */
static char digit(uint d) {
    return (char)(d < 10 ? '0' + d : 'a' + (d - 10));
}

/* Divides *n by `base` (2 to 65536), leaves the quotient in *n and returns
 * the remainder: Linux's do_div, which its printf divides digits off with
 * (lib/vsprintf.c, lib/math/div64.c): a divu and a remu. */
static uint do_div(unsigned long long *n, uint base) {
    uint r = (uint)(*n % base);
    *n /= base;
    return r;
}

/* `val` in `base` (10 or 16) with an optional minus sign. An unsigned long
 * long, the widest argument (%llu); every narrower one fits. */
static void emit_number(struct out *o, unsigned long long val, uint base, int negative, const struct spec *sp) {
    char digits[20]; /* up to 20 digits (18446744073709551615) */
    int n = 0;
    do {
        digits[n] = digit(do_div(&val, base));
        n++;
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
        /* The length: the argument's type for d, u and x. None: an int (or
         * unsigned int), 32 bits. 'l': a long, as wide as a register.
         * 'll': a long long, 64 bits too. 'z': a size_t
         * (ssize_t for d), as wide as a long, as on Linux. */
        int length = 0; /* 0: int, 1: long (and z), 2: long long */
        if (*fmt == 'l') {
            length = 1;
            fmt++;
            if (*fmt == 'l') {
                length = 2;
                fmt++;
            }
        } else if (*fmt == 'z') {
            length = 1;
            fmt++;
        }
        char conv = *fmt;
        if (conv == 0) {
            emit(o, '%'); /* a lone '%' at the end */
            break;
        }
        fmt++;
        if (conv == 'd' || conv == 'u' || conv == 'x') {
            /* The argument at its type, made a long long the way C
             * converts it: sign-extended for d, zero-extended for u and x
             * (an unsigned int has no sign to extend). */
            long long v;
            int is_signed = conv == 'd';
            if (length == 2) {
                v = va_arg(ap, long long);
            } else if (length == 1) {
                long l = va_arg(ap, long);
                v = is_signed ? (long long)l : (long long)(unsigned long)l;
            } else {
                int i = va_arg(ap, int);
                v = is_signed ? (long long)i : (long long)(uint)i;
            }
            int negative = is_signed && v < 0;
            /* Negated as unsigned, so INT_MIN, LONG_MIN and LLONG_MIN
             * work. */
            unsigned long long u = (unsigned long long)v;
            emit_number(o, negative ? 0ull - u : u, conv == 'x' ? 16 : 10, negative, &sp);
        } else if (conv == 'p') {
            /* Every digit of a pointer: 16. */
            struct spec hex = {2 * (int)sizeof(void *), 0, 1};
            emit(o, '0');
            emit(o, 'x');
            emit_number(o, (uintptr_t)va_arg(ap, void *), 16, 0, &hex);
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
