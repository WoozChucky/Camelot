/* Widgets (gui.h, "Widgets"): making them, drawing them, and the input.
 *
 * Only the leaves draw: a row or a column is the window's colour, which
 * gui_ui_invalidate clears a rectangle to (ui->background) before the
 * widgets in it are drawn again. A widget draws its whole rectangle, with
 * the buffer clipped to it, so drawing one never touches another; an open
 * menu's items are drawn last, over whatever is under them, and again
 * whenever something under them is. Everything drawn goes into ui->damage. */
#include "theme.h"

int theme_line(const struct gui_ui *ui) {
    return ui->font->height + THEME_LINE_GAP;
}

void gui_ui_init(struct gui_ui *ui, const struct gui_font *font) {
    memset(ui, 0, sizeof(*ui));
    ui->font = font;
    ui->x = -1;
    ui->y = -1;
    gui_keyboard_init(&ui->keyboard, &gui_keymap_us);
}

static void free_tree(struct gui_widget *w) {
    struct gui_widget *c = w->first;
    while (c != NULL) {
        struct gui_widget *next = c->next;
        free_tree(c);
        c = next;
    }
    free(w->chars);
    free(w);
}

void gui_ui_free(struct gui_ui *ui) {
    if (ui->root != NULL) {
        free_tree(ui->root);
    }
    ui->root = NULL;
    ui->focus = NULL;
    ui->hover = NULL;
    ui->grab = NULL;
    ui->menu = NULL;
}

/* ---- Making them ---- */

static struct gui_widget *add(struct gui_ui *ui, struct gui_widget *parent, int kind) {
    struct gui_widget *w = calloc(1, sizeof(struct gui_widget));
    if (w == NULL) {
        return NULL;
    }
    w->kind = kind;
    w->ui = ui;
    w->selected = -1;
    w->flags = GUI_DIRTY;
    if (parent == NULL) {
        parent = ui->root;
    }
    if (parent == NULL) {
        ui->root = w;
        return w;
    }
    w->parent = parent;
    if (parent->last != NULL) {
        parent->last->next = w;
    } else {
        parent->first = w;
    }
    parent->last = w;
    return w;
}

static struct gui_widget *box(struct gui_ui *ui, struct gui_widget *parent, int kind, int spacing, int padding) {
    struct gui_widget *w = add(ui, parent, kind);
    if (w != NULL) {
        w->spacing = spacing;
        w->padding = padding;
    }
    return w;
}

struct gui_widget *gui_row(struct gui_ui *ui, struct gui_widget *parent, int spacing, int padding) {
    return box(ui, parent, GUI_ROW, spacing, padding);
}

struct gui_widget *gui_column(struct gui_ui *ui, struct gui_widget *parent, int spacing, int padding) {
    return box(ui, parent, GUI_COLUMN, spacing, padding);
}

struct gui_widget *gui_label(struct gui_ui *ui, struct gui_widget *parent, const char *text) {
    struct gui_widget *w = add(ui, parent, GUI_LABEL);
    if (w != NULL) {
        w->text = text;
    }
    return w;
}

struct gui_widget *gui_button(struct gui_ui *ui, struct gui_widget *parent, const char *text, gui_action action) {
    struct gui_widget *w = add(ui, parent, GUI_BUTTON);
    if (w != NULL) {
        w->text = text;
        w->action = action;
        w->flags |= GUI_FOCUSABLE;
    }
    return w;
}

struct gui_widget *gui_field(struct gui_ui *ui, struct gui_widget *parent, int columns, int cap, gui_action action) {
    struct gui_widget *w = add(ui, parent, GUI_FIELD);
    if (w == NULL) {
        return NULL;
    }
    if (cap < 1) {
        cap = 1;
    }
    w->chars = malloc((size_t)cap + 1);
    if (w->chars == NULL) {
        return NULL; /* left in the tree, empty; gui_ui_free frees it */
    }
    w->chars[0] = 0;
    w->cap = cap;
    w->columns = columns < 1 ? 1 : columns;
    w->action = action;
    w->flags |= GUI_FOCUSABLE;
    return w;
}

