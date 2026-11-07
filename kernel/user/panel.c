/* panel: the desktop's panel (#185, docs/excalibur-spec.md section 10,
 * "The desktop shell"), a bar along the top of the screen:
 *
 *   panel [-v] [PROGRAM ARGS...]
 *
 * - The launcher, at the left: a click opens its menu of the applications
 *   /usr/share/applications describes (a freedesktop.org .desktop file
 *   each: Name, Exec, Icon, the icon /usr/share/images/ICON.qoi), sorted
 *   by name. Clicking one, or choosing it with the arrows and Enter, starts
 *   it; Escape, a click on the launcher again or the menu losing the
 *   keyboard (a click on a window) closes the menu.
 * - The windows (wlroots' foreign toplevel management), a button each in
 *   the order they came, the one with the keyboard lit: a click raises a
 *   window and gives it the keyboard, the middle button asks it to close.
 * - At the right, how busy each hart was over the last second (the harts
 *   system call's ticks that were not idle), a bar each, and the date and
 *   time from the real-time clock (CLOCK_REALTIME: the clock read at boot
 *   plus the time since), in UTC.
 *
 * The bar is a layer surface (wlroots' layer shell) anchored to the top
 * edge, with an exclusive zone of its height, so windows are placed below
 * it. It waits for the display server to come up (10 seconds at most),
 * then starts PROGRAM if no window is shown yet: the desktop's /etc/rc
 * gives it the terminal, so a panel started again finds the windows it
 * left. -v says what it does on standard output ("panel: ready",
 * "panel: started NAME", "panel: menu shown" and "closed"). It exits 1 if
 * it can't start, 0 when the server goes. */
#include "gui.h"

#define FONT "/usr/share/fonts/spleen/8x16.psfu"
#define APPS "/usr/share/applications"
#define IMAGES "/usr/share/images"
#define LOGO IMAGES "/castle.qoi"

/* The bar's height; the launcher's width; the widest a window's button. */
#define BAR_H 24
#define LAUNCHER_W 84
#define TASK_W 150
#define GAP 4
/* A hart's meter: a bar this wide, every hart's this far apart. */
#define METER_W 5
#define METER_GAP 2
#define MAX_HARTS 8
/* The menu: its width, an item's height, the space round the items. */
#define MENU_W 180
#define ITEM_H 24
#define MENU_PAD 2
#define MAX_APPS 16
#define NAME_MAX 32
#define EXEC_MAX 96
#define MAX_ARGS 8
/* The display server is tried every CONNECT_WAIT ticks, CONNECT_TRIES times. */
#define CONNECT_WAIT 1
#define CONNECT_TRIES 100
/* How often the meters and the clock are read, ms. */
#define REFRESH_MS 1000

#define BAR_BG GUI_RGB(0x1c, 0x24, 0x30)
#define BUTTON_BG GUI_RGB(0x2c, 0x38, 0x48)
#define BUTTON_HOVER GUI_RGB(0x3c, 0x4c, 0x60)
#define ACTIVE_BG GUI_RGB(0x30, 0x70, 0xd0)
#define TEXT GUI_RGB(0xf0, 0xf0, 0xf0)
#define DIM_TEXT GUI_RGB(0xa0, 0xa8, 0xb0)
#define METER_BG GUI_RGB(0x30, 0x3a, 0x46)
#define METER_FG GUI_RGB(0x40, 0xc0, 0x60)
#define MENU_BG GUI_RGB(0x24, 0x2e, 0x3c)
#define MENU_EDGE GUI_RGB(0x50, 0x60, 0x74)

struct app {
    char name[NAME_MAX];
    char exec[EXEC_MAX];
    struct gui_buffer icon;
    int has_icon;
};

static struct gui_display *display;
static struct gui_font font;
static struct gui_buffer logo;
static int has_logo;
static int verbose;

static struct app apps[MAX_APPS];
static int napps;

/* The bar. */
static struct gui_window *bar;
static int bar_frame_ready = 1;
static int bar_dirty = 1;
/* What the pointer is over: -1 nothing, -2 the launcher, else a window's
 * button. */
static int hover = -1;
static int pointer_x;
static int quit;

/* The menu, while it is open; the item under the pointer or chosen with
 * the arrows, -1 none. */
