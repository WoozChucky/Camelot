/* Rectangles (gui.h). The far edges, x + w and y + h, are computed in long:
 * an int rectangle near INT_MAX would wrap round in int. */
#include "../gui.h"

#define BIGGEST 0x7fffffffL

struct gui_rect gui_rect(int x, int y, int w, int h) {
    struct gui_rect r;
    r.x = x;
    r.y = y;
    r.w = w;
    r.h = h;
    return r;
}

int gui_rect_empty(struct gui_rect r) {
    return r.w <= 0 || r.h <= 0;
}

static long right(struct gui_rect r) {
    return (long)r.x + r.w;
}

static long bottom(struct gui_rect r) {
    return (long)r.y + r.h;
}

struct gui_rect gui_rect_intersect(struct gui_rect a, struct gui_rect b) {
    long x0 = a.x > b.x ? a.x : b.x;
    long y0 = a.y > b.y ? a.y : b.y;
    long x1 = right(a) < right(b) ? right(a) : right(b);
    long y1 = bottom(a) < bottom(b) ? bottom(a) : bottom(b);
    if (gui_rect_empty(a) || gui_rect_empty(b) || x1 <= x0 || y1 <= y0) {
        return gui_rect(0, 0, 0, 0);
    }
    /* Inside a, so its size fits in an int. */
    return gui_rect((int)x0, (int)y0, (int)(x1 - x0), (int)(y1 - y0));
}

struct gui_rect gui_rect_union(struct gui_rect a, struct gui_rect b) {
    if (gui_rect_empty(a)) {
        return gui_rect_empty(b) ? gui_rect(0, 0, 0, 0) : b;
    }
    if (gui_rect_empty(b)) {
        return a;
    }
    long x0 = a.x < b.x ? a.x : b.x;
    long y0 = a.y < b.y ? a.y : b.y;
    long x1 = right(a) > right(b) ? right(a) : right(b);
    long y1 = bottom(a) > bottom(b) ? bottom(a) : bottom(b);
    /* The size saturates at INT_MAX: two rectangles far apart in int's
     * range have a union wider than an int holds. */
    long w = x1 - x0 > BIGGEST ? BIGGEST : x1 - x0;
    long h = y1 - y0 > BIGGEST ? BIGGEST : y1 - y0;
    return gui_rect((int)x0, (int)y0, (int)w, (int)h);
}

int gui_rect_contains(struct gui_rect r, int x, int y) {
    return x >= r.x && y >= r.y && x < right(r) && y < bottom(r);
}

int gui_rect_covers(struct gui_rect a, struct gui_rect b) {
    if (gui_rect_empty(b)) {
        return 1;
    }
    return b.x >= a.x && b.y >= a.y && right(b) <= right(a) && bottom(b) <= bottom(a);
}
