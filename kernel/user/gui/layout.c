/* Laying the widgets out (gui.h, "Widgets"): two passes over the tree.
 *
 * measure() goes up from the leaves: each widget's min_w and min_h, the
 * room it asks for. A leaf's comes from its text and the font (a field's
 * from its columns, a list's from its widest item and its rows); a row's is
 * its children's widths added up, with `spacing` between them and
 * `padding` around, and the tallest child's height; a column's the same
 * turned on its side.
 *
 * arrange() goes down from the root: a widget gets a rectangle, and a row
 * or a column shares its own out. Along its axis each child gets what it
 * asked for, and the room left over is split evenly among the children
 * marked GUI_GROW (the first ones a pixel more when it doesn't divide);
 * across it, every child gets the whole height (or width). When there is
 * less room than asked for, the last children are cut off: each child's
 * rectangle is cut to its box's, and one wholly outside it is empty. */
#include "theme.h"

static int is_box(const struct gui_widget *w) {
    return w->kind == GUI_ROW || w->kind == GUI_COLUMN;
}

static void measure(struct gui_widget *w) {
    const struct gui_font *f = w->ui->font;
    int line = theme_line(w->ui);
    switch (w->kind) {
    case GUI_ROW:
    case GUI_COLUMN: {
        int along = 0;
        int across = 0;
        int n = 0;
        for (struct gui_widget *c = w->first; c != NULL; c = c->next) {
            measure(c);
            int a = w->kind == GUI_ROW ? c->min_w : c->min_h;
            int b = w->kind == GUI_ROW ? c->min_h : c->min_w;
            along += a;
            if (b > across) {
                across = b;
            }
            n++;
        }
        if (n > 1) {
            along += (n - 1) * w->spacing;
        }
        along += 2 * w->padding;
        across += 2 * w->padding;
        w->min_w = w->kind == GUI_ROW ? along : across;
        w->min_h = w->kind == GUI_ROW ? across : along;
        break;
    }
    case GUI_LABEL:
        w->min_w = gui_text_width(f, w->text, 0) + 2 * THEME_LABEL_PAD;
        w->min_h = f->height + 2 * THEME_LABEL_PAD;
        break;
    case GUI_BUTTON:
        w->min_w = gui_text_width(f, w->text, 0) + 2 * THEME_BUTTON_PAD_X;
        w->min_h = f->height + 2 * THEME_BUTTON_PAD_Y;
        break;
    case GUI_FIELD:
        w->min_w = w->columns * f->width + 2 * THEME_EDGE_PAD + 2;
        w->min_h = f->height + 2 * THEME_EDGE_PAD + 2;
        break;
    case GUI_LIST: {
        int widest = 8 * f->width;
        for (int i = 0; i < w->count; i++) {
            int tw = gui_text_width(f, w->items[i], 0);
            if (tw > widest) {
                widest = tw;
            }
        }
        w->min_w = widest + 2 * THEME_EDGE_PAD + 2;
        w->min_h = w->rows * line + 2 * THEME_EDGE_PAD;
        break;
    }
    case GUI_SCROLLBAR:
        w->min_w = THEME_SCROLLBAR;
        w->min_h = 3 * THEME_SCROLLBAR;
        break;
    case GUI_MENU:
        w->min_w = gui_text_width(f, w->text, 0) + 2 * THEME_MENU_PAD_X;
        w->min_h = f->height + 2 * THEME_MENU_PAD_Y;
        break;
    }
}

static void arrange(struct gui_widget *w, struct gui_rect r) {
    w->rect = r;
    w->flags |= GUI_DIRTY;
    if (w->kind == GUI_LIST) {
        widget_list_sync(w);
    } else if (w->kind == GUI_FIELD) {
        widget_field_sync(w);
    }
    if (!is_box(w)) {
        return;
    }
    int row = w->kind == GUI_ROW;
    struct gui_rect in = gui_rect(r.x + w->padding, r.y + w->padding, r.w - 2 * w->padding, r.h - 2 * w->padding);
    int room = row ? in.w : in.h;
    int asked = row ? w->min_w - 2 * w->padding : w->min_h - 2 * w->padding;
    int spare = room - asked;
    int growers = 0;
    for (struct gui_widget *c = w->first; c != NULL; c = c->next) {
        if (c->flags & GUI_GROW) {
            growers++;
        }
    }
    int share = spare > 0 && growers > 0 ? spare / growers : 0;
    int extra = spare > 0 && growers > 0 ? spare % growers : 0;
    int at = row ? in.x : in.y;
    for (struct gui_widget *c = w->first; c != NULL; c = c->next) {
        int size = row ? c->min_w : c->min_h;
        if (c->flags & GUI_GROW) {
            size += share;
            if (extra > 0) {
                size++;
                extra--;
            }
        }
        /* Cut to the box: what is cut off isn't drawn over the box's
         * neighbours, and is hit-tested as it is drawn. */
        struct gui_rect cr = row ? gui_rect(at, in.y, size, in.h) : gui_rect(in.x, at, in.w, size);
        arrange(c, gui_rect_intersect(cr, in));
        at += size + w->spacing;
    }
}

void gui_ui_layout(struct gui_ui *ui, struct gui_rect r) {
    if (ui->root == NULL) {
        return;
    }
    measure(ui->root);
    arrange(ui->root, r);
    gui_ui_invalidate(ui, r);
    if (ui->menu != NULL) {
        /* Its title may have moved: close it. */
        ui->menu->flags &= ~GUI_OPEN;
        ui->menu = NULL;
    }
}
