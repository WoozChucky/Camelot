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
 *   guitest damage       the damage lists of a sequence of rectangles
 *   guitest keys         the characters of key sequences
 *   guitest utf8         decoding and encoding
 *
 * Exits 0, or 1 with what went wrong. */
#include "gui.h"

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
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC);
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
    /* A copy of part of the buffer itself, overlapping. */
    gui_blit(&b, 6, 44, &b, gui_rect(4, 4, 30, 20));
    write_ppm(dir, "draw", &b);
    gui_buffer_free(&b);
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

int main(int argc, char **argv) {
    if (argc < 2) {
        printf("usage: guitest draw|text|image|widgets DIR, or damage|keys|utf8\n");
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
    } else if (strcmp(what, "damage") == 0) {
        scene_damage();
    } else if (strcmp(what, "keys") == 0) {
        scene_keys();
    } else if (strcmp(what, "utf8") == 0) {
        scene_utf8();
    } else {
        printf("guitest: no scene %s\n", what);
        return 2;
    }
    return failed;
}