static struct gui_window *menu;
static int menu_frame_ready;
static int menu_dirty;
static int menu_item = -1;
static int menu_closing;

/* The harts' ticks and idle ticks when last read, and how busy each was
 * since, in percent. */
static struct hartinfo last[MAX_HARTS];
static int nharts;
static int busy[MAX_HARTS];
static char clock_text[32];

static long now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

/* ---- Starting programs ---- */

/* Children are collected by a thread of their own, as they end (a parent
 * that never waits keeps its ended children's slots): it waits for any
 * child, and with none, sleeps on `children` until the next one starts. */
static int children;
static uchar reaper_stack[4096];

static int reaper(void *arg) {
    (void)arg;
    for (;;) {
        int seen = atomic_load(&children);
        if (wait(-1, NULL) == -ECHILD) {
            futex_wait(&children, seen);
        }
    }
    return 0;
}

/* Starts a command line, its words split at blanks (a .desktop file's
 * field codes, %f and the like, left out), the program in /bin unless the
 * first word is a path. */
static void launch(const char *command) {
    char line[EXEC_MAX];
    snprintf(line, sizeof(line), "%s", command);
    char *argv[MAX_ARGS + 1];
    int argc = 0;
    char *s = line;
    while (*s != 0 && argc < MAX_ARGS) {
        while (*s == ' ' || *s == '\t') {
            *s++ = 0;
        }
        if (*s == 0) {
            break;
        }
        char *word = s;
        while (*s != 0 && *s != ' ' && *s != '\t') {
            s++;
        }
        if (!(word[0] == '%' && word[1] != 0 && (word[2] == 0 || word[2] == ' ' || word[2] == '\t'))) {
            argv[argc++] = word;
        }
    }
    argv[argc] = NULL;
    if (argc == 0) {
        return;
    }
    char path[MAXPATH];
    if (strchr(argv[0], '/') == NULL) {
        snprintf(path, sizeof(path), "/bin/%s", argv[0]);
    } else {
        snprintf(path, sizeof(path), "%s", argv[0]);
    }
    int pid = spawn(path, argv, environ);
    if (pid < 0) {
        fprintf(2, "panel: can't start %s: %s\n", path, strerror(-pid));
        return;
    }
    atomic_add(&children, 1);
    futex_wake(&children, 1);
    if (verbose) {
        printf("panel: started %s\n", argv[0]);
    }
}

/* ---- The applications ---- */

/* `line` without the blanks round it. */
static char *trim(char *line) {
    while (*line == ' ' || *line == '\t') {
        line++;
    }
    size_t n = strlen(line);
    while (n > 0 && (line[n - 1] == ' ' || line[n - 1] == '\t' || line[n - 1] == '\r')) {
        line[--n] = 0;
    }
    return line;
}

/* Reads one .desktop file into apps[napps]: an application with a name
 * and a command, shown (not NoDisplay=true), or nothing. */
static void read_entry(const char *path) {
    uchar *data;
    size_t len;
    if (gui_read_file(path, &data, &len) < 0) {
        return;
    }
    struct app *a = &apps[napps];
    memset(a, 0, sizeof(*a));
    char icon[NAME_MAX] = "";
    int in_entry = 0;
    int application = 0;
    int hidden = 0;
    char *text = malloc(len + 1);
    if (text == NULL) {
        free(data);
        return;
    }
    memcpy(text, data, len);
    text[len] = 0;
    free(data);
    char *next = text;
    while (next != NULL && *next != 0) {
        char *line = next;
        next = strchr(line, '\n');
        if (next != NULL) {
            *next++ = 0;
        }
        line = trim(line);
        if (line[0] == '#' || line[0] == 0) {
            continue;
        }
        if (line[0] == '[') {
            in_entry = strcmp(line, "[Desktop Entry]") == 0;
            continue;
        }
        char *eq = strchr(line, '=');
        if (!in_entry || eq == NULL) {
            continue;
        }
        *eq = 0;
        char *key = trim(line);
        char *value = trim(eq + 1);
        if (strcmp(key, "Type") == 0) {
            application = strcmp(value, "Application") == 0;
        } else if (strcmp(key, "Name") == 0) {
            snprintf(a->name, sizeof(a->name), "%s", value);
        } else if (strcmp(key, "Exec") == 0) {
            snprintf(a->exec, sizeof(a->exec), "%s", value);
        } else if (strcmp(key, "Icon") == 0) {
            snprintf(icon, sizeof(icon), "%s", value);
        } else if (strcmp(key, "NoDisplay") == 0 || strcmp(key, "Hidden") == 0) {
            hidden = strcmp(value, "true") == 0;
        }
    }
    free(text);
    if (!application || hidden || a->name[0] == 0 || a->exec[0] == 0) {
        return;
    }
    if (icon[0] != 0) {
        char icon_path[MAXPATH];
        snprintf(icon_path, sizeof(icon_path), "%s/%s.qoi", IMAGES, icon);
        a->has_icon = gui_image_load(&a->icon, icon_path) == 0;
    }
    napps++;
}

