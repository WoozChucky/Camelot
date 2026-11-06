/* Drawing into buffers (gui.h, "Buffers and drawing"): every call works out
 * the part of its shape inside the buffer's clip rectangle, then writes
 * those pixels and no others. Integer arithmetic only. */
#include "../gui.h"

/* The largest buffer gui_buffer_alloc makes, a side: 16384 pixels, so that
 * width * height * 4 fits in an int with room to spare. */
#define MAX_SIDE 16384

/* x / 255 to the nearest, for x up to 255 * 255: with t = x + 128,
 * (t + (t >> 8)) >> 8 is exactly that (Jim Blinn's), with no division. */
static uint div255(uint x) {
    uint t = x + 128;
    return (t + (t >> 8)) >> 8;
}

uint gui_mix(uint dst, uint src) {
    uint a = src >> 24;
    if (a == 255) {
        return src;
    }
    if (a == 0) {
        return dst | 0xff000000u;
    }
    uint na = 255 - a;
    uint r = div255(((src >> 16) & 0xff) * a + ((dst >> 16) & 0xff) * na);
    uint g = div255(((src >> 8) & 0xff) * a + ((dst >> 8) & 0xff) * na);
    uint b = div255((src & 0xff) * a + (dst & 0xff) * na);
    return 0xff000000u | (r << 16) | (g << 8) | b;
}

void gui_buffer_init(struct gui_buffer *b, uint *pixels, int width, int height, int stride) {
    b->pixels = pixels;
    b->width = width;
    b->height = height;
    b->stride = stride;
    b->clip = gui_rect(0, 0, width, height);
}

int gui_buffer_alloc(struct gui_buffer *b, int width, int height) {
    if (width <= 0 || height <= 0 || width > MAX_SIDE || height > MAX_SIDE) {
        return -EINVAL;
    }
    uint *pixels = calloc((size_t)width * (size_t)height, sizeof(uint));
    if (pixels == NULL) {
        return -ENOMEM;
    }
    gui_buffer_init(b, pixels, width, height, width);
    return 0;
}

void gui_buffer_free(struct gui_buffer *b) {
    free(b->pixels);
    b->pixels = NULL;
    b->width = 0;
    b->height = 0;
    b->clip = gui_rect(0, 0, 0, 0);
}

void gui_clip(struct gui_buffer *b, struct gui_rect r) {
    b->clip = gui_rect_intersect(r, gui_rect(0, 0, b->width, b->height));
}

void gui_unclip(struct gui_buffer *b) {
    b->clip = gui_rect(0, 0, b->width, b->height);
}

uint gui_pixel(const struct gui_buffer *b, int x, int y) {
    if (x < 0 || y < 0 || x >= b->width || y >= b->height) {
        return 0;
    }
    return b->pixels[(long)y * b->stride + x];
}

void gui_plot(struct gui_buffer *b, int x, int y, uint color) {
    if (gui_rect_contains(b->clip, x, y)) {
        b->pixels[(long)y * b->stride + x] = color;
    }
}

void gui_fill(struct gui_buffer *b, struct gui_rect r, uint color) {
    r = gui_rect_intersect(r, b->clip);
    for (int y = r.y; y < r.y + r.h; y++) {
        uint *row = &b->pixels[(long)y * b->stride + r.x];
        for (int x = 0; x < r.w; x++) {
            row[x] = color;
        }
    }
}

void gui_fill_blend(struct gui_buffer *b, struct gui_rect r, uint color) {
    r = gui_rect_intersect(r, b->clip);
    for (int y = r.y; y < r.y + r.h; y++) {
        uint *row = &b->pixels[(long)y * b->stride + r.x];
        for (int x = 0; x < r.w; x++) {
            row[x] = gui_mix(row[x], color);
        }
    }
}

void gui_outline(struct gui_buffer *b, struct gui_rect r, uint color) {
    if (gui_rect_empty(r)) {
        return;
    }
    gui_fill(b, gui_rect(r.x, r.y, r.w, 1), color);
    gui_fill(b, gui_rect(r.x, r.y + r.h - 1, r.w, 1), color);
    gui_fill(b, gui_rect(r.x, r.y + 1, 1, r.h - 2), color);
    gui_fill(b, gui_rect(r.x + r.w - 1, r.y + 1, 1, r.h - 2), color);
}

