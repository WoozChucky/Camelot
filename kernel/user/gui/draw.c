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

void gui_line(struct gui_buffer *b, int x0, int y0, int x1, int y1, uint color) {
    /* Bresenham's, in all eight octants: err tracks dx * (y - y0) - dy *
     * (x - x0), the distance from the true line, doubled. */
    int dx = x1 > x0 ? x1 - x0 : x0 - x1;
    int dy = y1 > y0 ? y0 - y1 : y1 - y0; /* minus |y1 - y0| */
    int sx = x0 < x1 ? 1 : -1;
    int sy = y0 < y1 ? 1 : -1;
    int err = dx + dy;
    for (;;) {
        gui_plot(b, x0, y0, color);
        if (x0 == x1 && y0 == y1) {
            break;
        }
        int e2 = 2 * err;
        if (e2 >= dy) {
            err += dy;
            x0 += sx;
        }
        if (e2 <= dx) {
            err += dx;
            y0 += sy;
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
    for (int j = 0; j < to.h; j++) {
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