/* Every application of /usr/share/applications, sorted by name. */
static void read_apps(void) {
    int fd = open(APPS, O_RDONLY);
    if (fd < 0) {
        return;
    }
    struct dirent de;
    while (napps < MAX_APPS && read(fd, &de, sizeof de) == sizeof de) {
        char name[DIRSIZ + 1];
        memcpy(name, de.name, DIRSIZ);
        name[DIRSIZ] = 0;
        size_t n = strlen(name);
        if (de.inum == 0 || n < 8 || strcmp(name + n - 8, ".desktop") != 0) {
            continue;
        }
        char path[MAXPATH];
        snprintf(path, sizeof(path), "%s/%s", APPS, name);
        read_entry(path);
    }
    close(fd);
    for (int i = 1; i < napps; i++) {
        struct app a = apps[i];
        int j = i;
        while (j > 0 && strcmp(apps[j - 1].name, a.name) > 0) {
            apps[j] = apps[j - 1];
            j--;
        }
        apps[j] = a;
    }
}

/* ---- The bar ---- */

/* The windows' buttons: how many there are, and the n-th's rectangle. */
static int task_count(void) {
    const struct gui_toplevel *list;
    return gui_toplevels(display, &list);
}

/* Where the meters and the clock start: the windows' buttons end there. */
static int status_x(void) {
    int width = gui_window_width(bar);
    int clock_w = gui_text_width(&font, clock_text, 0);
    int meters_w = nharts * (METER_W + METER_GAP);
    return width - GAP - clock_w - 2 * GAP - meters_w;
}

static struct gui_rect task_rect(int i, int n) {
    int x0 = LAUNCHER_W + GAP;
    int room = status_x() - GAP - x0;
    int w = n > 0 ? room / n : 0;
    if (w > TASK_W) {
        w = TASK_W;
    }
    return gui_rect(x0 + i * w, 2, w - 2, BAR_H - 4);
}

static struct gui_rect launcher_rect(void) {
    return gui_rect(2, 2, LAUNCHER_W - 2, BAR_H - 4);
}

/* What is at x along the bar: -2 the launcher, a window's button, or -1. */
static int bar_hit(int x) {
    if (gui_rect_contains(launcher_rect(), x, BAR_H / 2)) {
        return -2;
    }
    int n = task_count();
    for (int i = 0; i < n; i++) {
        if (gui_rect_contains(task_rect(i, n), x, BAR_H / 2)) {
            return i;
        }
    }
    return -1;
}

/* Text cut to fit `width` pixels, ".." at its end when it was cut. */
static void fit_text(char *out, size_t size, const char *s, int width) {
    int cells = width / font.width;
    int n = 0;
    const char *p = s;
    while (*p != 0 && n < cells) {
        gui_utf8_next(&p);
        n++;
    }
    size_t bytes = (size_t)(p - s);
    if (*p != 0 && cells >= 2) {
        /* Two cells fewer, then "..". */
        const char *q = s;
        for (int i = 0; i < cells - 2; i++) {
            gui_utf8_next(&q);
        }
        bytes = (size_t)(q - s);
        if (bytes + 3 > size) {
            bytes = size - 3;
        }
        memcpy(out, s, bytes);
        memcpy(out + bytes, "..", 3);
        return;
    }
    if (bytes + 1 > size) {
        bytes = size - 1;
    }
    memcpy(out, s, bytes);
    out[bytes] = 0;
}