struct gui_widget *gui_scrollbar(struct gui_ui *ui, struct gui_widget *parent, int range, int page,
                                 gui_action action) {
    struct gui_widget *w = add(ui, parent, GUI_SCROLLBAR);
    if (w != NULL) {
        w->action = action;
        gui_scrollbar_set(w, 0, range, page);
    }
    return w;
}

struct gui_widget *gui_list(struct gui_ui *ui, struct gui_widget *parent, const char *const *items, int count,
                            int rows, gui_action action) {
    struct gui_widget *row = gui_row(ui, parent, 0, 0);
    if (row == NULL) {
        return NULL;
    }
    row->flags |= GUI_GROW;
    struct gui_widget *w = add(ui, row, GUI_LIST);
    if (w == NULL) {
        return NULL;
    }
    w->items = items;
    w->count = count;
    w->rows = rows < 1 ? 1 : rows;
    w->action = action;
    w->flags |= GUI_FOCUSABLE | GUI_GROW;
    struct gui_widget *bar = gui_scrollbar(ui, row, count, w->rows, NULL);
    if (bar != NULL) {
        bar->buddy = w;
        w->buddy = bar;
    }
    return w;
}

struct gui_widget *gui_menu(struct gui_ui *ui, struct gui_widget *parent, const char *title,
                            const char *const *items, int count, gui_action action) {
    struct gui_widget *w = add(ui, parent, GUI_MENU);
    if (w != NULL) {
        w->text = title;
        w->items = items;
        w->count = count;
        w->action = action;
    }
    return w;
}

void gui_grow(struct gui_widget *w) {
    w->flags |= GUI_GROW;
}

/* ---- Marking what to draw ---- */

static int is_box(const struct gui_widget *w) {
    return w->kind == GUI_ROW || w->kind == GUI_COLUMN;
}

static void invalidate(struct gui_widget *w, struct gui_rect r) {
    if (gui_rect_empty(gui_rect_intersect(w->rect, r))) {
        return;
    }
    w->flags |= GUI_DIRTY;
    for (struct gui_widget *c = w->first; c != NULL; c = c->next) {
        invalidate(c, r);
    }
}

void gui_ui_invalidate(struct gui_ui *ui, struct gui_rect r) {
    if (ui->root == NULL) {
        return;
    }
    gui_damage_add(&ui->background, gui_rect_intersect(r, ui->root->rect));
    invalidate(ui->root, r);
}

void gui_widget_dirty(struct gui_widget *w) {
    if (is_box(w)) {
        gui_ui_invalidate(w->ui, w->rect);
    } else {
        w->flags |= GUI_DIRTY;
    }
}

/* ---- Setting them ---- */

void gui_set_text(struct gui_widget *w, const char *text) {
    w->text = text;
    gui_widget_dirty(w);
}

/* How many code points the first `bytes` bytes of s hold. */
static int code_points(const char *s, int bytes) {
    const char *end = s + bytes;
    int n = 0;
    while (s < end && gui_utf8_next(&s) != 0) {
        n++;
    }
    return n;
}

/* The byte offset of code point `n` of s (its end if it is shorter). */
static int offset_of(const char *s, int n) {
    const char *p = s;
    while (n-- > 0 && gui_utf8_next(&p) != 0) {
    }
    return (int)(p - s);
}

/* The columns of text a field shows. */
static int field_columns(const struct gui_widget *w) {
    int n = (w->rect.w - 2 * THEME_EDGE_PAD - 2) / w->ui->font->width;
    return n < 1 ? 1 : n;
}

/* Scrolls a field so that its cursor shows. */
static void field_show_cursor(struct gui_widget *w) {
    int at = code_points(w->chars, w->cursor);
    int shown = field_columns(w);
    if (at < w->scroll) {
        w->scroll = at;
    } else if (at > w->scroll + shown - 1) {
        w->scroll = at - shown + 1;
    }
}