/* The steps i, from 0 to n, whose coordinate p0 + s * i lies in [lo, lo +
 * len): *first to *last (*first > *last when there are none). */
static void line_steps(long p0, int s, long n, long lo, long len, long *first, long *last) {
    long hi = lo + len - 1;
    long from = s > 0 ? lo - p0 : p0 - hi;
    long to = s > 0 ? hi - p0 : p0 - lo;
    *first = from < 0 ? 0 : from;
    *last = to > n ? n : to;
}

/* floor((2 * i * c + a) / (2 * a)), for 0 <= i, c <= a < 2^32, a > 0:
 * i * c fits in 64 bits but twice it may not, so the quotient by a is
 * taken first (i * c = q * a + r, and 2 q a divides by 2 a). */
static long line_step(unsigned long i, unsigned long c, unsigned long a) {
    unsigned long ic = i * c;
    unsigned long q = ic / a;
    unsigned long r = ic % a;
    return (long)(q + (2 * r + a) / (2 * a));
}

void gui_line(struct gui_buffer *b, int x0, int y0, int x1, int y1, uint color) {
    /* Bresenham's line, in all eight octants, but computed a column (or a
     * row) at a time instead of walked from x0, y0: only the columns the
     * clip rectangle holds are visited, however far off the ends are.
     *
     * The walk it computes: with a = |x1 - x0| and c = |y1 - y0|, err (the
     * distance from the true line, doubled) starts at a - c; a step in x
     * adds -c, one in y adds a; x steps when 2 err >= -c, y when
     * 2 err <= a. When a >= c, x steps every time and err stays in
     * (a/2 - c, 3a/2 - c], a window a wide: after i steps, m of them in y,
     * err is a - c - i c + m a, which only one m puts in the window,
     * m = floor((2 i c + a) / 2a). When c > a, y steps every time, err
     * stays in [a - 3c/2, a - c/2), and after j steps, k of them in x,
     * k = floor((2 j a + c) / 2c). */
    long a = x1 > x0 ? (long)x1 - x0 : (long)x0 - x1;
    long c = y1 > y0 ? (long)y1 - y0 : (long)y0 - y1;
    int sx = x0 < x1 ? 1 : -1;
    int sy = y0 < y1 ? 1 : -1;
    struct gui_rect clip = b->clip;
    if (gui_rect_empty(clip)) {
        return;
    }
    long first;
    long last;
    if (a >= c) {
        line_steps(x0, sx, a, clip.x, clip.w, &first, &last);
        for (long i = first; i <= last; i++) {
            long m = a == 0 ? 0 : line_step((unsigned long)i, (unsigned long)c, (unsigned long)a);
            gui_plot(b, (int)(x0 + sx * i), (int)(y0 + sy * m), color);
        }
    } else {
        line_steps(y0, sy, c, clip.y, clip.h, &first, &last);
        for (long j = first; j <= last; j++) {
            long k = line_step((unsigned long)j, (unsigned long)a, (unsigned long)c);
            gui_plot(b, (int)(x0 + sx * k), (int)(y0 + sy * j), color);
        }
    }
}

/* The radius r fits in: at most half the smaller side, and not negative. */
static int fit_radius(struct gui_rect r, int radius) {
    int most = (r.w < r.h ? r.w : r.h) / 2;
    if (radius > most) {
        radius = most;
    }
    return radius < 0 ? 0 : radius;
}

/* How many pixels of row j (0 the outermost) of a corner of radius r lie
 * outside the quarter circle: the first i whose pixel's centre is inside
 * it. With the corner's pixels from 0 to r - 1 and the circle's centre at
 * r (a pixel's centre is at i + 1/2), pixel i, j is inside when
 * (i + 1/2 - r)^2 + (j + 1/2 - r)^2 <= r^2, here doubled to stay in
 * integers. */
static int corner_inset(int r, int j) {
    int dy = 2 * j + 1 - 2 * r;
    for (int i = 0; i < r; i++) {
        int dx = 2 * i + 1 - 2 * r;
        if (dx * dx + dy * dy <= 4 * r * r) {
            return i;
        }
    }
    return r;
}

