/* The user programs' C library: strings and memory, printf on top of the
 * write system call, a line reader on top of read. Linked into every program
 * (with crt0.s and usys.s), so it has no static data of its own: a program
 * that doesn't need .data or .bss gets none. */
#include "user.h"

typedef __builtin_va_list va_list;
#define va_start(ap, last) __builtin_va_start(ap, last)
#define va_arg(ap, type) __builtin_va_arg(ap, type)
#define va_end(ap) __builtin_va_end(ap)

/* ---- strings and memory: byte at a time, simple first ---- */

size_t strlen(const char *s) {
    size_t n = 0;
    while (s[n]) {
        n++;
    }
    return n;
}

int strcmp(const char *a, const char *b) {
    while (*a && *a == *b) {
        a++;
        b++;
    }
    return (int)(uchar)*a - (int)(uchar)*b;
}

char *strchr(const char *s, int c) {
    for (;; s++) {
        if (*s == (char)c) {
            return (char *)s;
        }
        if (*s == 0) {
            return NULL;
        }
    }
}

void *memset(void *dst, int c, size_t n) {
    uchar *d = (uchar *)dst;
    while (n--) {
        *d++ = (uchar)c;
    }
    return dst;
}

void *memcpy(void *dst, const void *src, size_t n) {
    uchar *d = (uchar *)dst;
    const uchar *s = (const uchar *)src;
    while (n--) {
        *d++ = *s++;
    }
    return dst;
}

void *memmove(void *dst, const void *src, size_t n) {
    uchar *d = (uchar *)dst;
    const uchar *s = (const uchar *)src;
    if (d < s) {
        while (n--) {
            *d++ = *s++;
        }
    } else {
        d += n;
        s += n;
        while (n--) {
            *--d = *--s;
        }
    }
    return dst;
}

int memcmp(const void *a, const void *b, size_t n) {
    const uchar *x = (const uchar *)a;
    const uchar *y = (const uchar *)b;
    for (; n; n--, x++, y++) {
        if (*x != *y) {
            return *x < *y ? -1 : 1;
        }
    }
    return 0;
}

int atoi(const char *s) {
    int negative = *s == '-';
    if (negative) {
        s++;
    }
    uint n = 0;
    while (*s >= '0' && *s <= '9') {
        n = n * 10 + (uint)(*s - '0');
        s++;
    }
    return negative ? (int)(0u - n) : (int)n;
}

/* ---- printf ---- */

/* printf's output: buffered on the caller's stack and written in chunks, so a
 * line usually takes one system call. */
#define OUT_SIZE 64

struct out {
    int fd;
    int len;   /* bytes in buf */
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
    if (o->len == OUT_SIZE) {
        flush(o);
    }
    o->buf[o->len] = c;
    o->len++;
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
    char digits[11]; /* a sign and up to 10 digits (4294967295) */
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

static int vfprintf(int fd, const char *fmt, va_list ap) {
    struct out o;
    o.fd = fd;
    o.len = 0;
    o.count = 0;
    o.err = 0;
    while (*fmt) {
        char c = *fmt++;
        if (c != '%') {
            emit(&o, c);
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
            emit(&o, '%'); /* a lone '%' at the end */
            break;
        }
        fmt++;
        if (conv == 'd') {
            int v = va_arg(ap, int);
            /* Negated as unsigned, so INT_MIN works. */
            emit_number(&o, v < 0 ? 0u - (uint)v : (uint)v, 10, v < 0, &sp);
        } else if (conv == 'u') {
            emit_number(&o, va_arg(ap, uint), 10, 0, &sp);
        } else if (conv == 'x') {
            emit_number(&o, va_arg(ap, uint), 16, 0, &sp);
        } else if (conv == 'p') {
            struct spec hex = {8, 0, 1};
            emit(&o, '0');
            emit(&o, 'x');
            emit_number(&o, (uint)va_arg(ap, void *), 16, 0, &hex);
        } else if (conv == 's') {
            const char *s = va_arg(ap, const char *);
            if (s == NULL) {
                s = "(null)";
            }
            emit_text(&o, s, (int)strlen(s), &sp);
        } else if (conv == 'c') {
            char ch = (char)va_arg(ap, int);
            emit_text(&o, &ch, 1, &sp);
        } else if (conv == '%') {
            emit(&o, '%');
        } else {
            /* Unknown: print it as written. */
            emit(&o, '%');
            while (start < fmt) {
                emit(&o, *start++);
            }
        }
    }
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

int puts(const char *s) {
    int n = write(STDOUT, s, (uint)strlen(s));
    if (n < 0) {
        return n;
    }
    return write(STDOUT, "\n", 1);
}

/* ---- input ---- */

int getline(char *buf, int size) {
    /* The console returns one line per read, so one read is enough. */
    int n = read(STDIN, buf, (uint)(size - 1));
    if (n <= 0) {
        return -1;
    }
    if (buf[n - 1] == '\n') {
        n--;
    }
    buf[n] = 0;
    return n;
}

/* ---- the rest ---- */

const char *strerror(int err) {
    switch (err) {
    case EPERM:
        return "operation not permitted";
    case ENOENT:
        return "no such file or directory";
    case E2BIG:
        return "argument list too long";
    case ENOEXEC:
        return "not an executable";
    case EBADF:
        return "bad file descriptor";
    case ECHILD:
        return "no such child";
    case EAGAIN:
        return "no free process slot";
    case ENOMEM:
        return "out of memory";
    case EACCES:
        return "not a regular file";
    case EFAULT:
        return "bad address";
    case EEXIST:
        return "file exists";
    case ENODEV:
        return "no such device";
    case ENOTDIR:
        return "not a directory";
    case EISDIR:
        return "is a directory";
    case EINVAL:
        return "invalid argument";
    case ENFILE:
        return "too many open files in the system";
    case EMFILE:
        return "too many open files";
    case EFBIG:
        return "file too large";
    case ENOSPC:
        return "no space left on the disk";
    case ESPIPE:
        return "not seekable";
    case EROFS:
        return "read-only filesystem";
    case EPIPE:
        return "broken pipe";
    case ENAMETOOLONG:
        return "name too long";
    case ENOSYS:
        return "no such system call";
    case ENOTEMPTY:
        return "directory not empty";
    case EMSGSIZE:
        return "message too long";
    case EADDRINUSE:
        return "address in use";
    case ENETUNREACH:
        return "network unreachable";
    case ENOBUFS:
        return "no buffer space";
    case ETIMEDOUT:
        return "timed out";
    case EHOSTUNREACH:
        return "host unreachable";
    default:
        return "unknown error";
    }
}

int stat(const char *path, struct stat *st) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        return fd;
    }
    int r = fstat(fd, st);
    close(fd);
    return r;
}

void spin(uint n) {
    /* volatile, so the compiler keeps the loop. */
    volatile uint i;
    for (i = 0; i < n; i++) {
    }
}