void gui_field_set(struct gui_widget *w, const char *text) {
    int len = 0;
    /* Whole code points only, up to cap bytes. */
    const char *p = text;
    while (*p != 0) {
        const char *q = p;
        gui_utf8_next(&q);
        if (len + (int)(q - p) > w->cap) {
            break;
        }
        len += (int)(q - p);
        p = q;
    }
    memcpy(w->chars, text, (size_t)len);
    w->chars[len] = 0;
    w->len = len;
    w->cursor = len;
    w->scroll = 0;
    field_show_cursor(w);
    gui_widget_dirty(w);
}

int widget_list_lines(const struct gui_widget *w) {
    int n = (w->rect.h - 2 * THEME_EDGE_PAD) / theme_line(w->ui);
    return n < 1 ? 1 : n;
}

void widget_list_sync(struct gui_widget *w) {
    int lines = widget_list_lines(w);
    int most = w->count - lines;
    if (w->top > most) {
        w->top = most;
    }
    if (w->top < 0) {
        w->top = 0;
    }
    if (w->buddy != NULL) {
        gui_scrollbar_set(w->buddy, w->top, w->count, lines);
    }
}

void gui_list_set(struct gui_widget *w, const char *const *items, int count) {
    w->items = items;
    w->count = count;
    w->top = 0;
    if (w->selected >= count) {
        w->selected = -1;
    }
    widget_list_sync(w);
    gui_widget_dirty(w);
}

void gui_scrollbar_set(struct gui_widget *w, int value, int range, int page) {
    w->range = range < 0 ? 0 : range;
    w->page = page < 1 ? 1 : page;
    int most = w->range - w->page;
    if (value > most) {
        value = most;
    }
    w->value = value < 0 ? 0 : value;
    gui_widget_dirty(w);
}

/* ---- Drawing ---- */

/* Text in the middle of r, up and down, and from its left plus `left`, or
 * in the middle across too when `left` is negative. */
static void text_in(struct gui_buffer *b, const struct gui_ui *ui, struct gui_rect r, int left, const char *s,
                    uint color) {
    const struct gui_font *f = ui->font;
    int x = left < 0 ? r.x + (r.w - gui_text_width(f, s, 0)) / 2 : r.x + left;
    gui_text(b, f, x, r.y + (r.h - f->height) / 2, s, color, 0);
}

/* A scroll bar's thumb. */
static struct gui_rect thumb(const struct gui_widget *w) {
    struct gui_rect r = w->rect;
    if (w->range <= w->page) {
        return r;
    }
    int len = r.h * w->page / w->range;
    if (len < THEME_THUMB_MIN) {
        len = THEME_THUMB_MIN;
    }
    if (len > r.h) {
        len = r.h;
    }
    int at = (r.h - len) * w->value / (w->range - w->page);
    return gui_rect(r.x, r.y + at, r.w, len);
}

static void draw_button(struct gui_buffer *b, struct gui_widget *w) {
    struct gui_ui *ui = w->ui;
    uint face = THEME_BUTTON;
    if ((w->flags & GUI_PRESSED) && (w->flags & GUI_HOVER)) {
        face = THEME_BUTTON_PRESSED;
    } else if (w->flags & GUI_HOVER) {
        face = THEME_BUTTON_HOVER;
    }
    gui_fill(b, w->rect, THEME_WINDOW);
    gui_fill_round(b, w->rect, THEME_RADIUS, face);
    gui_outline_round(b, w->rect, THEME_RADIUS, ui->focus == w ? THEME_FOCUS : THEME_BORDER);
    text_in(b, ui, w->rect, -1, w->text, THEME_TEXT);
}

static void draw_field(struct gui_buffer *b, struct gui_widget *w) {
    struct gui_ui *ui = w->ui;
    const struct gui_font *f = ui->font;
    gui_fill(b, w->rect, THEME_FIELD);
    gui_outline(b, w->rect, ui->focus == w ? THEME_FOCUS : THEME_BORDER);
    int x = w->rect.x + 1 + THEME_EDGE_PAD;
    int y = w->rect.y + (w->rect.h - f->height) / 2;
    gui_text(b, f, x, y, w->chars + offset_of(w->chars, w->scroll), THEME_TEXT, 0);
    if (ui->focus == w) {
        int col = code_points(w->chars, w->cursor) - w->scroll;
        gui_fill(b, gui_rect(x + col * f->width, y, 1, f->height), THEME_TEXT);
    }
}

