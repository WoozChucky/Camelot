/* Damage (gui.h, "Damage"): a short list of rectangles that never overlap
 * or touch, so that a compositor redraws each damaged pixel once. */
#include "../gui.h"

void gui_damage_clear(struct gui_damage *d) {
    d->count = 0;
}

/* Whether a and b overlap, or share an edge or a corner. */
static int touch(struct gui_rect a, struct gui_rect b) {
    /* The far edges in long, as gui/rect.c computes them. */
    return a.x <= (long)b.x + b.w && b.x <= (long)a.x + a.w && a.y <= (long)b.y + b.h &&
           b.y <= (long)a.y + a.h;
}

static long area(struct gui_rect r) {
    return (long)r.w * r.h;
}

static void remove_at(struct gui_damage *d, int i) {
    d->count--;
    for (; i < d->count; i++) {
        d->rects[i] = d->rects[i + 1];
    }
}

void gui_damage_add(struct gui_damage *d, struct gui_rect r) {
    if (gui_rect_empty(r)) {
        return;
    }
    for (;;) {
        /* Merge r with every rectangle it touches; their union may touch
         * others, so look again from the start after each. */
        int merged = 0;
        for (int i = 0; i < d->count; i++) {
            if (touch(d->rects[i], r)) {
                r = gui_rect_union(d->rects[i], r);
                remove_at(d, i);
                merged = 1;
                break;
            }
        }
        if (merged) {
            continue;
        }
        if (d->count < GUI_DAMAGE_RECTS) {
            d->rects[d->count++] = r;
            return;
        }
        /* Full: merge r with the rectangle that grows the least, then try
         * again (the union may touch another now). */
        int best = 0;
        long best_growth = 0;
        for (int i = 0; i < d->count; i++) {
            long growth = area(gui_rect_union(d->rects[i], r)) - area(d->rects[i]);
            if (i == 0 || growth < best_growth) {
                best = i;
                best_growth = growth;
            }
        }
        r = gui_rect_union(d->rects[best], r);
        remove_at(d, best);
    }
}

struct gui_rect gui_damage_bounds(const struct gui_damage *d) {
    struct gui_rect r = gui_rect(0, 0, 0, 0);
    for (int i = 0; i < d->count; i++) {
        r = gui_rect_union(r, d->rects[i]);
    }
    return r;
}
