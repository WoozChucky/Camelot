/* Rectangles (gui.h). */
#include "../gui.h"

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

struct gui_rect gui_rect_intersect(struct gui_rect a, struct gui_rect b) {
    int x0 = a.x > b.x ? a.x : b.x;
    int y0 = a.y > b.y ? a.y : b.y;
    int x1 = a.x + a.w < b.x + b.w ? a.x + a.w : b.x + b.w;
    int y1 = a.y + a.h < b.y + b.h ? a.y + a.h : b.y + b.h;
    if (gui_rect_empty(a) || gui_rect_empty(b) || x1 <= x0 || y1 <= y0) {
        return gui_rect(0, 0, 0, 0);
    }
    return gui_rect(x0, y0, x1 - x0, y1 - y0);
}

struct gui_rect gui_rect_union(struct gui_rect a, struct gui_rect b) {
    if (gui_rect_empty(a)) {
        return gui_rect_empty(b) ? gui_rect(0, 0, 0, 0) : b;
    }
    if (gui_rect_empty(b)) {
        return a;
    }
    int x0 = a.x < b.x ? a.x : b.x;
    int y0 = a.y < b.y ? a.y : b.y;
    int x1 = a.x + a.w > b.x + b.w ? a.x + a.w : b.x + b.w;
    int y1 = a.y + a.h > b.y + b.h ? a.y + a.h : b.y + b.h;
    return gui_rect(x0, y0, x1 - x0, y1 - y0);
}

int gui_rect_contains(struct gui_rect r, int x, int y) {
    return x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h;
}

int gui_rect_covers(struct gui_rect a, struct gui_rect b) {
    if (gui_rect_empty(b)) {
        return 1;
    }
    return b.x >= a.x && b.y >= a.y && b.x + b.w <= a.x + a.w && b.y + b.h <= a.y + a.h;
}