void gui_fill_round(struct gui_buffer *b, struct gui_rect r, int radius, uint color) {
    if (gui_rect_empty(r)) {
        return;
    }
    radius = fit_radius(r, radius);
    for (int j = 0; j < radius; j++) {
        int in = corner_inset(radius, j);
        gui_fill(b, gui_rect(r.x + in, r.y + j, r.w - 2 * in, 1), color);
        gui_fill(b, gui_rect(r.x + in, r.y + r.h - 1 - j, r.w - 2 * in, 1), color);
    }
    gui_fill(b, gui_rect(r.x, r.y + radius, r.w, r.h - 2 * radius), color);
}

void gui_outline_round(struct gui_buffer *b, struct gui_rect r, int radius, uint color) {
    if (gui_rect_empty(r)) {
        return;
    }
    radius = fit_radius(r, radius);
    /* A corner's row j runs from its inset to just before the inset of the
     * row outside it (one pixel at least), so that the curve has no gaps;
     * the outermost row runs to the straight edge. */
    int outer = radius;
    for (int j = 0; j < radius; j++) {
        int in = corner_inset(radius, j);
        int end = outer - 1 > in ? outer - 1 : in;
        int len = end - in + 1;
        int top = r.y + j;
        int bottom = r.y + r.h - 1 - j;
        gui_fill(b, gui_rect(r.x + in, top, len, 1), color);
        gui_fill(b, gui_rect(r.x + r.w - 1 - end, top, len, 1), color);
        gui_fill(b, gui_rect(r.x + in, bottom, len, 1), color);
        gui_fill(b, gui_rect(r.x + r.w - 1 - end, bottom, len, 1), color);
        outer = in;
    }
    /* The straight edges between the corners. */
    gui_fill(b, gui_rect(r.x + radius, r.y, r.w - 2 * radius, 1), color);
    gui_fill(b, gui_rect(r.x + radius, r.y + r.h - 1, r.w - 2 * radius, 1), color);
    gui_fill(b, gui_rect(r.x, r.y + radius, 1, r.h - 2 * radius), color);
    gui_fill(b, gui_rect(r.x + r.w - 1, r.y + radius, 1, r.h - 2 * radius), color);
}

/* The part of src's `from` that lands inside b's clip at x, y: *to is
 * where it goes in b, and the result where it comes from in src. */
static struct gui_rect blit_area(struct gui_buffer *b, int x, int y, const struct gui_buffer *src,
                                 struct gui_rect from, struct gui_rect *to) {
    struct gui_rect inside = gui_rect_intersect(from, gui_rect(0, 0, src->width, src->height));
    x += inside.x - from.x;
    y += inside.y - from.y;
    *to = gui_rect_intersect(gui_rect(x, y, inside.w, inside.h), b->clip);
    return gui_rect(inside.x + to->x - x, inside.y + to->y - y, to->w, to->h);
}

void gui_blit(struct gui_buffer *b, int x, int y, const struct gui_buffer *src, struct gui_rect from) {
    struct gui_rect to;
    struct gui_rect s = blit_area(b, x, y, src, from, &to);
    /* A copy to higher addresses goes from the bottom row up, so that no
     * row of the same memory (b and src one buffer, or two views of it) is
     * overwritten before it is copied; memmove handles the overlap within a
     * row. Apart, the order makes no difference. */
    uintptr_t first_to = (uintptr_t)&b->pixels[(long)to.y * b->stride + to.x];
    uintptr_t first_from = (uintptr_t)&src->pixels[(long)s.y * src->stride + s.x];
    int up = first_to > first_from;
    for (int n = 0; n < to.h; n++) {
        int j = up ? to.h - 1 - n : n;
        uint *d = &b->pixels[(long)(to.y + j) * b->stride + to.x];
        const uint *p = &src->pixels[(long)(s.y + j) * src->stride + s.x];
        memmove(d, p, (size_t)to.w * sizeof(uint));
    }
}

void gui_blend(struct gui_buffer *b, int x, int y, const struct gui_buffer *src, struct gui_rect from) {
    struct gui_rect to;
    struct gui_rect s = blit_area(b, x, y, src, from, &to);
    for (int j = 0; j < to.h; j++) {
        uint *d = &b->pixels[(long)(to.y + j) * b->stride + to.x];
        const uint *p = &src->pixels[(long)(s.y + j) * src->stride + s.x];
        for (int i = 0; i < to.w; i++) {
            d[i] = gui_mix(d[i], p[i]);
        }
    }
}
