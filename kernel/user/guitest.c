/* guitest: draws the GUI library's test scenes (gui.h) into buffers in
 * memory and writes each as a PPM file, for crates/xtask/tests/gui.rs to
 * compare with the expected images; and prints what the parts that draw
 * nothing do. On the desktop image, which has the fonts and the images.
 *
 *   guitest draw DIR     rectangles, lines, rounded corners, blending,
 *                        clipping, blits: DIR/draw.ppm
 *   guitest text DIR     the font, bold, large, UTF-8, clipped: DIR/text.ppm
 *   guitest image DIR    the desktop's images over a checkerboard:
 *                        DIR/image.ppm
 *   guitest widgets DIR  a window's widgets, then after input: DIR/widgets.ppm,
 *                        DIR/input.ppm, and the actions printed
 *   guitest wayland DIR  a display server of a script's worth for guidemo, over
 *                        /run/wayland-0: its requests printed, its frames in
 *                        DIR/wayland1.ppm to wayland3.ppm
 *   guitest damage       the damage lists of a sequence of rectangles
 *   guitest keys         the characters of key sequences
 *   guitest utf8         decoding and encoding
 *   guitest wire         the wire format reading forged messages
 *
 * Exits 0, or 1 with what went wrong. */
#include "gui.h"
#include "gui/protocol.h"

/* wl_display, always object 1. */
#define WL_DISPLAY_ID 1

#define FONT "/usr/share/fonts/spleen/8x16.psfu"

static int failed;

static void fail(const char *what, int err) {
    printf("guitest: %s: %s\n", what, err < 0 ? strerror(-err) : "failed");
    failed = 1;
}

/* Writes b as DIR/NAME.ppm: "P6", the width and height, 255, then each
 * pixel's red, green and blue. */
static void write_ppm(const char *dir, const char *name, const struct gui_buffer *b) {
    char path[MAXPATH + 1];
    snprintf(path, sizeof(path), "%s/%s.ppm", dir, name);
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        fail(path, fd);
        return;
    }
    char head[32];
    int n = snprintf(head, sizeof(head), "P6\n%d %d\n255\n", b->width, b->height);
    write(fd, head, (size_t)n);
    uchar *row = malloc((size_t)b->width * 3);
    if (row == NULL) {
        fail("malloc", -ENOMEM);
        close(fd);
        return;
    }
    for (int y = 0; y < b->height; y++) {
        for (int x = 0; x < b->width; x++) {
            uint p = b->pixels[(long)y * b->stride + x];
            row[3 * x] = (uchar)(p >> 16);
            row[3 * x + 1] = (uchar)(p >> 8);
            row[3 * x + 2] = (uchar)p;
        }
        if (write(fd, row, (size_t)b->width * 3) != b->width * 3) {
            fail(path, -EIO);
            break;
        }
    }
    free(row);
    close(fd);
}

static int new_buffer(struct gui_buffer *b, int w, int h) {
    int r = gui_buffer_alloc(b, w, h);
    if (r < 0) {
        fail("gui_buffer_alloc", r);
    }
    return r;
}

/* ---- draw ---- */

/* Bresenham's line as it is usually written, walked a pixel at a time from
 * one end: what gui_line computes a column or a row at a time. */