static void draw_bar(void) {
    struct gui_buffer *b = gui_window_buffer(bar);
    if (b == NULL) {
        quit = 1;
        return;
    }
    int width = b->width;
    gui_fill(b, gui_rect(0, 0, width, BAR_H), BAR_BG);
    int ty = (BAR_H - font.height) / 2;

    struct gui_rect l = launcher_rect();
    gui_fill_round(b, l, 3, menu != NULL ? ACTIVE_BG : hover == -2 ? BUTTON_HOVER : BUTTON_BG);
    int x = l.x + 4;
    if (has_logo) {
        gui_blend(b, x, (BAR_H - logo.height) / 2, &logo, gui_rect(0, 0, logo.width, logo.height));
        x += logo.width + 4;
    }
    gui_text(b, &font, x, ty, "Camelot", TEXT, 0);

    const struct gui_toplevel *list;
    int n = gui_toplevels(display, &list);
    for (int i = 0; i < n; i++) {
        struct gui_rect r = task_rect(i, n);
        uint bg = list[i].activated ? ACTIVE_BG : hover == i ? BUTTON_HOVER : BUTTON_BG;
        gui_fill_round(b, r, 3, bg);
        char title[GUI_TOPLEVEL_TITLE];
        fit_text(title, sizeof(title), list[i].title[0] != 0 ? list[i].title : "(untitled)", r.w - 12);
        gui_clip(b, r);
        gui_text(b, &font, r.x + 6, ty, title, list[i].activated ? TEXT : DIM_TEXT, 0);
        gui_unclip(b);
    }

    x = status_x();
    for (int i = 0; i < nharts; i++) {
        int h = BAR_H - 8;
        int fill = (busy[i] * h + 50) / 100;
        gui_fill(b, gui_rect(x, 4, METER_W, h), METER_BG);
        gui_fill(b, gui_rect(x, 4 + h - fill, METER_W, fill), METER_FG);
        x += METER_W + METER_GAP;
    }
    x += 2 * GAP - METER_GAP;
    gui_text(b, &font, x, ty, clock_text, TEXT, 0);

    gui_window_damage(bar, gui_rect(0, 0, width, BAR_H));
    if (gui_window_commit(bar) < 0) {
        quit = 1;
    }
    bar_frame_ready = 0;
    bar_dirty = 0;
}

/* The harts' load since the last call, and the clock: whether either
 * changed what the bar shows. */
static int refresh(void) {
    int changed = 0;
    struct hartinfo now[MAX_HARTS];
    int n = harts(now, MAX_HARTS);
    if (n > MAX_HARTS) {
        n = MAX_HARTS;
    }
    if (n > 0) {
        for (int i = 0; i < n; i++) {
            int b = 0;
            if (i < nharts) {
                uint ticks = now[i].ticks - last[i].ticks;
                uint idle = now[i].idle - last[i].idle;
                if (ticks > 0 && idle <= ticks) {
                    b = (int)((ticks - idle) * 100 / ticks);
                }
            }
            changed |= b != busy[i];
            busy[i] = b;
            last[i] = now[i];
        }
        changed |= n != nharts;
        nharts = n;
    }
    static const char *const days[] = {"Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"};
    static const char *const months[] = {"Jan", "Feb", "Mar", "Apr", "May", "Jun",
                                         "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"};
    char text[32];
    time_t t = time(NULL);
    if (t < 0) {
        snprintf(text, sizeof(text), "--:--");
    } else {
        struct tm tm;
        gmtime_r(&t, &tm);
        snprintf(text, sizeof(text), "%s %d %s %02d:%02d", days[tm.tm_wday], tm.tm_mday, months[tm.tm_mon],
                 tm.tm_hour, tm.tm_min);
    }
    if (strcmp(text, clock_text) != 0) {
        memcpy(clock_text, text, sizeof(text));
        changed = 1;
    }
    return changed;
}

/* ---- The menu ---- */

/* Closed from the main loop, not from one of its own handlers. */
static void close_menu(void) {
    if (menu != NULL) {
        menu_closing = 1;
    }
}

static void menu_gone(void) {
    gui_window_destroy(menu);
    menu = NULL;
    menu_closing = 0;
    bar_dirty = 1;
    if (verbose) {
        printf("panel: menu closed\n");
    }
}

