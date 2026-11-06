/* sysmon: the desktop's system monitor (#185, docs/excalibur-spec.md
 * section 10, "The desktop shell"): a window that shows, read again every
 * second,
 *
 * - each hart, how busy it was over the last second (the harts system
 *   call's ticks that were not idle) as a bar and a percentage, and the
 *   process it runs;
 * - the memory (sysinfo): how much of it is used, as a bar and in KiB,
 *   the time since boot and the number of processes;
 * - the processes (procs): pid, parent, state, size and name, as many as
 *   the window has room for.
 *
 * Exits 0 when the window is closed, 1 if it can't start. */
#include "gui.h"

#define FONT "/usr/share/fonts/spleen/8x16.psfu"

#define WIDTH 400
#define HEIGHT 360
#define MARGIN 8
#define BAR_W 160
#define MAX_HARTS 8
#define MAX_PROCS 32
#define REFRESH_MS 1000

#define BG GUI_RGB(0x18, 0x1e, 0x26)
#define TEXT GUI_RGB(0xe8, 0xe8, 0xe8)
#define HEADING GUI_RGB(0x80, 0xb0, 0xf0)
#define DIM GUI_RGB(0x90, 0x98, 0xa0)
#define BAR_BG GUI_RGB(0x30, 0x3a, 0x46)
#define HART_FG GUI_RGB(0x40, 0xc0, 0x60)
#define MEM_FG GUI_RGB(0x40, 0x80, 0xe0)

static struct gui_font font;
static struct gui_window *win;
static int frame_ready = 1;
static int dirty = 1;
static int quit;

static struct hartinfo last[MAX_HARTS];
static struct hartinfo now_harts[MAX_HARTS];
static int busy[MAX_HARTS];
static int nharts;
static struct sysinfo info;
static struct procinfo procs_now[MAX_PROCS];
static int nprocs;

static long now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

static void refresh(void) {
    int n = harts(now_harts, MAX_HARTS);
    n = n > MAX_HARTS ? MAX_HARTS : n;
    for (int i = 0; i < n; i++) {
        busy[i] = 0;
        if (i < nharts) {
            uint ticks = now_harts[i].ticks - last[i].ticks;
            uint idle = now_harts[i].idle - last[i].idle;
            if (ticks > 0 && idle <= ticks) {
                busy[i] = (int)((ticks - idle) * 100 / ticks);
            }
        }
        last[i] = now_harts[i];
    }
    nharts = n < 0 ? 0 : n;
    sysinfo(&info);
    n = procs(procs_now, MAX_PROCS);
    nprocs = n < 0 ? 0 : n > MAX_PROCS ? MAX_PROCS : n;
    dirty = 1;
}

static const char *state_name(uint state) {
    switch (state) {
    case PROC_RUNNABLE:
        return "ready";
    case PROC_RUNNING:
        return "running";
    case PROC_ZOMBIE:
        return "zombie";
    case PROC_SLEEPING:
        return "sleeping";
    }
    return "?";
}

/* A bar `percent` full, `w` wide, at x, y. */
static void meter(struct gui_buffer *b, int x, int y, int w, int percent, uint color) {
    int h = font.height - 4;
    gui_fill(b, gui_rect(x, y + 2, w, h), BAR_BG);
    gui_fill(b, gui_rect(x, y + 2, w * percent / 100, h), color);
}