static void draw_list(struct gui_buffer *b, struct gui_widget *w) {
    struct gui_ui *ui = w->ui;
    int line = theme_line(ui);
    gui_fill(b, w->rect, THEME_FIELD);
    gui_outline(b, w->rect, ui->focus == w ? THEME_FOCUS : THEME_BORDER);
    int lines = widget_list_lines(w);
    for (int i = 0; i < lines && w->top + i < w->count; i++) {
        int item = w->top + i;
        struct gui_rect r = gui_rect(w->rect.x + 1, w->rect.y + THEME_EDGE_PAD + i * line, w->rect.w - 2, line);
        uint color = THEME_TEXT;
        if (item == w->selected) {
            gui_fill(b, r, THEME_SELECTED);
            color = THEME_SELECTED_TEXT;
        }
        text_in(b, ui, r, THEME_EDGE_PAD, w->items[item], color);
    }
}

static void draw_scrollbar(struct gui_buffer *b, struct gui_widget *w) {
    gui_fill(b, w->rect, THEME_TRACK);
    struct gui_rect t = thumb(w);
    t = gui_rect(t.x + 2, t.y + 1, t.w - 4, t.h - 2);
    gui_fill_round(b, t, THEME_RADIUS, w->ui->grab == w ? THEME_THUMB_HELD : THEME_THUMB);
}

static void draw_menu(struct gui_buffer *b, struct gui_widget *w) {
    struct gui_ui *ui = w->ui;
    uint color = THEME_TEXT;
    if (w->flags & GUI_OPEN) {
        gui_fill(b, w->rect, THEME_SELECTED);
        color = THEME_SELECTED_TEXT;
    } else {
        gui_fill(b, w->rect, (w->flags & GUI_HOVER) ? THEME_BUTTON_HOVER : THEME_WINDOW);
    }
    text_in(b, ui, w->rect, -1, w->text, color);
}

/* The open menu's items, over everything. */
static void draw_popup(struct gui_ui *ui, struct gui_buffer *b) {
    struct gui_widget *m = ui->menu;
    struct gui_rect clip = b->clip;
    gui_clip(b, gui_rect_intersect(clip, ui->popup));
    gui_fill(b, ui->popup, THEME_FIELD);
    gui_outline(b, ui->popup, THEME_BORDER);
    int line = theme_line(ui);
    for (int i = 0; i < m->count; i++) {
        struct gui_rect r = gui_rect(ui->popup.x + 1, ui->popup.y + 2 + i * line, ui->popup.w - 2, line);
        uint color = THEME_TEXT;
        if (i == m->selected) {
            gui_fill(b, r, THEME_SELECTED);
            color = THEME_SELECTED_TEXT;
        }
        text_in(b, ui, r, THEME_MENU_PAD_X - 1, m->items[i], color);
    }
    b->clip = clip;
    gui_damage_add(&ui->damage, ui->popup);
}

static void draw_leaf(struct gui_buffer *b, struct gui_widget *w) {
    struct gui_ui *ui = w->ui;
    struct gui_rect clip = b->clip;
    gui_clip(b, gui_rect_intersect(clip, w->rect));
    switch (w->kind) {
    case GUI_LABEL:
        gui_fill(b, w->rect, THEME_WINDOW);
        text_in(b, ui, w->rect, THEME_LABEL_PAD, w->text, THEME_TEXT);
        break;
    case GUI_BUTTON:
        draw_button(b, w);
        break;
    case GUI_FIELD:
        draw_field(b, w);
        break;
    case GUI_LIST:
        draw_list(b, w);
        break;
    case GUI_SCROLLBAR:
        draw_scrollbar(b, w);
        break;
    case GUI_MENU:
        draw_menu(b, w);
        break;
    }
    b->clip = clip;
    gui_damage_add(&ui->damage, w->rect);
    if (ui->menu != NULL && !gui_rect_empty(gui_rect_intersect(w->rect, ui->popup))) {
        ui->popup_dirty = 1;
    }
}