static void draw_menu(void) {
    struct gui_buffer *b = gui_window_buffer(menu);
    if (b == NULL) {
        close_menu();
        return;
    }
    gui_fill(b, gui_rect(0, 0, b->width, b->height), MENU_BG);
    gui_outline(b, gui_rect(0, 0, b->width, b->height), MENU_EDGE);
    for (int i = 0; i < napps; i++) {
        struct gui_rect r = gui_rect(MENU_PAD, MENU_PAD + i * ITEM_H, b->width - 2 * MENU_PAD, ITEM_H);
        if (i == menu_item) {
            gui_fill(b, r, ACTIVE_BG);
        }
        int x = r.x + 6;
        if (apps[i].has_icon) {
            struct gui_buffer *icon = &apps[i].icon;
            gui_blend(b, x, r.y + (ITEM_H - icon->height) / 2, icon, gui_rect(0, 0, icon->width, icon->height));
        }
        x += 16 + 8;
        gui_text(b, &font, x, r.y + (ITEM_H - font.height) / 2, apps[i].name, TEXT, 0);
    }
    gui_window_damage(menu, gui_rect(0, 0, b->width, b->height));
    if (gui_window_commit(menu) < 0) {
        close_menu();
    }
    menu_frame_ready = 0;
    menu_dirty = 0;
}

static void choose(int item) {
    if (item >= 0 && item < napps) {
        launch(apps[item].exec);
    }
    close_menu();
}

static int menu_hit(int y) {
    int i = (y - MENU_PAD) / ITEM_H;
    return y >= MENU_PAD && i < napps ? i : -1;
}

static void menu_pointer(struct gui_window *w, int x, int y) {
    (void)w;
    (void)x;
    int i = menu_hit(y);
    if (i != menu_item) {
        menu_item = i;
        menu_dirty = 1;
    }
}

static void menu_leave(struct gui_window *w) {
    (void)w;
    if (menu_item != -1) {
        menu_item = -1;
        menu_dirty = 1;
    }
}

static void menu_button(struct gui_window *w, uint button, int pressed) {
    (void)w;
    if (button == BTN_LEFT && !pressed && menu_item >= 0) {
        choose(menu_item);
    }
}

static void menu_key(struct gui_window *w, uint code, int pressed) {
    (void)w;
    if (!pressed) {
        return;
    }
    if (code == KEY_ESC) {
        close_menu();
    } else if (code == KEY_DOWN) {
        menu_item = menu_item + 1 < napps ? menu_item + 1 : 0;
        menu_dirty = 1;
    } else if (code == KEY_UP) {
        menu_item = menu_item > 0 ? menu_item - 1 : napps - 1;
        menu_dirty = 1;
    } else if (code == KEY_ENTER) {
        choose(menu_item);
    }
}

static void menu_focus(struct gui_window *w, int focused) {
    (void)w;
    if (!focused) {
        close_menu();
    }
}

static void menu_close(struct gui_window *w) {
    (void)w;
    close_menu();
}

static void menu_frame(struct gui_window *w) {
    (void)w;
    menu_frame_ready = 1;
}

static const struct gui_window_handlers menu_handlers = {
    .close = menu_close,
    .pointer = menu_pointer,
    .leave = menu_leave,
    .button = menu_button,
    .key = menu_key,
    .focus = menu_focus,
    .frame = menu_frame,
};

static void open_menu(void) {
    if (menu != NULL || napps == 0) {
        return;
    }
    menu = gui_layer_open(display, "launcher", GUI_LAYER_TOP, GUI_ANCHOR_TOP | GUI_ANCHOR_LEFT, 0, 1, MENU_W,
                          napps * ITEM_H + 2 * MENU_PAD, &menu_handlers, NULL);
    if (menu == NULL) {
        return;
    }
    menu_item = -1;
    menu_frame_ready = 1;
    menu_dirty = 1;
    menu_closing = 0;
    bar_dirty = 1;
    if (verbose) {
        printf("panel: menu shown\n");
    }
}

/* ---- The bar's input ---- */

static void bar_configure(struct gui_window *w, int width, int height) {
    (void)w;
    (void)width;
    (void)height;
    bar_dirty = 1;
}

static void bar_pointer(struct gui_window *w, int x, int y) {
    (void)w;
    (void)y;
    pointer_x = x;
    int h = bar_hit(x);
    if (h != hover) {
        hover = h;
        bar_dirty = 1;
    }
}