static void walk_line(struct gui_buffer *b, int x0, int y0, int x1, int y1, uint color) {
    int dx = x1 > x0 ? x1 - x0 : x0 - x1;
    int dy = y1 > y0 ? y0 - y1 : y1 - y0;
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

/* Whether a and b hold the same pixels. */
static int same_pixels(const struct gui_buffer *a, const struct gui_buffer *b) {
    return memcmp(a->pixels, b->pixels, (size_t)a->width * (size_t)a->height * sizeof(uint)) == 0;
}

/* gui_line draws what the walk draws: every line from a buffer's middle to
 * the points of a square around it, whole and clipped; and lines whose
 * ends lie near the ends of int's range draw only what lands in the buffer,
 * without walking there (or wrapping round). */
static void check_lines(void) {
    struct gui_buffer a;
    struct gui_buffer w;
    if (new_buffer(&a, 40, 40) < 0 || new_buffer(&w, 40, 40) < 0) {
        return;
    }
    for (int clip = 0; clip < 2; clip++) {
        for (int x = 1; x < 40; x++) {
            for (int y = 1; y < 40; y++) {
                gui_fill(&a, gui_rect(0, 0, 40, 40), 0);
                gui_fill(&w, gui_rect(0, 0, 40, 40), 0);
                if (clip) {
                    gui_clip(&a, gui_rect(12, 15, 13, 9));
                    gui_clip(&w, gui_rect(12, 15, 13, 9));
                }
                gui_line(&a, 20, 20, x, y, 0xffffff);
                walk_line(&w, 20, 20, x, y, 0xffffff);
                gui_unclip(&a);
                gui_unclip(&w);
                if (!same_pixels(&a, &w)) {
                    printf("guitest: line 20,20 to %d,%d%s is not the walk's\n", x, y, clip ? ", clipped" : "");
                    failed = 1;
                    clip = 2;
                    x = 40;
                    break;
                }
            }
        }
    }
    /* A diagonal from far off to far off: the buffer's diagonal. A steep
     * line from int's bottom to its top, one column over: x is 0 below y 0
     * and 1 from y 0 on, so the buffer's column 1. */
    gui_fill(&a, gui_rect(0, 0, 40, 40), 0);
    gui_fill(&w, gui_rect(0, 0, 40, 40), 0);
    gui_line(&a, -1000000000, -1000000000, 1000000000, 1000000000, 0xffffff);
    gui_line(&a, 0, -2147483647 - 1, 1, 2147483647, 0xff0000);
    for (int i = 0; i < 40; i++) {
        gui_plot(&w, i, i, 0xffffff);
        gui_plot(&w, 1, i, 0xff0000);
    }
    if (!same_pixels(&a, &w)) {
        printf("guitest: lines from far off are not what they should be\n");
        failed = 1;
    }
    gui_buffer_free(&a);
    gui_buffer_free(&w);
}

/* gui_blit within one buffer, down and up over rows it is copying: each row
 * numbered, then scrolled by 3 rows each way. */
static void check_scroll(void) {
    struct gui_buffer b;
    if (new_buffer(&b, 4, 12) < 0) {
        return;
    }
    for (int dir = 0; dir < 2; dir++) {
        for (int y = 0; y < 12; y++) {
            gui_fill(&b, gui_rect(0, y, 4, 1), (uint)y);
        }
        int down = dir == 0;
        gui_blit(&b, 1, down ? 3 : 0, &b, gui_rect(0, down ? 0 : 3, 3, 9));
        for (int y = 0; y < 12; y++) {
            uint want = down ? (y >= 3 ? (uint)(y - 3) : (uint)y) : (y < 9 ? (uint)(y + 3) : (uint)y);
            if (b.pixels[y * 4 + 0] != (uint)y || b.pixels[y * 4 + 1] != want ||
                b.pixels[y * 4 + 3] != want) {
                printf("guitest: a scroll %s: row %d holds %x, not %x\n", down ? "down" : "up", y,
                       b.pixels[y * 4 + 1], want);
                failed = 1;
                break;
            }
        }
    }
    gui_buffer_free(&b);
}

static void scene_draw(const char *dir) {
    struct gui_buffer b;
    if (new_buffer(&b, 160, 120) < 0) {
        return;
    }
    gui_fill(&b, gui_rect(0, 0, 160, 120), GUI_RGB(0x20, 0x30, 0x40));
    /* Filled and outlined rectangles, and an empty one (nothing). */
    gui_fill(&b, gui_rect(4, 4, 30, 20), GUI_RGB(0xe0, 0x40, 0x40));
    gui_outline(&b, gui_rect(38, 4, 30, 20), GUI_RGB(0xff, 0xff, 0xff));
    gui_fill(&b, gui_rect(70, 4, 0, 20), GUI_RGB(0xff, 0xff, 0x00));
    /* Rounded rectangles: filled, outlined, a radius too big for it. */
    gui_fill_round(&b, gui_rect(72, 4, 40, 24), 8, GUI_RGB(0x40, 0xc0, 0x60));
    gui_outline_round(&b, gui_rect(116, 4, 40, 24), 6, GUI_RGB(0xf0, 0xf0, 0x80));
    gui_fill_round(&b, gui_rect(4, 28, 20, 12), 50, GUI_RGB(0x80, 0x80, 0xf0));
    /* Lines in every octant, from the centre of a star. */
    for (int i = 0; i < 16; i++) {
        static const int ends[16][2] = {{20, 0},  {20, 8},   {20, 20},  {8, 20},  {0, 20},   {-8, 20},
                                        {-20, 20}, {-20, 8}, {-20, 0},  {-20, -8}, {-20, -20}, {-8, -20},
                                        {0, -20}, {8, -20},  {20, -20}, {20, -8}};
        gui_line(&b, 50, 60, 50 + ends[i][0], 60 + ends[i][1], GUI_RGB(0xff, 0xc0, 0x40));
    }
    /* Blending: half-transparent white and a quarter red over the stripes. */
    for (int x = 80; x < 156; x += 8) {
        gui_fill(&b, gui_rect(x, 36, 4, 40), GUI_RGB(0x00, 0x00, 0x00));
    }
    gui_fill_blend(&b, gui_rect(84, 40, 60, 14), GUI_ARGB(0x80, 0xff, 0xff, 0xff));
    gui_fill_blend(&b, gui_rect(84, 58, 60, 14), GUI_ARGB(0x40, 0xff, 0x00, 0x00));
    /* Clipping: a big fill, a line and a rounded outline cut to a box. */
    gui_outline(&b, gui_rect(7, 85, 42, 30), GUI_RGB(0x60, 0x60, 0x60));
    gui_clip(&b, gui_rect(8, 86, 40, 28));
    gui_fill(&b, gui_rect(0, 100, 160, 4), GUI_RGB(0x40, 0x80, 0xff));
    gui_line(&b, 0, 80, 60, 120, GUI_RGB(0xff, 0xff, 0xff));
    gui_outline_round(&b, gui_rect(30, 90, 40, 40), 10, GUI_RGB(0xff, 0x80, 0xff));
    gui_unclip(&b);
    /* A gradient of alpha, blended and blitted. */
    struct gui_buffer g;
    if (new_buffer(&g, 32, 16) == 0) {
        for (int y = 0; y < 16; y++) {
            for (int x = 0; x < 32; x++) {
                g.pixels[y * 32 + x] = GUI_ARGB(x * 8, 0xff, 0xff, y * 16);
            }
        }
        gui_blend(&b, 60, 90, &g, gui_rect(0, 0, 32, 16));
        gui_blit(&b, 96, 90, &g, gui_rect(0, 0, 32, 16));
        /* Parts: from outside the source, and past the buffer's edge. */
        gui_blend(&b, 130, 90, &g, gui_rect(-4, 8, 16, 16));
        gui_blit(&b, 150, 110, &g, gui_rect(0, 0, 32, 16));
        gui_buffer_free(&g);
    }
    /* A copy of part of the buffer itself, its columns overlapping (rows
     * too: check_scroll). */
    gui_blit(&b, 6, 44, &b, gui_rect(4, 4, 30, 20));
    write_ppm(dir, "draw", &b);
    gui_buffer_free(&b);
    check_lines();
    check_scroll();
}

/* ---- text ---- */

static void scene_text(const char *dir) {
    struct gui_font f;
    int r = gui_font_load(&f, FONT);
    if (r < 0) {
        fail(FONT, r);
        return;
    }
    printf("font %dx%d, %d glyphs\n", f.width, f.height, f.glyphs);
    struct gui_buffer b;
    if (new_buffer(&b, 240, 112) == 0) {
        gui_fill(&b, gui_rect(0, 0, 240, 112), GUI_RGB(0xf0, 0xf0, 0xe8));
        int x = gui_text(&b, &f, 2, 2, "Hello, Camelot!", GUI_RGB(0, 0, 0), 0);
        printf("advance %d, width %d\n", x - 2, gui_text_width(&f, "Hello, Camelot!", 0));
        gui_text(&b, &f, 2, 18, "Bold text", GUI_RGB(0x20, 0x40, 0xa0), GUI_BOLD);
        gui_text(&b, &f, 2, 34, "Large", GUI_RGB(0xa0, 0x20, 0x20), GUI_LARGE);
        gui_text(&b, &f, 100, 34, "Both", GUI_RGB(0x20, 0x80, 0x20), GUI_LARGE | GUI_BOLD);
        /* UTF-8: Latin-1, box drawing, a code point the font has not, a
         * broken sequence. */
        gui_text(&b, &f, 2, 70, "caf\xc3\xa9 \xe2\x94\x8c\xe2\x94\x80\xe2\x94\x90 \xe2\x82\xac \xe4\xb8\xad \xff!",
                 GUI_RGB(0, 0, 0), 0);
        /* Clipped: only the middle of the line shows. */
        gui_clip(&b, gui_rect(20, 90, 100, 10));
        gui_text(&b, &f, 2, 88, "clipped clipped clipped", GUI_RGB(0x80, 0, 0x80), 0);
        gui_unclip(&b);
        printf("height %d, large %d\n", gui_text_height(&f, 0), gui_text_height(&f, GUI_LARGE));
        write_ppm(dir, "text", &b);
        gui_buffer_free(&b);
    }
    gui_font_free(&f);
}

/* ---- image ---- */

static void scene_image(const char *dir) {
    struct gui_buffer b;
    if (new_buffer(&b, 96, 48) < 0) {
        return;
    }
    /* A checkerboard of 4-pixel squares, for the transparency to show. */
    for (int y = 0; y < 48; y += 4) {
        for (int x = 0; x < 96; x += 4) {
            uint c = ((x + y) / 4) % 2 ? GUI_RGB(0xc0, 0xc0, 0xc0) : GUI_RGB(0x80, 0x80, 0x80);
            gui_fill(&b, gui_rect(x, y, 4, 4), c);
        }
    }
    static const char *const names[] = {"/usr/share/images/castle.qoi", "/usr/share/images/cursor.qoi"};
    for (int i = 0; i < 2; i++) {
        struct gui_buffer img;
        int r = gui_image_load(&img, names[i]);
        if (r < 0) {
            fail(names[i], r);
            continue;
        }
        printf("%s: %dx%d\n", names[i], img.width, img.height);
        struct gui_rect all = gui_rect(0, 0, img.width, img.height);
        /* Blended, copied (transparent pixels as they are: black), and
         * half off the buffer's edge. */
        gui_blend(&b, 4 + i * 24, 4, &img, all);
        gui_blit(&b, 4 + i * 24, 26, &img, all);
        gui_blend(&b, 88, 36 - i * 30, &img, all);
        gui_buffer_free(&img);
    }
    /* Not QOI, and cut short. */
    struct gui_buffer bad;
    static const uchar junk[] = {'q', 'o', 'i', 'x', 0, 0, 0, 1, 0, 0, 0, 1, 3, 0, 0, 0, 0, 0, 0, 0, 0, 1};
    static const uchar short_[] = {'q', 'o', 'i', 'f', 0, 0, 0, 2, 0, 0, 0, 2, 3, 0, 0xfe, 1, 2, 3,
                                   0, 0, 0, 0, 0, 0, 0, 1};
    printf("junk %d, short %d\n", gui_qoi_decode(&bad, junk, sizeof(junk)),
           gui_qoi_decode(&bad, short_, sizeof(short_)));
    write_ppm(dir, "image", &b);
    gui_buffer_free(&b);
}

/* ---- damage ---- */

static void print_damage(const char *what, const struct gui_damage *d) {
    printf("%s:", what);
    for (int i = 0; i < d->count; i++) {
        struct gui_rect r = d->rects[i];
        printf(" %d,%d %dx%d", r.x, r.y, r.w, r.h);
    }
    struct gui_rect all = gui_damage_bounds(d);
    printf(" (bounds %d,%d %dx%d)\n", all.x, all.y, all.w, all.h);
}

static void scene_damage(void) {
    struct gui_damage d;
    gui_damage_clear(&d);
    gui_damage_add(&d, gui_rect(0, 0, 0, 10));
    print_damage("empty", &d);
    gui_damage_add(&d, gui_rect(10, 10, 10, 10));
    gui_damage_add(&d, gui_rect(40, 10, 10, 10));
    print_damage("apart", &d);
    gui_damage_add(&d, gui_rect(12, 12, 4, 4));
    print_damage("inside", &d);
    gui_damage_add(&d, gui_rect(20, 10, 5, 5));
    print_damage("touching", &d);
    gui_damage_add(&d, gui_rect(24, 12, 17, 2));
    print_damage("bridging", &d);
    gui_damage_clear(&d);
    for (int i = 0; i < GUI_DAMAGE_RECTS + 2; i++) {
        gui_damage_add(&d, gui_rect(i * 20, i * 3, 5, 5));
    }
    print_damage("full", &d);
}

/* ---- widgets ---- */

static void act(struct gui_widget *w, int value) {
    printf("action: %s %d\n", (const char *)w->data, value);
}

static void show_damage(const char *what, struct gui_ui *ui) {
    print_damage(what, &ui->damage);
    gui_damage_clear(&ui->damage);
}

/* A key pressed and let go of. */
static void tap(struct gui_ui *ui, uint code) {
    gui_ui_key(ui, code, 1);
    gui_ui_key(ui, code, 0);
}

static void click(struct gui_ui *ui, int x, int y) {
    gui_ui_pointer(ui, x, y);
    gui_ui_button(ui, BTN_LEFT, 1);
    gui_ui_button(ui, BTN_LEFT, 0);
}

static void centre(struct gui_widget *w, int *x, int *y) {
    *x = w->rect.x + w->rect.w / 2;
    *y = w->rect.y + w->rect.h / 2;
}

/* A field set before the layout shows its text from the start once laid
 * out, scrolls when made too narrow for it and back when widened again; a
 * column too short for its children cuts them to it, the last one off. */
static void check_layout(struct gui_font *f) {
    struct gui_ui ui;
    gui_ui_init(&ui, f);
    struct gui_widget *root = gui_column(&ui, NULL, 2, 0);
    struct gui_widget *field = gui_field(&ui, root, 8, 32, NULL);
    struct gui_widget *a = gui_button(&ui, root, "A", NULL);
    struct gui_widget *b = gui_button(&ui, root, "B", NULL);
    if (field == NULL || a == NULL || b == NULL) {
        fail("check_layout", -ENOMEM);
        gui_ui_free(&ui);
        return;
    }
    gui_field_set(field, "Excalibur");
    gui_ui_layout(&ui, gui_rect(0, 0, 120, 200));
    printf("set before the layout: scroll %d\n", field->scroll);
    gui_ui_layout(&ui, gui_rect(0, 0, 40, 200));
    printf("narrow: scroll %d\n", field->scroll);
    gui_ui_layout(&ui, gui_rect(0, 0, 120, 200));
    printf("wide again: scroll %d\n", field->scroll);
    gui_ui_layout(&ui, gui_rect(0, 0, 120, 40));
    printf("short: A %d,%d %dx%d, B %d,%d %dx%d\n", a->rect.x, a->rect.y, a->rect.w, a->rect.h, b->rect.x,
           b->rect.y, b->rect.w, b->rect.h);
    gui_ui_free(&ui);
}

static void scene_widgets(const char *dir) {
    struct gui_font f;
    int r = gui_font_load(&f, FONT);
    if (r < 0) {
        fail(FONT, r);
        return;
    }
    static const char *const file_items[] = {"Open", "Save", "Quit"};
    static const char *const edit_items[] = {"Cut", "Copy", "Paste"};
    static const char *const knights[] = {"Arthur",   "Lancelot", "Galahad", "Percival", "Gawain",
                                          "Bedivere", "Tristan",  "Kay",     "Gareth",   "Bors"};
    struct gui_ui ui;
    gui_ui_init(&ui, &f);
    struct gui_widget *root = gui_column(&ui, NULL, 4, 4);
    struct gui_widget *bar = gui_row(&ui, root, 0, 0);
    struct gui_widget *file = gui_menu(&ui, bar, "File", file_items, 3, act);
    file->data = "menu File";
    gui_menu(&ui, bar, "Edit", edit_items, 3, act)->data = "menu Edit";
    struct gui_widget *name_row = gui_row(&ui, root, 4, 0);
    gui_label(&ui, name_row, "Name:");
    struct gui_widget *name = gui_field(&ui, name_row, 12, 32, act);
    name->data = "field";
    gui_grow(name);
    struct gui_widget *list = gui_list(&ui, root, knights, 10, 4, act);
    list->data = "list";
    gui_grow(list->parent);
    struct gui_widget *buttons = gui_row(&ui, root, 4, 0);
    struct gui_widget *gap = gui_label(&ui, buttons, "");
    gui_grow(gap);
    gui_button(&ui, buttons, "Cancel", act)->data = "button Cancel";
    struct gui_widget *ok = gui_button(&ui, buttons, "OK", act);
    ok->data = "button OK";

    struct gui_buffer b;
    if (new_buffer(&b, 200, 172) < 0) {
        gui_ui_free(&ui);
        gui_font_free(&f);
        return;
    }
    gui_ui_layout(&ui, gui_rect(0, 0, 200, 172));
    printf("list %d,%d %dx%d, %d lines; ok %d,%d %dx%d\n", list->rect.x, list->rect.y, list->rect.w,
           list->rect.h, list->buddy->page, ok->rect.x, ok->rect.y, ok->rect.w, ok->rect.h);
    gui_ui_draw(&ui, &b);
    show_damage("first", &ui);
    write_ppm(dir, "widgets", &b);

    /* Nothing changed: nothing drawn. */
    gui_ui_draw(&ui, &b);
    show_damage("again", &ui);

    /* Typing in the field: a click gives it the focus. */
    int x, y;
    centre(name, &x, &y);
    click(&ui, x, y);
    /* "Krthurx", one back, to the start, one deleted, "A": "Arthur". */
    static const uint arthur[] = {KEY_R, KEY_T, KEY_H, KEY_U, KEY_R, KEY_X, KEY_BACKSPACE, KEY_HOME, KEY_DELETE};
    gui_ui_key(&ui, KEY_LEFTSHIFT, 1);
    tap(&ui, KEY_K);
    gui_ui_key(&ui, KEY_LEFTSHIFT, 0);
    for (int i = 0; i < 9; i++) {
        tap(&ui, arthur[i]);
    }
    gui_ui_key(&ui, KEY_LEFTSHIFT, 1);
    tap(&ui, KEY_A);
    gui_ui_key(&ui, KEY_LEFTSHIFT, 0);
    tap(&ui, KEY_ENTER);
    printf("field: \"%s\", cursor %d\n", name->chars, name->cursor);
    gui_ui_draw(&ui, &b);
    show_damage("typed", &ui);

    /* Tab to the list; down twice; the wheel. */
    tap(&ui, KEY_TAB);
    tap(&ui, KEY_DOWN);
    tap(&ui, KEY_DOWN);
    centre(list, &x, &y);
    gui_ui_pointer(&ui, x, y);
    gui_ui_scroll(&ui, 1);
    printf("list: selected %d, top %d, bar %d\n", list->selected, list->top, list->buddy->value);
    /* A click on the third line shown. */
    click(&ui, x, list->rect.y + 3 + 2 * (f.height + 2) + 4);
    printf("list: selected %d\n", list->selected);
    gui_ui_draw(&ui, &b);
    show_damage("list", &ui);

    /* OK pressed, the pointer moved off it, let go: no action; then a
     * click. */
    centre(ok, &x, &y);
    gui_ui_pointer(&ui, x, y);
    gui_ui_button(&ui, BTN_LEFT, 1);
    gui_ui_pointer(&ui, 2, 160);
    gui_ui_button(&ui, BTN_LEFT, 0);
    click(&ui, x, y);
    /* Shift+Tab back from OK to Cancel, and Enter. */
    gui_ui_key(&ui, KEY_LEFTSHIFT, 1);
    tap(&ui, KEY_TAB);
    gui_ui_key(&ui, KEY_LEFTSHIFT, 0);
    tap(&ui, KEY_ENTER);

    /* The File menu opened, the pointer on its second item. */
    centre(file, &x, &y);
    click(&ui, x, y);
    gui_ui_pointer(&ui, x, ui.popup.y + 2 + (f.height + 2) + 4);
    gui_ui_draw(&ui, &b);
    show_damage("menu", &ui);
    write_ppm(dir, "input", &b);
    /* Down, and Enter: the third. */
    tap(&ui, KEY_DOWN);
    tap(&ui, KEY_ENTER);
    gui_ui_draw(&ui, &b);
    show_damage("closed", &ui);
    /* Opened again, and a click outside closes it. */
    click(&ui, x, y);
    click(&ui, 190, 100);
    printf("menu open: %d\n", ui.menu != NULL);

    gui_buffer_free(&b);
    gui_ui_free(&ui);
    check_layout(&f);
    gui_font_free(&f);
}

/* ---- keys ---- */

static void type_keys(struct gui_keyboard *k, const char *what, const uint *codes, int n) {
    char out[64];
    int len = 0;
    for (int i = 0; i < n; i++) {
        /* A code with 0x1000 set is let go of; 0x2000 repeats. */
        uint code = codes[i] & 0xfff;
        int pressed = (codes[i] & 0x1000) ? 0 : (codes[i] & 0x2000) ? 2 : 1;
        uint c = gui_keyboard_key(k, code, pressed);
        if (c == '\n') {
            len += snprintf(out + len, sizeof(out) - len, "\\n");
        } else if (c == '\b') {
            len += snprintf(out + len, sizeof(out) - len, "\\b");
        } else if (c != 0) {
            len += gui_utf8_put(out + len, c);
        }
    }
    out[len] = 0;
    printf("%s: \"%s\" mods %x\n", what, out, k->mods);
}

#define UP 0x1000
#define REPEAT 0x2000

static void scene_keys(void) {
    struct gui_keyboard k;
    gui_keyboard_init(&k, &gui_keymap_us);
    static const uint hello[] = {KEY_LEFTSHIFT, KEY_H, KEY_LEFTSHIFT | UP, KEY_E, KEY_L, KEY_L | REPEAT, KEY_O,
                                 KEY_COMMA, KEY_SPACE, KEY_W, KEY_O, KEY_R, KEY_L, KEY_D, KEY_RIGHTSHIFT,
                                 KEY_1, KEY_RIGHTSHIFT | UP, KEY_ENTER};
    type_keys(&k, "hello", hello, sizeof(hello) / sizeof(hello[0]));
    static const uint caps[] = {KEY_CAPSLOCK, KEY_CAPSLOCK | UP, KEY_A, KEY_1, KEY_LEFTSHIFT, KEY_B, KEY_2,
                                KEY_LEFTSHIFT | UP, KEY_CAPSLOCK | REPEAT, KEY_C};
    type_keys(&k, "caps", caps, sizeof(caps) / sizeof(caps[0]));
    static const uint caps_off[] = {KEY_CAPSLOCK, KEY_D};
    type_keys(&k, "caps off", caps_off, sizeof(caps_off) / sizeof(caps_off[0]));
    static const uint ctrl[] = {KEY_LEFTCTRL, KEY_C, KEY_RIGHTALT, KEY_X, KEY_LEFTCTRL | UP, KEY_Y,
                                KEY_RIGHTALT | UP, KEY_Z};
    type_keys(&k, "ctrl alt", ctrl, sizeof(ctrl) / sizeof(ctrl[0]));
    static const uint both[] = {KEY_LEFTSHIFT, KEY_RIGHTSHIFT, KEY_LEFTSHIFT | UP, KEY_GRAVE,
                                KEY_RIGHTSHIFT | UP, KEY_GRAVE, KEY_BACKSPACE, KEY_F1, KEY_LEFTMETA};
    type_keys(&k, "two shifts", both, sizeof(both) / sizeof(both[0]));
    /* The keyboard leaves with Logo and Alt held (their releases go to
     * another window): both let go of, Caps Lock kept. */
    gui_keyboard_key(&k, KEY_LEFTALT, 1);
    gui_keyboard_key(&k, KEY_CAPSLOCK, 1);
    gui_keyboard_modifiers(&k, 0, k.mods);
    static const uint after_leave[] = {KEY_Q};
    type_keys(&k, "after leaving", after_leave, 1);
    /* The server says Shift is held, pressed before this window had the
     * keyboard, and nothing locked: a left Shift, which its release ends. */
    gui_keyboard_modifiers(&k, GUI_MOD_SHIFT, 0);
    static const uint held[] = {KEY_Q, KEY_LEFTSHIFT | UP, KEY_Q};
    type_keys(&k, "shift held on entering", held, 3);
}

/* ---- utf8 ---- */

static void scene_utf8(void) {
    static const char *const texts[] = {
        "A\xc3\xa9\xe2\x82\xac\xf0\x9f\x98\x80",  /* A, e acute, euro, an emoji */
        "\xc0\xaf\xe0\x80\xaf\xed\xa0\x80",      /* overlong, overlong, a surrogate */
        "\xf4\x90\x80\x80\x80\xc3",              /* past U+10FFFF, a lone continuation, cut short */
    };
    for (int i = 0; i < 3; i++) {
        const char *s = texts[i];
        printf("utf8 %d:", i);
        uint c;
        while ((c = gui_utf8_next(&s)) != 0) {
            printf(" %x", c);
        }
        printf("\n");
    }
    static const uint codes[] = {0x41, 0xe9, 0x20ac, 0x1f600, 0xd800, 0x110000};
    printf("encoded:");
    for (int i = 0; i < 6; i++) {
        char out[4];
        int n = gui_utf8_put(out, codes[i]);
        printf(" ");
        for (int j = 0; j < n; j++) {
            printf("%x", (uchar)out[j]);
        }
    }
    printf("\n");
}

/* ---- wire ---- */

/* Messages as a peer could forge them, written raw down a socket pair (the
 * words little-endian, as the wire is) and read back by gui_wire: each
 * well-formed argument read, or the message marked broken. Opcode 1 is a
 * uint, a string and an array; 2 and 3 a string; 4 an array; 5 a
 * descriptor. */
static const uint forged[] = {
    5, 28 << 16 | 1, 7, 3, 'h' | 'i' << 8, 2, 0xbbaa,  /* good: 7 "hi" [aa bb] */
    5, 16 << 16 | 2, 0xfffffffc, 0,                       /* 2^32 - 4 bytes: pos 4 + that wraps to 0 */
    5, 16 << 16 | 4, 0xfffffffd, 0,                       /* rounded up to 4, the length wraps round */
    5, 20 << 16 | 1, 7, 0xfffffff8, 0,                    /* after a uint: pos 8 + 2^32 - 8 wraps to 0 */
    5, 16 << 16 | 3, 4, 'a' | 'b' << 8 | 'c' << 16 | 'd' << 24, /* no NUL */
    5, 16 << 16 | 3, 4, 'a' | 'b' << 16,                  /* a NUL inside */
    5, 8 << 16 | 5,                                       /* a descriptor that did not come */
    5, 12 << 16 | 1, 7,                                   /* the string missing */
    5, 10 << 16 | 1,                                      /* a size not a multiple of 4 */
};

static void scene_wire(void) {
    int fds[2];
    int r = socketpair(AF_UNIX, SOCK_STREAM, 0, fds);
    if (r < 0) {
        fail("socketpair", r);
        return;
    }
    if (write(fds[0], forged, sizeof(forged)) != (ssize_t)sizeof(forged)) {
        fail("write", -EIO);
    }
    close(fds[0]);
    static struct gui_wire w;
    gui_wire_init(&w, fds[1]);
    int n = gui_wire_read(&w, 0);
    if (n != (int)sizeof(forged)) {
        fail("gui_wire_read", n);
    }
    struct gui_message m;
    while ((r = gui_wire_next(&w, &m)) == 1) {
        printf("wire %u.%u:", m.object, m.opcode);
        if (m.opcode == 1) {
            uint v = gui_msg_uint(&m);
            const char *s = gui_msg_string(&m);
            uint len;
            const uchar *a = gui_msg_array(&m, &len);
            if (!m.bad) {
                printf(" %u \"%s\" [", v, s);
                for (uint i = 0; i < len; i++) {
                    printf(i ? " %x" : "%x", a[i]);
                }
                printf("]");
            }
        } else if (m.opcode == 2 || m.opcode == 3) {
            const char *s = gui_msg_string(&m);
            if (!m.bad) {
                printf(" \"%s\"", s);
            }
        } else if (m.opcode == 4) {
            uint len;
            gui_msg_array(&m, &len);
            if (!m.bad) {
                printf(" [%u bytes]", len);
            }
        } else if (m.opcode == 5) {
            gui_msg_fd(&m);
        }
        printf("%s\n", m.bad ? " broken" : "");
    }
    printf("wire: %s\n", r == -EPROTO ? "a broken size" : r == 0 ? "the end" : strerror(-r));
    gui_wire_release(&w);
    close(fds[1]);
}

/* ---- wayland ---- */

/* A display server of a script's worth, for the client half (#184's is the
 * real one): it listens on /run/wayland-0, starts /bin/guidemo, prints each
 * request it gets, answers as a server does (the globals, the seat's
 * capabilities, a configure of 260x220, a buffer released when the next is
 * committed, frame callbacks done), and writes each frame committed to
 * DIR/waylandN.ppm. After the first frame it clicks in the name field,
 * types "Kay" and Enter, and pings; after the second it turns the wheel a
 * notch over the list and clicks its first line; after the third it closes
 * the window. It ends when guidemo hangs up, and prints how it exited. */

#define SOCKET "/run/wayland-0"
#define MAX_OBJECTS 64

struct object {
    uint id;
    const char *interface;
    int offset; /* a wl_buffer's place in its pool, and its shape */
    int w;
    int h;
    int stride;
};

struct fake {
    struct gui_wire wire;
    struct object objects[MAX_OBJECTS];
    int nobjects;
    uchar *pool; /* the pool's memory */
    size_t pool_size;
    uint wm_base;
    uint surface;
    uint xdg_surface;
    uint toplevel;
    uint pointer;
    uint keyboard;
    uint attached; /* the buffer attached since the last commit */
    uint shown;    /* the buffer last committed */
    uint frame;    /* the frame callback asked for */
    int keymap_fd; /* sent with the next flush, then closed */
    int configured;
    int frames;
    uint serial;
    const char *dir;
};

/* The globals: name i + 1 is globals[i]. */
static const char *const globals[] = {"wl_compositor", "wl_shm", "xdg_wm_base", "wl_seat", "wl_output"};
static const uint global_versions[] = {4, 1, 2, 5, 3};

static struct object *find(struct fake *s, uint id) {
    for (int i = 0; i < s->nobjects; i++) {
        if (s->objects[i].id == id) {
            return &s->objects[i];
        }
    }
    return NULL;
}

static struct object *add_object(struct fake *s, uint id, const char *interface) {
    if (s->nobjects == MAX_OBJECTS) {
        return NULL;
    }
    struct object *o = &s->objects[s->nobjects++];
    memset(o, 0, sizeof(*o));
    o->id = id;
    o->interface = interface;
    return o;
}

/* Forgets an object, and tells the client its id is free. */
static void delete_object(struct fake *s, uint id) {
    for (int i = 0; i < s->nobjects; i++) {
        if (s->objects[i].id == id) {
            s->objects[i] = s->objects[--s->nobjects];
            break;
        }
    }
    gui_wire_begin(&s->wire, WL_DISPLAY_ID, WL_DISPLAY_DELETE_ID);
    gui_wire_uint(&s->wire, id);
    gui_wire_end(&s->wire);
}

static void event0(struct fake *s, uint id, uint op) {
    gui_wire_begin(&s->wire, id, op);
    gui_wire_end(&s->wire);
}

static void event1(struct fake *s, uint id, uint op, uint a) {
    gui_wire_begin(&s->wire, id, op);
    gui_wire_uint(&s->wire, a);
    gui_wire_end(&s->wire);
}

/* A wl_pointer enter or motion at x, y (whole pixels, sent as fixed). */
static void pointer_at(struct fake *s, uint op, int x, int y) {
    gui_wire_begin(&s->wire, s->pointer, op);
    if (op == WL_POINTER_ENTER) {
        gui_wire_uint(&s->wire, ++s->serial);
        gui_wire_uint(&s->wire, s->surface);
    } else {
        gui_wire_uint(&s->wire, 1000); /* the time */
    }
    gui_wire_fixed(&s->wire, x * 256);
    gui_wire_fixed(&s->wire, y * 256);
    gui_wire_end(&s->wire);
}

static void pointer_button(struct fake *s, int pressed) {
    gui_wire_begin(&s->wire, s->pointer, WL_POINTER_BUTTON);
    gui_wire_uint(&s->wire, ++s->serial);
    gui_wire_uint(&s->wire, 1000);
    gui_wire_uint(&s->wire, BTN_LEFT);
    gui_wire_uint(&s->wire, (uint)pressed);
    gui_wire_end(&s->wire);
}

static void key(struct fake *s, uint code, int pressed) {
    gui_wire_begin(&s->wire, s->keyboard, WL_KEYBOARD_KEY);
    gui_wire_uint(&s->wire, ++s->serial);
    gui_wire_uint(&s->wire, 1000);
    gui_wire_uint(&s->wire, code);
    gui_wire_uint(&s->wire, (uint)pressed);
    gui_wire_end(&s->wire);
}

static void key_tap(struct fake *s, uint code) {
    key(s, code, 1);
    key(s, code, 0);
}

/* The keyboard enters the window, no key held, or leaves it. */
static void keyboard_focus(struct fake *s, int enter) {
    gui_wire_begin(&s->wire, s->keyboard, enter ? WL_KEYBOARD_ENTER : WL_KEYBOARD_LEAVE);
    gui_wire_uint(&s->wire, ++s->serial);
    gui_wire_uint(&s->wire, s->surface);
    if (enter) {
        gui_wire_array(&s->wire, NULL, 0);
    }
    gui_wire_end(&s->wire);
}

/* No modifier held or locked, as a server says after an enter. */
static void no_modifiers(struct fake *s) {
    gui_wire_begin(&s->wire, s->keyboard, WL_KEYBOARD_MODIFIERS);
    gui_wire_uint(&s->wire, ++s->serial);
    for (int i = 0; i < 4; i++) {
        gui_wire_uint(&s->wire, 0); /* depressed, latched, locked, group */
    }
    gui_wire_end(&s->wire);
}

/* The input that follows frame n. */
static void after_frame(struct fake *s, int n) {
    if (n == 1) {
        pointer_at(s, WL_POINTER_ENTER, 120, 38);
        pointer_button(s, 1);
        pointer_button(s, 0);
        /* Alt goes down, and the keyboard leaves before it comes up: back
         * in the window, it types again. No modifiers event after the
         * second enter, so it is the leave that lets go of Alt (gui_run's
         * focus handler). */
        keyboard_focus(s, 1);
        no_modifiers(s);
        key(s, KEY_LEFTALT, 1);
        keyboard_focus(s, 0);
        keyboard_focus(s, 1);
        key(s, KEY_LEFTSHIFT, 1);
        key_tap(s, KEY_K);
        key(s, KEY_LEFTSHIFT, 0);
        key_tap(s, KEY_A);
        key_tap(s, KEY_Y);
        key_tap(s, KEY_ENTER);
        event1(s, s->wm_base, XDG_WM_BASE_PING, 77);
    } else if (n == 2) {
        pointer_at(s, WL_POINTER_MOTION, 60, 90);
        gui_wire_begin(&s->wire, s->pointer, WL_POINTER_AXIS);
        gui_wire_uint(&s->wire, 1000);
        gui_wire_uint(&s->wire, WL_POINTER_AXIS_VERTICAL_SCROLL);
        gui_wire_fixed(&s->wire, 10 * 256);
        gui_wire_end(&s->wire);
        pointer_at(s, WL_POINTER_MOTION, 60, 66);
        pointer_button(s, 1);
        pointer_button(s, 0);
    } else if (n == 3) {
        event0(s, s->toplevel, XDG_TOPLEVEL_CLOSE);
    }
}

/* The buffer committed, written out as frame s->frames. */
static void write_frame(struct fake *s, const struct object *b) {
    if (s->pool == NULL || b->offset < 0 || b->w <= 0 || b->h <= 0 || b->stride < b->w * 4 ||
        (size_t)b->offset + (size_t)b->stride * (size_t)b->h > s->pool_size) {
        printf("frame %d: a buffer outside its pool\n", s->frames);
        return;
    }
    struct gui_buffer view;
    gui_buffer_init(&view, (uint *)(s->pool + b->offset), b->w, b->h, b->stride / 4);
    char name[16];
    snprintf(name, sizeof(name), "wayland%d", s->frames);
    write_ppm(s->dir, name, &view);
}

static void commit(struct fake *s) {
    if (!s->configured) {
        /* The first commit: the window's first configure. */
        gui_wire_begin(&s->wire, s->toplevel, XDG_TOPLEVEL_CONFIGURE);
        gui_wire_int(&s->wire, 260);
        gui_wire_int(&s->wire, 220);
        gui_wire_array(&s->wire, NULL, 0);
        gui_wire_end(&s->wire);
        event1(s, s->xdg_surface, XDG_SURFACE_CONFIGURE, 100);
        s->configured = 1;
        return;
    }
    if (s->attached != 0) {
        struct object *b = find(s, s->attached);
        s->frames++;
        if (b != NULL) {
            write_frame(s, b);
        }
        if (s->shown != 0 && s->shown != s->attached) {
            event0(s, s->shown, WL_BUFFER_RELEASE);
        }
        s->shown = s->attached;
        s->attached = 0;
        after_frame(s, s->frames);
    }
    if (s->frame != 0) {
        event1(s, s->frame, WL_CALLBACK_DONE, 0);
        delete_object(s, s->frame);
        s->frame = 0;
    }
}

/* Prints a request and answers it: 0, or -1 for one that can't be. */
static int request(struct fake *s, struct gui_message *m) {
    struct object *o = m->object == WL_DISPLAY_ID ? NULL : find(s, m->object);
    const char *in = m->object == WL_DISPLAY_ID ? "wl_display" : o != NULL ? o->interface : NULL;
    if (in == NULL) {
        printf("> unknown object %u\n", m->object);
        return -1;
    }
    uint op = m->opcode;
    printf("> %s@%u.", in, m->object);
    if (strcmp(in, "wl_display") == 0 && op == WL_DISPLAY_SYNC) {
        uint id = gui_msg_uint(m);
        printf("sync %u\n", id);
        event1(s, id, WL_CALLBACK_DONE, ++s->serial);
        delete_object(s, id);
    } else if (strcmp(in, "wl_display") == 0 && op == WL_DISPLAY_GET_REGISTRY) {
        uint id = gui_msg_uint(m);
        printf("get_registry %u\n", id);
        add_object(s, id, "wl_registry");
        for (int i = 0; i < 5; i++) {
            gui_wire_begin(&s->wire, id, WL_REGISTRY_GLOBAL);
            gui_wire_uint(&s->wire, (uint)i + 1);
            gui_wire_string(&s->wire, globals[i]);
            gui_wire_uint(&s->wire, global_versions[i]);
            gui_wire_end(&s->wire);
        }
    } else if (strcmp(in, "wl_registry") == 0 && op == WL_REGISTRY_BIND) {
        uint name = gui_msg_uint(m);
        const char *interface = gui_msg_string(m);
        uint version = gui_msg_uint(m);
        uint id = gui_msg_uint(m);
        if (m->bad || name < 1 || name > 5 || interface == NULL || strcmp(interface, globals[name - 1]) != 0) {
            printf("bind: bad\n");
            return -1;
        }
        printf("bind %u %s v%u -> %u\n", name, interface, version, id);
        add_object(s, id, globals[name - 1]);
        if (name == 2) {
            event1(s, id, WL_SHM_FORMAT, WL_SHM_FORMAT_ARGB8888);
            event1(s, id, WL_SHM_FORMAT, WL_SHM_FORMAT_XRGB8888);
        } else if (name == 3) {
            s->wm_base = id;
        } else if (name == 4) {
            event1(s, id, WL_SEAT_CAPABILITIES, WL_SEAT_CAPABILITY_POINTER | WL_SEAT_CAPABILITY_KEYBOARD);
        }
    } else if (strcmp(in, "wl_compositor") == 0 && op == WL_COMPOSITOR_CREATE_SURFACE) {
        uint id = gui_msg_uint(m);
        printf("create_surface %u\n", id);
        add_object(s, id, "wl_surface");
        s->surface = id;
    } else if (strcmp(in, "wl_shm") == 0 && op == WL_SHM_CREATE_POOL) {
        uint id = gui_msg_uint(m);
        int fd = gui_msg_fd(m);
        int size = gui_msg_int(m);
        printf("create_pool %u, %d bytes\n", id, size);
        if (fd < 0 || size <= 0) {
            return -1;
        }
        if (s->pool != NULL) {
            munmap(s->pool, s->pool_size);
        }
        void *mem = mmap(NULL, (size_t)size, PROT_READ, MAP_SHARED, fd, 0);
        close(fd);
        if ((long)mem < 0) {
            printf("mmap: %s\n", strerror(-(int)(long)mem));
            return -1;
        }
        s->pool = mem;
        s->pool_size = (size_t)size;
        add_object(s, id, "wl_shm_pool");
    } else if (strcmp(in, "wl_shm_pool") == 0 && op == WL_SHM_POOL_CREATE_BUFFER) {
        uint id = gui_msg_uint(m);
        int offset = gui_msg_int(m);
        int w = gui_msg_int(m);
        int h = gui_msg_int(m);
        int stride = gui_msg_int(m);
        uint format = gui_msg_uint(m);
        printf("create_buffer %u, at %d, %dx%d, stride %d, format %u\n", id, offset, w, h, stride, format);
        struct object *b = add_object(s, id, "wl_buffer");
        if (b != NULL) {
            b->offset = offset;
            b->w = w;
            b->h = h;
            b->stride = stride;
        }
    } else if ((strcmp(in, "wl_shm_pool") == 0 && op == WL_SHM_POOL_DESTROY) ||
               (strcmp(in, "wl_buffer") == 0 && op == WL_BUFFER_DESTROY) ||
               (strcmp(in, "xdg_surface") == 0 && op == XDG_SURFACE_DESTROY) ||
               (strcmp(in, "xdg_toplevel") == 0 && op == XDG_TOPLEVEL_DESTROY) ||
               (strcmp(in, "wl_surface") == 0 && op == WL_SURFACE_DESTROY)) {
        printf("destroy\n");
        delete_object(s, m->object);
    } else if (strcmp(in, "xdg_wm_base") == 0 && op == XDG_WM_BASE_GET_XDG_SURFACE) {
        uint id = gui_msg_uint(m);
        uint surface = gui_msg_uint(m);
        printf("get_xdg_surface %u, surface %u\n", id, surface);
        add_object(s, id, "xdg_surface");
        s->xdg_surface = id;
    } else if (strcmp(in, "xdg_wm_base") == 0 && op == XDG_WM_BASE_PONG) {
        printf("pong %u\n", gui_msg_uint(m));
    } else if (strcmp(in, "xdg_surface") == 0 && op == XDG_SURFACE_GET_TOPLEVEL) {
        uint id = gui_msg_uint(m);
        printf("get_toplevel %u\n", id);
        add_object(s, id, "xdg_toplevel");
        s->toplevel = id;
    } else if (strcmp(in, "xdg_surface") == 0 && op == XDG_SURFACE_ACK_CONFIGURE) {
        printf("ack_configure %u\n", gui_msg_uint(m));
    } else if (strcmp(in, "xdg_toplevel") == 0 && op == XDG_TOPLEVEL_SET_TITLE) {
        const char *title = gui_msg_string(m);
        printf("set_title \"%s\"\n", title != NULL ? title : "(null)");
    } else if (strcmp(in, "wl_surface") == 0 && op == WL_SURFACE_ATTACH) {
        uint buffer = gui_msg_uint(m);
        int x = gui_msg_int(m);
        int y = gui_msg_int(m);
        printf("attach %u, %d, %d\n", buffer, x, y);
        s->attached = buffer;
    } else if (strcmp(in, "wl_surface") == 0 && (op == WL_SURFACE_DAMAGE_BUFFER || op == WL_SURFACE_DAMAGE)) {
        int x = gui_msg_int(m);
        int y = gui_msg_int(m);
        int w = gui_msg_int(m);
        int h = gui_msg_int(m);
        printf("%s %d,%d %dx%d\n", op == WL_SURFACE_DAMAGE ? "damage" : "damage_buffer", x, y, w, h);
    } else if (strcmp(in, "wl_surface") == 0 && op == WL_SURFACE_FRAME) {
        s->frame = gui_msg_uint(m);
        printf("frame %u\n", s->frame);
    } else if (strcmp(in, "wl_surface") == 0 && op == WL_SURFACE_COMMIT) {
        printf("commit\n");
        commit(s);
    } else if (strcmp(in, "wl_seat") == 0 && op == WL_SEAT_GET_POINTER) {
        s->pointer = gui_msg_uint(m);
        printf("get_pointer %u\n", s->pointer);
        add_object(s, s->pointer, "wl_pointer");
    } else if (strcmp(in, "wl_seat") == 0 && op == WL_SEAT_GET_KEYBOARD) {
        s->keyboard = gui_msg_uint(m);
        printf("get_keyboard %u\n", s->keyboard);
        add_object(s, s->keyboard, "wl_keyboard");
        /* No keymap: a descriptor all the same, as the event has one. */
        s->keymap_fd = memfd_create("keymap", MFD_CLOEXEC);
        gui_wire_begin(&s->wire, s->keyboard, WL_KEYBOARD_KEYMAP);
        gui_wire_uint(&s->wire, WL_KEYBOARD_KEYMAP_FORMAT_NO_KEYMAP);
        gui_wire_fd(&s->wire, s->keymap_fd);
        gui_wire_uint(&s->wire, 0);
        gui_wire_end(&s->wire);
    } else {
        printf("request %u\n", op);
    }
    if (m->bad) {
        printf("> a broken request\n");
        return -1;
    }
    return 0;
}

static void scene_wayland(const char *dir) {
    struct fake *s = calloc(1, sizeof(struct fake));
    if (s == NULL) {
        fail("calloc", -ENOMEM);
        return;
    }
    s->dir = dir;
    s->keymap_fd = -1;
    struct sockaddr_un a;
    memset(&a, 0, sizeof(a));
    a.sun_family = AF_UNIX;
    strcpy(a.sun_path, SOCKET);
    int l = socket(AF_UNIX, SOCK_STREAM, 0);
    int r = bind(l, &a);
    if (r == 0) {
        r = listen(l, 1);
    }
    if (r < 0) {
        fail(SOCKET, r);
        free(s);
        return;
    }
    char *argv[] = {"guidemo", NULL};
    int pid = spawn("/bin/guidemo", argv, environ);
    if (pid < 0) {
        fail("spawn /bin/guidemo", pid);
        free(s);
        return;
    }
    struct sockaddr_un peer;
    int c = accept(l, &peer);
    if (c < 0) {
        fail("accept", c);
    } else {
        gui_wire_init(&s->wire, c);
        int stop = 0;
        while (!stop) {
            int n = gui_wire_read(&s->wire, 0);
            if (n <= 0) {
                if (n < 0) {
                    fail("recvmsg", n);
                }
                break;
            }
            struct gui_message m;
            while (!stop && (r = gui_wire_next(&s->wire, &m)) == 1) {
                stop = request(s, &m) < 0;
            }
            if (r == -EPROTO) {
                printf("> a broken message\n");
                stop = 1;
            } else if (r < 0) {
                printf("> the connection: %s\n", strerror(-r)); /* a send's error, sticky */
                stop = 1;
            }
            if (gui_wire_flush(&s->wire) < 0) {
                stop = 1;
            }
            if (s->keymap_fd >= 0) {
                close(s->keymap_fd);
                s->keymap_fd = -1;
            }
        }
        printf("frames: %d\n", s->frames);
        gui_wire_release(&s->wire);
        close(c);
    }
    int status = -1;
    wait(pid, &status);
    printf("guidemo exited %d\n", status);
    close(l);
    unlink(SOCKET);
    if (s->pool != NULL) {
        munmap(s->pool, s->pool_size);
    }
    free(s);
}

int main(int argc, char **argv) {
    if (argc < 2) {
        printf("usage: guitest draw|text|image|widgets|wayland DIR, or damage|keys|utf8|wire\n");
        return 2;
    }
    const char *what = argv[1];
    const char *dir = argc > 2 ? argv[2] : ".";
    if (strcmp(what, "draw") == 0) {
        scene_draw(dir);
    } else if (strcmp(what, "text") == 0) {
        scene_text(dir);
    } else if (strcmp(what, "image") == 0) {
        scene_image(dir);
    } else if (strcmp(what, "widgets") == 0) {
        scene_widgets(dir);
    } else if (strcmp(what, "wayland") == 0) {
        scene_wayland(dir);
    } else if (strcmp(what, "damage") == 0) {
        scene_damage();
    } else if (strcmp(what, "keys") == 0) {
        scene_keys();
    } else if (strcmp(what, "utf8") == 0) {
        scene_utf8();
    } else if (strcmp(what, "wire") == 0) {
        scene_wire();
    } else {
        printf("guitest: no scene %s\n", what);
        return 2;
    }
    return failed;
}