static void draw_tree(struct gui_buffer *b, struct gui_widget *w) {
    if (!is_box(w) && (w->flags & GUI_DIRTY)) {
        draw_leaf(b, w);
    }
    w->flags &= ~GUI_DIRTY;
    for (struct gui_widget *c = w->first; c != NULL; c = c->next) {
        draw_tree(b, c);
    }
}

void gui_ui_draw(struct gui_ui *ui, struct gui_buffer *b) {
    if (ui->root == NULL) {
        return;
    }
    for (int i = 0; i < ui->background.count; i++) {
        struct gui_rect r = ui->background.rects[i];
        gui_fill(b, r, THEME_WINDOW);
        gui_damage_add(&ui->damage, r);
        if (ui->menu != NULL && !gui_rect_empty(gui_rect_intersect(r, ui->popup))) {
            ui->popup_dirty = 1;
        }
    }
    gui_damage_clear(&ui->background);
    draw_tree(b, ui->root);
    if (ui->menu != NULL && ui->popup_dirty) {
        draw_popup(ui, b);
    }
    ui->popup_dirty = 0;
}

/* ---- Finding them ---- */

static struct gui_widget *hit(struct gui_widget *w, int x, int y) {
    if (!gui_rect_contains(w->rect, x, y)) {
        return NULL;
    }
    for (struct gui_widget *c = w->first; c != NULL; c = c->next) {
        struct gui_widget *h = hit(c, x, y);
        if (h != NULL) {
            return h;
        }
    }
    return w;
}

struct gui_widget *gui_ui_hit(struct gui_ui *ui, int x, int y) {
    if (ui->menu != NULL && gui_rect_contains(ui->popup, x, y)) {
        return ui->menu;
    }
    return ui->root == NULL ? NULL : hit(ui->root, x, y);
}

void gui_ui_focus(struct gui_ui *ui, struct gui_widget *w) {
    if (ui->focus == w) {
        return;
    }
    if (ui->focus != NULL) {
        gui_widget_dirty(ui->focus);
    }
    ui->focus = w;
    if (w != NULL) {
        gui_widget_dirty(w);
    }
}

/* The focusable widgets in tree order: how many, and where `w` is among
 * them in *at (left as it was if it isn't). Stores the first `max` in
 * out. */
static int focusables(struct gui_widget *t, struct gui_widget *w, int *at, struct gui_widget **out, int max,
                      int n) {
    if (t->flags & GUI_FOCUSABLE) {
        if (t == w) {
            *at = n;
        }
        if (n < max) {
            out[n] = t;
        }
        n++;
    }
    for (struct gui_widget *c = t->first; c != NULL; c = c->next) {
        n = focusables(c, w, at, out, max, n);
    }
    return n;
}

/* The most focusable widgets Tab goes through. */
#define MAX_FOCUSABLE 64

/* The focusable widget `step` (1 or -1) from the focus, round the end. */
static void move_focus(struct gui_ui *ui, int step) {
    if (ui->root == NULL) {
        return;
    }
    struct gui_widget *all[MAX_FOCUSABLE];
    int at = -1;
    int n = focusables(ui->root, ui->focus, &at, all, MAX_FOCUSABLE, 0);
    if (n > MAX_FOCUSABLE) {
        n = MAX_FOCUSABLE;
    }
    if (n == 0) {
        return;
    }
    if (at >= n) {
        at = -1;
    }
    int want = at < 0 ? (step > 0 ? 0 : n - 1) : (at + step + n) % n;
    gui_ui_focus(ui, all[want]);
}

/* ---- Menus ---- */

static void open_menu(struct gui_ui *ui, struct gui_widget *m) {
    const struct gui_font *f = ui->font;
    int widest = 0;
    for (int i = 0; i < m->count; i++) {
        int tw = gui_text_width(f, m->items[i], 0);
        if (tw > widest) {
            widest = tw;
        }
    }
    int w = widest + 2 * THEME_MENU_PAD_X;
    if (w < m->rect.w) {
        w = m->rect.w;
    }
    ui->popup = gui_rect(m->rect.x, m->rect.y + m->rect.h, w, m->count * theme_line(ui) + 4);
    ui->menu = m;
    ui->popup_dirty = 1;
    m->flags |= GUI_OPEN;
    m->selected = -1;
    gui_widget_dirty(m);
}