static void bar_leave(struct gui_window *w) {
    (void)w;
    pointer_x = -1;
    if (hover != -1) {
        hover = -1;
        bar_dirty = 1;
    }
}

static void bar_button(struct gui_window *w, uint button, int pressed) {
    (void)w;
    if (!pressed) {
        return;
    }
    int h = bar_hit(pointer_x);
    if (h == -2 && button == BTN_LEFT) {
        if (menu != NULL) {
            close_menu();
        } else {
            open_menu();
        }
        return;
    }
    close_menu();
    const struct gui_toplevel *list;
    int n = gui_toplevels(display, &list);
    if (h >= 0 && h < n) {
        if (button == BTN_LEFT) {
            gui_toplevel_activate(display, list[h].handle);
        } else if (button == BTN_MIDDLE) {
            gui_toplevel_close(display, list[h].handle);
        }
    }
}

static void bar_close(struct gui_window *w) {
    (void)w;
    quit = 1;
}

static void bar_frame(struct gui_window *w) {
    (void)w;
    bar_frame_ready = 1;
}

static const struct gui_window_handlers bar_handlers = {
    .configure = bar_configure,
    .close = bar_close,
    .pointer = bar_pointer,
    .leave = bar_leave,
    .button = bar_button,
    .frame = bar_frame,
};

int main(int argc, char **argv) {
    int first = 1;
    if (argc > 1 && strcmp(argv[1], "-v") == 0) {
        verbose = 1;
        first = 2;
    }
    if (gui_font_load(&font, FONT) < 0) {
        fprintf(2, "panel: no font at %s\n", FONT);
        return 1;
    }
    has_logo = gui_image_load(&logo, LOGO) == 0;
    read_apps();
    int err = 0;
    for (int i = 0; i < CONNECT_TRIES && display == NULL; i++) {
        display = gui_connect(NULL, &err);
        if (display == NULL) {
            sleep(CONNECT_WAIT);
        }
    }
    if (display == NULL) {
        fprintf(2, "panel: can't connect to the display server: %s\n", strerror(-err));
        return 1;
    }
    if (!gui_has_layers(display) || !gui_has_toplevels(display)) {
        fprintf(2, "panel: the display server has no layer shell or foreign toplevels\n");
        return 1;
    }
    refresh();
    bar = gui_layer_open(display, "panel", GUI_LAYER_TOP, GUI_ANCHOR_TOP | GUI_ANCHOR_LEFT | GUI_ANCHOR_RIGHT,
                         BAR_H, 0, 0, BAR_H, &bar_handlers, NULL);
    if (bar == NULL) {
        fprintf(2, "panel: can't open the bar\n");
        return 1;
    }
    if (thread_start(reaper, NULL, reaper_stack, sizeof(reaper_stack)) < 0) {
        fprintf(2, "panel: can't start a thread\n");
        return 1;
    }
    /* The windows already there are announced by now. */
    if (gui_roundtrip(display) < 0) {
        return 0;
    }
    if (verbose) {
        printf("panel: ready, %d applications\n", napps);
    }
    if (argc > first && task_count() == 0) {
        char command[EXEC_MAX] = "";
        size_t used = 0;
        for (int i = first; i < argc; i++) {
            used += (size_t)snprintf(command + used, sizeof(command) - used, "%s%s", i > first ? " " : "",
                                     argv[i]);
            if (used >= sizeof(command)) {
                break;
            }
        }
        launch(command);
    }
    long next_refresh = now_ms() + REFRESH_MS;
    while (!quit) {
        if (menu_closing) {
            menu_gone();
        }
        if (gui_toplevels_changed(display)) {
            bar_dirty = 1;
        }
        if (bar_dirty && bar_frame_ready) {
            draw_bar();
        }
        if (menu != NULL && menu_dirty && menu_frame_ready) {
            draw_menu();
        }
        long left = next_refresh - now_ms();
        if (left <= 0) {
            if (refresh()) {
                bar_dirty = 1;
            }
            next_refresh = now_ms() + REFRESH_MS;
            continue;
        }
        if (gui_dispatch(display, (int)left) < 0) {
            break;
        }
    }
    if (menu != NULL) {
        gui_window_destroy(menu);
    }
    gui_window_destroy(bar);
    gui_disconnect(display);
    return 0;
}
