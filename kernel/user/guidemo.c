/* guidemo: the GUI library's demo (gui.h): a window of widgets on the
 * display server (#184), at /run/wayland-0 or $WAYLAND_DISPLAY. Type a
 * name, or pick a knight from the list, and Greet (or Enter); File > Quit
 * or the window's close button ends it. On the desktop image, which has
 * the font. Exits 0, or 1 when it can't connect or draw. */
#include "gui.h"

#define FONT "/usr/share/fonts/spleen/8x16.psfu"

static const char *const knights[] = {"Arthur",   "Lancelot", "Galahad", "Percival", "Gawain",
                                      "Bedivere", "Tristan",  "Kay",     "Gareth",   "Bors"};
static const char *const file_items[] = {"Clear", "Quit"};
static const char *const help_items[] = {"About"};

static struct gui_widget *name;
static struct gui_widget *status;
static char greeting[64];
static int quit;

static void greet(struct gui_widget *w, int value) {
    (void)w;
    (void)value;
    snprintf(greeting, sizeof(greeting), "Hello, %s!", name->chars[0] != 0 ? name->chars : "stranger");
    gui_set_text(status, greeting);
}

static void pick(struct gui_widget *w, int item) {
    (void)w;
    gui_field_set(name, knights[item]);
}

static void clear(struct gui_widget *w, int value) {
    (void)w;
    (void)value;
    gui_field_set(name, "");
    gui_set_text(status, "Who goes there?");
}

static void file_menu(struct gui_widget *w, int item) {
    if (item == 0) {
        clear(w, 0);
    } else {
        quit = 1;
    }
}

static void help_menu(struct gui_widget *w, int item) {
    (void)w;
    (void)item;
    gui_set_text(status, "Camelot's GUI library");
}

int main(void) {
    struct gui_font font;
    int r = gui_font_load(&font, FONT);
    if (r < 0) {
        printf("guidemo: %s: %s\n", FONT, strerror(-r));
        return 1;
    }
    struct gui_ui ui;
    gui_ui_init(&ui, &font);
    struct gui_widget *root = gui_column(&ui, NULL, 4, 4);
    struct gui_widget *bar = gui_row(&ui, root, 0, 0);
    gui_menu(&ui, bar, "File", file_items, 2, file_menu);
    gui_menu(&ui, bar, "Help", help_items, 1, help_menu);
    struct gui_widget *row = gui_row(&ui, root, 4, 0);
    gui_label(&ui, row, "Name:");
    name = gui_field(&ui, row, 12, 32, greet);
    gui_grow(name);
    struct gui_widget *list = gui_list(&ui, root, knights, 10, 4, pick);
    gui_grow(list->parent);
    status = gui_label(&ui, root, "Who goes there?");
    struct gui_widget *buttons = gui_row(&ui, root, 4, 0);
    gui_grow(gui_label(&ui, buttons, ""));
    gui_button(&ui, buttons, "Clear", clear);
    gui_button(&ui, buttons, "Greet", greet);
    gui_ui_focus(&ui, name);

    struct gui_display *d = gui_connect(NULL, &r);
    if (d == NULL) {
        printf("guidemo: no display server: %s\n", strerror(-r));
        return 1;
    }
    r = gui_run(d, &ui, "Camelot", 220, 200, &quit);
    if (r < 0) {
        printf("guidemo: %s\n", r == -EPROTO ? "protocol error" : strerror(-r));
    }
    gui_disconnect(d);
    gui_ui_free(&ui);
    gui_font_free(&font);
    return r < 0;
}