static void close_menu(struct gui_ui *ui) {
    struct gui_widget *m = ui->menu;
    if (m == NULL) {
        return;
    }
    m->flags &= ~GUI_OPEN;
    ui->menu = NULL;
    gui_widget_dirty(m);
    gui_ui_invalidate(ui, ui->popup);
}

/* The menu item at y, or -1. */
static int popup_item(struct gui_ui *ui, int y) {
    int i = (y - ui->popup.y - 2) / theme_line(ui);
    if (y < ui->popup.y + 2 || i >= ui->menu->count) {
        return -1;
    }
    return i;
}

static void choose(struct gui_ui *ui, int item) {
    struct gui_widget *m = ui->menu;
    close_menu(ui);
    if (item >= 0 && m->action != NULL) {
        m->action(m, item);
    }
}

/* ---- Lists and scroll bars ---- */

static void list_scroll(struct gui_widget *w, int top) {
    int old = w->top;
    w->top = top;
    widget_list_sync(w);
    if (w->top != old) {
        gui_widget_dirty(w);
    }
}

static void list_select(struct gui_widget *w, int item) {
    if (item < 0) {
        item = 0;
    }
    if (item >= w->count) {
        item = w->count - 1;
    }
    if (item < 0 || item == w->selected) {
        return;
    }
    w->selected = item;
    gui_widget_dirty(w);
    int lines = widget_list_lines(w);
    if (item < w->top) {
        list_scroll(w, item);
    } else if (item >= w->top + lines) {
        list_scroll(w, item - lines + 1);
    }
    if (w->action != NULL) {
        w->action(w, item);
    }
}

/* Moves a scroll bar to `value`, and its list with it. */
static void scroll_to(struct gui_widget *w, int value) {
    int old = w->value;
    gui_scrollbar_set(w, value, w->range, w->page);
    if (w->value == old) {
        return;
    }
    if (w->buddy != NULL) {
        list_scroll(w->buddy, w->value);
    }
    if (w->action != NULL) {
        w->action(w, w->value);
    }
}

/* The scroll bar's value with its thumb's top at y. */
static void drag_thumb(struct gui_widget *w, int y) {
    struct gui_rect t = thumb(w);
    int travel = w->rect.h - t.h;
    if (travel <= 0) {
        return;
    }
    int at = y - w->grab_offset - w->rect.y;
    int range = w->range - w->page;
    scroll_to(w, (at * range + travel / 2) / travel);
}

/* ---- Fields ---- */

static void field_insert(struct gui_widget *w, uint code) {
    char enc[4];
    int n = gui_utf8_put(enc, code);
    if (w->len + n > w->cap) {
        return;
    }
    memmove(w->chars + w->cursor + n, w->chars + w->cursor, (size_t)(w->len - w->cursor + 1));
    memcpy(w->chars + w->cursor, enc, (size_t)n);
    w->len += n;
    w->cursor += n;
}

/* The byte offset of the code point before the cursor. */
static int field_back(const struct gui_widget *w) {
    int at = w->cursor;
    while (at > 0) {
        at--;
        if (((uchar)w->chars[at] & 0xc0) != 0x80) {
            break;
        }
    }
    return at;
}

static int field_forward(const struct gui_widget *w) {
    const char *p = w->chars + w->cursor;
    gui_utf8_next(&p);
    return (int)(p - w->chars);
}

static void field_delete(struct gui_widget *w, int from, int to) {
    memmove(w->chars + from, w->chars + to, (size_t)(w->len - to + 1));
    w->len -= to - from;
    w->cursor = from;
}

