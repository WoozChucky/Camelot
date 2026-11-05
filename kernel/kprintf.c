/* kprintf: the kernel's printf. See kprintf.h for the supported formats. */
#include "kprintf.h"
#include "spinlock.h"
#include "trap.h"
#include "uart.h"

/* Counts what it emits so every entry point can return a length. */
struct out {
    kputc_fn put;
    void *ctx;
    int count;
};

static void emit(struct out *o, char c) {
    o->put(c, o->ctx);
    o->count++;
}

static void emit_pad(struct out *o, char pad, int n) {
    while (n > 0) {
        emit(o, pad);
        n--;
    }
}

/* Prints `val` in `base` (10 or 16) with an optional minus sign, padded to
 * `width` with spaces before the sign or zeros after it. */
static void emit_number(struct out *o, u64 val, u32 base, bool negative, int width, bool zero_pad) {
    char digits[20]; /* 18446744073709551615 has 20 digits; 2^64 - 1 in hex 16 */
    int n = 0;
    do {
        u32 d = (u32)(val % base);
        digits[n] = (char)(d < 10 ? '0' + d : 'a' + (d - 10));
        n++;
        val /= base;
    } while (val != 0);

    int len = n + (negative ? 1 : 0);
    if (!zero_pad) {
        emit_pad(o, ' ', width - len);
    }
    if (negative) {
        emit(o, '-');
    }
    if (zero_pad) {
        emit_pad(o, '0', width - len);
    }
    while (n > 0) {
        n--;
        emit(o, digits[n]);
    }
}

int kvformat(kputc_fn put, void *ctx, const char *fmt, va_list ap) {
    struct out o;
    o.put = put;
    o.ctx = ctx;
    o.count = 0;

    while (*fmt) {
        char c = *fmt++;
        if (c != '%') {
            emit(&o, c);
            continue;
        }

        const char *spec = fmt; /* for echoing unknown conversions */
        bool zero_pad = false;
        int width = 0;
        if (*fmt == '0') {
            zero_pad = true;
            fmt++;
        }
        while (*fmt >= '0' && *fmt <= '9') {
            width = width * 10 + (*fmt - '0');
            fmt++;
        }
        /* Length: none (int), `l` (long: a register's width, 64 bits) or
         * `ll` (long long: 64 bits too), as in C's printf and Linux's
         * printk. */
        int longs = 0;
        while (*fmt == 'l' && longs < 2) {
            longs++;
            fmt++;
        }

        char conv = *fmt;
        if (conv == '\0') {
            /* An incomplete conversion at the end ("%", "%5", "%l"): print
             * it as written. */
            emit(&o, '%');
            while (spec < fmt) {
                emit(&o, *spec++);
            }
            break;
        }
        fmt++;

        switch (conv) {
        case 'd': {
            i64 v;
            if (longs == 2) {
                v = va_arg(ap, i64);
            } else if (longs == 1) {
                v = va_arg(ap, isize);
            } else {
                v = va_arg(ap, i32);
            }
            /* Negate as unsigned so the most negative value works. */
            u64 mag = v < 0 ? 0ull - (u64)v : (u64)v;
            emit_number(&o, mag, 10, v < 0, width, zero_pad);
            break;
        }
        case 'u':
        case 'x': {
            u64 v;
            if (longs == 2) {
                v = va_arg(ap, u64);
            } else if (longs == 1) {
                v = va_arg(ap, usize);
            } else {
                v = va_arg(ap, u32);
            }
            emit_number(&o, v, conv == 'u' ? 10 : 16, false, width, zero_pad);
            break;
        }
        case 'p':
            /* Every hex digit of a pointer: 16. */
            emit(&o, '0');
            emit(&o, 'x');
            emit_number(&o, (uintptr)va_arg(ap, void *), 16, false, (int)(2 * sizeof(void *)), true);
            break;
        case 's': {
            const char *s = va_arg(ap, const char *);
            if (s == NULL) {
                s = "(null)";
            }
            int len = 0;
            while (s[len]) {
                len++;
            }
            emit_pad(&o, ' ', width - len);
            while (*s) {
                emit(&o, *s++);
            }
            break;
        }
        case 'c':
            emit_pad(&o, ' ', width - 1);
            emit(&o, (char)va_arg(ap, int));
            break;
        case '%':
            emit(&o, '%');
            break;
        default:
            /* Unknown conversion: print it as written. */
            emit(&o, '%');
            while (spec < fmt) {
                emit(&o, *spec++);
            }
            break;
        }
    }
    return o.count;
}

static void uart_sink(char c, void *ctx) {
    (void)ctx;
    uart_putc(c);
}

/* The whole message under the UART's lock, so messages from different
 * harts don't mix. Not once the kernel panics: the code that panicked may
 * hold the lock (a panic inside kprintf, say), and the report must get
 * out. */
int kprintf(const char *fmt, ...) {
    /* Another hart is printing a panic: this one stops here, so the report
     * comes out whole (and uart_putc() would stop it anyway). */
    panic_stop_others();
    bool locking = !panicking;
    if (locking) {
        acquire(&uart_tx_lock);
    }
    va_list ap;
    va_start(ap, fmt);
    int n = kvformat(uart_sink, NULL, fmt, ap);
    va_end(ap);
    if (locking) {
        release(&uart_tx_lock);
    }
    return n;
}

struct buf {
    char *p;
    usize size;
    usize len;
};

static void buf_sink(char c, void *ctx) {
    struct buf *b = (struct buf *)ctx;
    if (b->len + 1 < b->size) {
        b->p[b->len] = c;
    }
    b->len++;
}

int ksnprintf(char *buf, usize size, const char *fmt, ...) {
    struct buf b;
    b.p = buf;
    b.size = size;
    b.len = 0;

    va_list ap;
    va_start(ap, fmt);
    int n = kvformat(buf_sink, &b, fmt, ap);
    va_end(ap);

    if (size > 0) {
        buf[b.len < size ? b.len : size - 1] = '\0';
    }
    return n;
}