static void draw(void) {
    struct gui_buffer *b = gui_window_buffer(win);
    if (b == NULL) {
        quit = 1;
        return;
    }
    char line[96];
    int lh = font.height + 2;
    int y = MARGIN;
    gui_fill(b, gui_rect(0, 0, b->width, b->height), BG);

    gui_text(b, &font, MARGIN, y, "Harts", HEADING, GUI_BOLD);
    y += lh;
    for (int i = 0; i < nharts; i++) {
        snprintf(line, sizeof(line), "hart %u", now_harts[i].hart);
        gui_text(b, &font, MARGIN, y, line, TEXT, 0);
        meter(b, MARGIN + 8 * font.width, y, BAR_W, busy[i], HART_FG);
        if (now_harts[i].pid != 0) {
            snprintf(line, sizeof(line), "%3d%%  pid %u", busy[i], now_harts[i].pid);
        } else {
            snprintf(line, sizeof(line), "%3d%%  idle", busy[i]);
        }
        gui_text(b, &font, MARGIN + 8 * font.width + BAR_W + 8, y, line, DIM, 0);
        y += lh;
    }
    y += lh / 2;

    gui_text(b, &font, MARGIN, y, "Memory", HEADING, GUI_BOLD);
    y += lh;
    unsigned long unit = info.mem_unit != 0 ? info.mem_unit : 1;
    unsigned long total = info.totalram * unit / 1024;
    unsigned long used = (info.totalram - info.freeram) * unit / 1024;
    int percent = total > 0 ? (int)(used * 100 / total) : 0;
    gui_text(b, &font, MARGIN, y, "used", TEXT, 0);
    meter(b, MARGIN + 8 * font.width, y, BAR_W, percent, MEM_FG);
    snprintf(line, sizeof(line), "%lu of %lu KiB", used, total);
    gui_text(b, &font, MARGIN + 8 * font.width + BAR_W + 8, y, line, DIM, 0);
    y += lh;
    long up = info.uptime;
    snprintf(line, sizeof(line), "up %ld:%02ld:%02ld, %u processes", up / 3600, up / 60 % 60, up % 60,
             (uint)info.procs);
    gui_text(b, &font, MARGIN, y, line, TEXT, 0);
    y += lh + lh / 2;

    gui_text(b, &font, MARGIN, y, "Processes", HEADING, GUI_BOLD);
    y += lh;
    gui_text(b, &font, MARGIN, y, "  PID  PPID STATE        SIZE NAME", DIM, 0);
    y += lh;
    for (int i = 0; i < nprocs && y + font.height <= b->height - MARGIN; i++) {
        struct procinfo *p = &procs_now[i];
        snprintf(line, sizeof(line), "%5u %5u %-8s %8zu %s", p->pid, p->ppid, state_name(p->state), p->size,
                 p->name);
        gui_text(b, &font, MARGIN, y, line, TEXT, 0);
        y += lh;
    }

    gui_window_damage(win, gui_rect(0, 0, b->width, b->height));
    if (gui_window_commit(win) < 0) {
        quit = 1;
    }
    frame_ready = 0;
    dirty = 0;
}

static void on_configure(struct gui_window *w, int width, int height) {
    (void)w;
    (void)width;
    (void)height;
    dirty = 1;
}

static void on_close(struct gui_window *w) {
    (void)w;
    quit = 1;
}

static void on_key(struct gui_window *w, uint code, int pressed) {
    (void)w;
    if (pressed && code == KEY_ESC) {
        quit = 1;
    }
}

static void on_frame(struct gui_window *w) {
    (void)w;
    frame_ready = 1;
}

static const struct gui_window_handlers handlers = {
    .configure = on_configure,
    .close = on_close,
    .key = on_key,
    .frame = on_frame,
};

int main(void) {
    int r = gui_font_load(&font, FONT);
    if (r < 0) {
        fprintf(2, "sysmon: %s: %s\n", FONT, strerror(-r));
        return 1;
    }
    struct gui_display *d = gui_connect(NULL, &r);
    if (d == NULL) {
        fprintf(2, "sysmon: can't connect to the display server: %s\n", strerror(-r));
        return 1;
    }
    win = gui_window_open(d, "System Monitor", WIDTH, HEIGHT, &handlers, NULL);
    if (win == NULL) {
        fprintf(2, "sysmon: can't open a window\n");
        return 1;
    }
    refresh();
    long next = now_ms() + REFRESH_MS;
    while (!quit) {
        if (dirty && frame_ready) {
            draw();
        }
        long left = next - now_ms();
        if (left <= 0) {
            refresh();
            next = now_ms() + REFRESH_MS;
            continue;
        }
        if (gui_dispatch(d, (int)left) < 0) {
            break;
        }
    }
    gui_window_destroy(win);
    gui_disconnect(d);
    return 0;
}