static void field_key(struct gui_widget *w, uint code, uint ch) {
    switch (code) {
    case KEY_LEFT:
        w->cursor = field_back(w);
        break;
    case KEY_RIGHT:
        w->cursor = field_forward(w);
        break;
    case KEY_HOME:
        w->cursor = 0;
        break;
    case KEY_END:
        w->cursor = w->len;
        break;
    case KEY_BACKSPACE:
        if (w->cursor > 0) {
            field_delete(w, field_back(w), w->cursor);
        }
        break;
    case KEY_DELETE:
        if (w->cursor < w->len) {
            field_delete(w, w->cursor, field_forward(w));
        }
        break;
    case KEY_ENTER:
        if (w->action != NULL) {
            w->action(w, 0);
        }
        return;
    default:
        if (ch < 0x20 || ch == 0x7f) {
            return;
        }
        field_insert(w, ch);
        break;
    }
    field_show_cursor(w);
    gui_widget_dirty(w);
}

/* The cursor at the code point under x. */
static void field_click(struct gui_widget *w, int x) {
    int col = (x - w->rect.x - 1 - THEME_EDGE_PAD + w->ui->font->width / 2) / w->ui->font->width;
    if (col < 0) {
        col = 0;
    }
    w->cursor = offset_of(w->chars, w->scroll + col);
    field_show_cursor(w);
    gui_widget_dirty(w);
}

/* ---- The input ---- */

/* The widget under the pointer, if it is one that shows it: a button or a
 * menu's title. */
static void set_hover(struct gui_ui *ui, struct gui_widget *w) {
    if (w != NULL && w->kind != GUI_BUTTON && w->kind != GUI_MENU) {
        w = NULL;
    }
    if (w == ui->hover) {
        return;
    }
    if (ui->hover != NULL) {
        ui->hover->flags &= ~GUI_HOVER;
        gui_widget_dirty(ui->hover);
    }
    ui->hover = w;
    if (w != NULL) {
        w->flags |= GUI_HOVER;
        gui_widget_dirty(w);
    }
}

void gui_ui_pointer(struct gui_ui *ui, int x, int y) {
    ui->x = x;
    ui->y = y;
    struct gui_widget *g = ui->grab;
    if (g != NULL) {
        if (g->kind == GUI_SCROLLBAR) {
            drag_thumb(g, y);
        } else if (g->kind == GUI_BUTTON) {
            /* Held: it shows pressed only while the pointer is over it. */
            set_hover(ui, gui_rect_contains(g->rect, x, y) ? g : NULL);
        }
        return;
    }
    if (ui->menu != NULL && gui_rect_contains(ui->popup, x, y)) {
        int item = popup_item(ui, y);
        if (item != ui->menu->selected) {
            ui->menu->selected = item;
            ui->popup_dirty = 1;
        }
        set_hover(ui, NULL);
        return;
    }
    set_hover(ui, gui_ui_hit(ui, x, y));
}

void gui_ui_leave(struct gui_ui *ui) {
    ui->x = -1;
    ui->y = -1;
    if (ui->grab == NULL) {
        set_hover(ui, NULL);
    }
}

static void press(struct gui_ui *ui, struct gui_widget *w) {
    int x = ui->x;
    int y = ui->y;
    if (w->flags & GUI_FOCUSABLE) {
        gui_ui_focus(ui, w);
    }
    switch (w->kind) {
    case GUI_BUTTON:
        w->flags |= GUI_PRESSED;
        ui->grab = w;
        gui_widget_dirty(w);
        break;
    case GUI_MENU:
        open_menu(ui, w);
        break;
    case GUI_LIST: {
        int line = (y - w->rect.y - THEME_EDGE_PAD) / theme_line(ui);
        if (y >= w->rect.y + THEME_EDGE_PAD && line < widget_list_lines(w) && w->top + line < w->count) {
            list_select(w, w->top + line);
        }
        break;
    }
    case GUI_SCROLLBAR: {
        struct gui_rect t = thumb(w);
        if (gui_rect_contains(t, x, y)) {
            w->grab_offset = y - t.y;
            ui->grab = w;
            gui_widget_dirty(w);
        } else {
            scroll_to(w, w->value + (y < t.y ? -w->page : w->page));
        }
        break;
    }
    case GUI_FIELD:
        field_click(w, x);
        break;
    }
}

void gui_ui_button(struct gui_ui *ui, uint button, int pressed) {
    if (button != BTN_LEFT) {
        return;
    }
    if (pressed) {
        if (ui->menu != NULL) {
            /* An open menu takes the click: an item, or anywhere else to
             * close it. */
            if (gui_rect_contains(ui->popup, ui->x, ui->y)) {
                choose(ui, popup_item(ui, ui->y));
            } else {
                close_menu(ui);
            }
            return;
        }
        struct gui_widget *w = gui_ui_hit(ui, ui->x, ui->y);
        if (w != NULL) {
            press(ui, w);
        }
        return;
    }
    struct gui_widget *g = ui->grab;
    if (g == NULL) {
        return;
    }
    ui->grab = NULL;
    gui_widget_dirty(g);
    if (g->kind == GUI_BUTTON) {
        g->flags &= ~GUI_PRESSED;
        int inside = gui_rect_contains(g->rect, ui->x, ui->y);
        set_hover(ui, inside ? g : gui_ui_hit(ui, ui->x, ui->y));
        if (inside && g->action != NULL) {
            g->action(g, 0);
        }
    }
}

void gui_ui_scroll(struct gui_ui *ui, int amount) {
    struct gui_widget *w = gui_ui_hit(ui, ui->x, ui->y);
    if (w == NULL || ui->menu != NULL) {
        return;
    }
    if (w->kind == GUI_LIST && w->buddy != NULL) {
        w = w->buddy;
    }
    if (w->kind == GUI_SCROLLBAR) {
        scroll_to(w, w->value + amount * THEME_WHEEL_LINES);
    } else if (w->kind == GUI_LIST) {
        list_scroll(w, w->top + amount * THEME_WHEEL_LINES);
    }
}

static void menu_key(struct gui_ui *ui, uint code) {
    struct gui_widget *m = ui->menu;
    switch (code) {
    case KEY_UP:
        m->selected = m->selected <= 0 ? m->count - 1 : m->selected - 1;
        ui->popup_dirty = 1;
        break;
    case KEY_DOWN:
        m->selected = m->selected + 1 >= m->count ? 0 : m->selected + 1;
        ui->popup_dirty = 1;
        break;
    case KEY_ENTER:
        choose(ui, m->selected);
        break;
    case KEY_ESC:
        close_menu(ui);
        break;
    }
}

static void list_key(struct gui_widget *w, uint code) {
    int lines = widget_list_lines(w);
    int at = w->selected;
    switch (code) {
    case KEY_UP:
        list_select(w, at < 0 ? 0 : at - 1);
        break;
    case KEY_DOWN:
        list_select(w, at + 1);
        break;
    case KEY_PAGEUP:
        list_select(w, at - lines);
        break;
    case KEY_PAGEDOWN:
        list_select(w, at < 0 ? lines - 1 : at + lines);
        break;
    case KEY_HOME:
        list_select(w, 0);
        break;
    case KEY_END:
        list_select(w, w->count - 1);
        break;
    }
}

void gui_ui_key(struct gui_ui *ui, uint code, int pressed) {
    uint ch = gui_keyboard_key(&ui->keyboard, code, pressed);
    if (!pressed) {
        return;
    }
    if (ui->menu != NULL) {
        menu_key(ui, code);
        return;
    }
    if (code == KEY_TAB) {
        move_focus(ui, (ui->keyboard.mods & GUI_MOD_SHIFT) ? -1 : 1);
        return;
    }
    struct gui_widget *w = ui->focus;
    if (w == NULL) {
        return;
    }
    switch (w->kind) {
    case GUI_BUTTON:
        if ((code == KEY_ENTER || code == KEY_SPACE) && w->action != NULL) {
            w->action(w, 0);
        }
        break;
    case GUI_FIELD:
        if (!(ui->keyboard.mods & (GUI_MOD_CTRL | GUI_MOD_ALT))) {
            field_key(w, code, ch);
        }
        break;
    case GUI_LIST:
        list_key(w, code);
        break;
    }
}
