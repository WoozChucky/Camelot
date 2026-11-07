/* term: the desktop's terminal emulator (#185, docs/excalibur-spec.md
 * section 10, "The desktop shell"): a window on the display server with a
 * shell on a pseudo-terminal, `term [-v] [-T TITLE] [PROGRAM ARGS...]`
 * (/bin/sh by default; the window's title "Terminal" unless -T, xterm's,
 * gives one).
 *
 * - The program runs on a pty's slave (/dev/pts/N, its standard input,
 *   output and error); what it writes is read from the master and goes
 *   through the escape sequence parser the kernel's framebuffer console
 *   shares (kernel/vt.c), whose actions change a grid of cells: a VT100's
 *   and ANSI's subset, what Linux's console and xterm take for the same.
 * - Keys go to the master as a VT100's (xterm's, for the keys it lacks):
 *   characters as UTF-8, Ctrl+letter as its control character, Alt as an
 *   ESC before it, the arrows and the editing keys as their sequences.
 *   The display server sends no repeats: a held key repeats here. What
 *   goes to the program is written by a thread of its own (input_ring),
 *   so a paste the program is slow to read never stops its output being
 *   read.
 * - Lines that scroll off the top go into the scrollback (SCROLLBACK lines),
 *   read back with the wheel or Shift+PageUp/PageDown; anything typed
 *   comes back to the bottom.
 * - Dragging with the left button selects text, which is copied to the
 *   clipboard (wl_data_device, which every terminal shares) when the
 *   button is let go; Ctrl+Shift+C copies too, and Ctrl+Shift+V,
 *   Shift+Insert or the middle button paste it.
 * - The window is resized at its corner or edges: the grid follows, and
 *   TIOCSWINSZ tells the program its rows and columns.
 *
 * When the program ends (the slave's last descriptor is closed: the
 * master reads -EIO) the window goes, and term exits 0; 1 if it can't
 * start, 2 on a usage error. -v says on standard error, each time a frame
 * is committed, what the cursor's line holds up to the cursor
 * (`term "Terminal": drawn "# "`, after the title), which is how a test
 * knows the program's output is on the screen. */
#include "gui.h"
#include "../vt.c"

#define FONT "/usr/share/fonts/spleen/8x16.psfu"

/* The grid's limits, and its size at first. */
#define MAX_COLS 200
#define MAX_ROWS 80
#define COLS 80
#define ROWS 24
#define SCROLLBACK 1000
/* The space round the grid, pixels. */
#define PAD 2

/* A held key repeats after REPEAT_DELAY ms, every REPEAT_RATE ms. */
#define REPEAT_DELAY 500
#define REPEAT_RATE 40

/* A cell's attributes: its colours (palette indices, or DEFAULT) and
 * flags. */
#define DEFAULT_FG 7
#define DEFAULT_BG 0
#define A_BOLD 0x01
#define A_REVERSE 0x02
#define A_UNDERLINE 0x04

/* The colours: Linux's console's, which fbcon.c draws with too, the
 * bright ones for bold. */
static const uint palette[16] = {
    0x000000, 0xaa0000, 0x00aa00, 0xaa5500, 0x0000aa, 0xaa00aa, 0x00aaaa, 0xaaaaaa,
    0x555555, 0xff5555, 0x55ff55, 0xffff55, 0x5555ff, 0xff55ff, 0x55ffff, 0xffffff,
};
#define SELECTION_BG GUI_RGB(0x30, 0x60, 0xa0)

struct cell {
    uint ch; /* a code point, ' ' when blank */
    uchar fg;
    uchar bg;
    uchar attr;
    uchar pad;
};

struct line {
    struct cell c[MAX_COLS];
};

static struct gui_display *display;
static struct gui_window *win;
static struct gui_font font;
static struct gui_keyboard kbd;
static struct vt_parser vt;
static int master = -1;
static int child = -1;
static int quit;
static int focused;
static int verbose;
static const char *title = "Terminal";

/* The screen: rows x cols cells, `screen[r]` row r. */
static int rows;
static int cols;
static struct line *screen[MAX_ROWS];
/* The scrollback, a ring: the oldest of `nhist` lines at hist_start. */
static struct line *hist;
static int hist_start;
static int nhist;
/* How many lines up from the bottom the view is (0: the screen). */
static int view;

/* What goes to the program (keys, pastes, answers) waits in a ring for
 * the writer thread, which writes it to the master: a write sleeps while
 * the pty's input is full, until the program reads, and the program may
 * itself be waiting for its output to be read, which the main loop goes
 * on doing meanwhile. Bytes that find the ring full are dropped, as a
 * terminal's overflowing input is. `input_seq` changes with each lot
 * queued (the writer sleeps on it). */
#define INPUT_RING 16384
static char input_ring[INPUT_RING];
static int input_tail;
static int input_len;
static int input_seq;
static struct mutex input_lock;
static uchar writer_stack[8192];

/* The cursor; col == cols after the last column was written: the next
 * character wraps first (a VT100's deferred wrap). */
static int cur_row;
static int cur_col;
static int cursor_shown = 1;
static int saved_row;
static int saved_col;
static uchar saved_fg = DEFAULT_FG;
static uchar saved_bg = DEFAULT_BG;
static uchar saved_attr;
/* What is written next: its colours and attributes. */
static uchar pen_fg = DEFAULT_FG;
static uchar pen_bg = DEFAULT_BG;
static uchar pen_attr;
/* The scrolling region, rows top to bottom (DECSTBM). */
static int top;
static int bottom;
/* DEC's modes: DECCKM (the cursor keys as ESC O), DECAWM (wrap). */
static int app_cursor;
static int autowrap = 1;
/* A UTF-8 sequence being read: the code point so far, bytes still to come. */
static uint utf8_code;
static int utf8_left;

/* Rows of the view to draw again (a bit each), and all of it. */
static uchar row_dirty[MAX_ROWS];
static int all_dirty = 1;
static int frame_ready = 1;
/* Where the cursor was drawn. */
static int drawn_row = -1;
static int drawn_col = -1;

/* The selection: from its anchor to where the pointer is, in lines of the
 * scrollback and the screen together (0 the oldest) and columns. */
static int selecting;
static int has_selection;
static int sel_line0;
static int sel_col0;
static int sel_line1;
static int sel_col1;
static int ptr_x;
static int ptr_y;

/* The key repeating, its next time. */
static uint repeat_code;
static uint repeat_at;

/* Milliseconds on the monotonic clock. */
static uint now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint)(ts.tv_sec * 1000 + ts.tv_nsec / 1000000);
}

/* ---- The grid ---- */

static struct cell blank(void) {
    struct cell c;
    c.ch = ' ';
    c.fg = pen_fg;
    c.bg = pen_bg;
    c.attr = 0;
    c.pad = 0;
    return c;
}

static void clear_cells(struct line *l, int from, int to) {
    struct cell b = blank();
    for (int i = from; i < to && i < MAX_COLS; i++) {
        l->c[i] = b;
    }
}

static void dirty_row(int r) {
    if (r >= 0 && r < MAX_ROWS) {
        row_dirty[r] = 1;
    }
}

static void dirty_all(void) {
    all_dirty = 1;
}

/* Something changed on the screen: a view scrolled back stays where it is
 * (it is the screen's rows that changed), a selection is dropped. */
static void changed(int r) {
    if (has_selection) {
        has_selection = 0;
        dirty_all();
    }
    dirty_row(r + view);
}

/* Line `n` of the scrollback and the screen together. */
static struct line *line_at(int n) {
    if (n < nhist) {
        return &hist[(hist_start + n) % SCROLLBACK];
    }
    return screen[n - nhist];
}

/* The top row of the screen goes into the scrollback. */
static void push_history(struct line *l) {
    if (hist == NULL) {
        return;
    }
    int slot;
    if (nhist < SCROLLBACK) {
        slot = (hist_start + nhist) % SCROLLBACK;
        nhist++;
    } else {
        /* Full: the oldest line goes, and every other is one older. */
        slot = hist_start;
        hist_start = (hist_start + 1) % SCROLLBACK;
    }
    /* A view scrolled back stays on the lines it shows, one further up;
     * at the top already, it stays there, on what is the top now. */
    if (view > 0 && view < nhist) {
        view++;
    }
    hist[slot] = *l;
}

/* Rows top..bottom up `n`, blank rows in at the bottom. */
static void scroll_up(int from, int to, int n) {
    for (int k = 0; k < n; k++) {
        struct line *gone = screen[from];
        if (from == 0 && to == rows - 1) {
            push_history(gone);
        }
        for (int r = from; r < to; r++) {
            screen[r] = screen[r + 1];
        }
        screen[to] = gone;
        clear_cells(gone, 0, MAX_COLS);
    }
    if (has_selection) {
        has_selection = 0;
    }
    dirty_all();
}

/* Rows top..bottom down `n`, blank rows in at the top. */
static void scroll_down(int from, int to, int n) {
    for (int k = 0; k < n; k++) {
        struct line *gone = screen[to];
        for (int r = to; r > from; r--) {
            screen[r] = screen[r - 1];
        }
        screen[from] = gone;
        clear_cells(gone, 0, MAX_COLS);
    }
    has_selection = 0;
    dirty_all();
}

/* LF: down a row, scrolling the region at its bottom. */
static void index_down(void) {
    if (cur_row == bottom) {
        scroll_up(top, bottom, 1);
    } else if (cur_row < rows - 1) {
        cur_row++;
    }
}

/* RI: up a row, scrolling the region down at its top. */
static void index_up(void) {
    if (cur_row == top) {
        scroll_down(top, bottom, 1);
    } else if (cur_row > 0) {
        cur_row--;
    }
}

static void put_char(uint ch) {
    if (cur_col >= cols) {
        if (autowrap) {
            cur_col = 0;
            index_down();
        } else {
            cur_col = cols - 1;
        }
    }
    struct cell *c = &screen[cur_row]->c[cur_col];
    c->ch = ch;
    c->fg = pen_fg;
    c->bg = pen_bg;
    c->attr = pen_attr;
    changed(cur_row);
    cur_col++;
}

static void reset(void) {
    pen_fg = DEFAULT_FG;
    pen_bg = DEFAULT_BG;
    pen_attr = 0;
    top = 0;
    bottom = rows - 1;
    app_cursor = 0;
    autowrap = 1;
    cursor_shown = 1;
    cur_row = 0;
    cur_col = 0;
    for (int r = 0; r < rows; r++) {
        clear_cells(screen[r], 0, MAX_COLS);
    }
    dirty_all();
}

/* ---- What the program writes ---- */

static void sgr(void) {
    int n = vt.nparams > 0 ? vt.nparams : 1;
    for (int i = 0; i < n; i++) {
        uint p = vt.params[i];
        if (p == 0) {
            pen_fg = DEFAULT_FG;
            pen_bg = DEFAULT_BG;
            pen_attr = 0;
        } else if (p == 1) {
            pen_attr |= A_BOLD;
        } else if (p == 4) {
            pen_attr |= A_UNDERLINE;
        } else if (p == 7) {
            pen_attr |= A_REVERSE;
        } else if (p == 22) {
            pen_attr &= (uchar)~A_BOLD;
        } else if (p == 24) {
            pen_attr &= (uchar)~A_UNDERLINE;
        } else if (p == 27) {
            pen_attr &= (uchar)~A_REVERSE;
        } else if (p >= 30 && p <= 37) {
            pen_fg = (uchar)(p - 30);
        } else if (p == 39) {
            pen_fg = DEFAULT_FG;
        } else if (p >= 40 && p <= 47) {
            pen_bg = (uchar)(p - 40);
        } else if (p == 49) {
            pen_bg = DEFAULT_BG;
        } else if (p >= 90 && p <= 97) {
            pen_fg = (uchar)(p - 90 + 8);
        } else if (p >= 100 && p <= 107) {
            pen_bg = (uchar)(p - 100 + 8);
        } else if ((p == 38 || p == 48) && i + 1 < n) {
            /* 256 colours (5;N) and RGB (2;R;G;B): their places skipped,
             * the first 16 taken. */
            if (vt.params[i + 1] == 5 && i + 2 < n) {
                uint c = vt.params[i + 2];
                if (c < 16) {
                    if (p == 38) {
                        pen_fg = (uchar)c;
                    } else {
                        pen_bg = (uchar)c;
                    }
                }
                i += 2;
            } else if (vt.params[i + 1] == 2) {
                i += 4;
            }
        }
    }
}

static int clamp(int v, int lo, int hi) {
    return v < lo ? lo : v > hi ? hi : v;
}

/* Erases columns from..to of row r (with the pen's background). */
static void erase(int r, int from, int to) {
    clear_cells(screen[r], from, to);
    changed(r);
}

/* Queues n bytes for the program. */
static void send_input(const char *s, size_t n) {
    mutex_lock(&input_lock);
    for (size_t i = 0; i < n && input_len < INPUT_RING; i++) {
        input_ring[(input_tail + input_len) % INPUT_RING] = s[i];
        input_len++;
    }
    atomic_add(&input_seq, 1);
    mutex_unlock(&input_lock);
    futex_wake(&input_seq, 1);
}

/* The writer thread: what is queued, to the master, until the program
 * has gone. */
static int writer(void *arg) {
    (void)arg;
    char chunk[256];
    for (;;) {
        mutex_lock(&input_lock);
        int n = 0;
        while (n < (int)sizeof(chunk) && input_len > 0) {
            chunk[n++] = input_ring[input_tail];
            input_tail = (input_tail + 1) % INPUT_RING;
            input_len--;
        }
        int seen = atomic_load(&input_seq);
        mutex_unlock(&input_lock);
        if (n == 0) {
            futex_wait(&input_seq, seen);
        } else if (write(master, chunk, (size_t)n) < 0) {
            return 0;
        }
    }
    return 0;
}

/* What the terminal answers (DSR, DA) goes to the program. */
static void reply(const char *s) {
    send_input(s, strlen(s));
}

static void private_mode(int set) {
    for (int i = 0; i < (vt.nparams > 0 ? vt.nparams : 1); i++) {
        uint p = vt.params[i];
        if (p == 1) {
            app_cursor = set;
        } else if (p == 7) {
            autowrap = set;
        } else if (p == 25) {
            cursor_shown = set;
            changed(cur_row);
        }
        /* The rest (the alternate screen, the mouse's reports, bracketed
         * paste) are taken and do nothing. */
    }
}

static void csi(uchar final) {
    if (vt.marker == '?') {
        if (final == 'h' || final == 'l') {
            private_mode(final == 'h');
        }
        return;
    }
    if (vt.marker != 0 || vt.intermediate != 0) {
        if (final == 'c' && vt.marker == '>') {
            reply("\033[>0;0;0c"); /* secondary DA */
        }
        return;
    }
    int n = (int)vt_param(&vt, 0, 1);
    int c = cur_col < cols ? cur_col : cols - 1;
    switch (final) {
    case 'm':
        sgr();
        break;
    case 'A':
        cur_row = clamp(cur_row - n, cur_row >= top ? top : 0, rows - 1);
        cur_col = c;
        break;
    case 'B':
        cur_row = clamp(cur_row + n, 0, cur_row <= bottom ? bottom : rows - 1);
        cur_col = c;
        break;
    case 'C':
        cur_col = clamp(c + n, 0, cols - 1);
        break;
    case 'D':
        cur_col = clamp(c - n, 0, cols - 1);
        break;
    case 'E':
        cur_row = clamp(cur_row + n, 0, rows - 1);
        cur_col = 0;
        break;
    case 'F':
        cur_row = clamp(cur_row - n, 0, rows - 1);
        cur_col = 0;
        break;
    case 'G':
    case '`':
        cur_col = clamp(n - 1, 0, cols - 1);
        break;
    case 'd':
        cur_row = clamp(n - 1, 0, rows - 1);
        cur_col = c;
        break;
    case 'H':
    case 'f':
        cur_row = clamp((int)vt_param(&vt, 0, 1) - 1, 0, rows - 1);
        cur_col = clamp((int)vt_param(&vt, 1, 1) - 1, 0, cols - 1);
        break;
    case 'J': {
        uint how = vt.params[0];
        if (how == 0) {
            erase(cur_row, c, cols);
            for (int r = cur_row + 1; r < rows; r++) {
                erase(r, 0, cols);
            }
        } else if (how == 1) {
            for (int r = 0; r < cur_row; r++) {
                erase(r, 0, cols);
            }
            erase(cur_row, 0, c + 1);
        } else if (how == 2 || how == 3) {
            for (int r = 0; r < rows; r++) {
                erase(r, 0, cols);
            }
        }
        break;
    }
    case 'K': {
        uint how = vt.params[0];
        if (how == 0) {
            erase(cur_row, c, cols);
        } else if (how == 1) {
            erase(cur_row, 0, c + 1);
        } else if (how == 2) {
            erase(cur_row, 0, cols);
        }
        break;
    }
    case 'X':
        erase(cur_row, c, c + n < cols ? c + n : cols);
        break;
    case '@': {
        /* ICH: blanks in at the cursor, the rest of the row to the right. */
        struct line *l = screen[cur_row];
        n = n < cols - c ? n : cols - c;
        memmove(&l->c[c + n], &l->c[c], (size_t)(cols - c - n) * sizeof(struct cell));
        clear_cells(l, c, c + n);
        changed(cur_row);
        cur_col = c;
        break;
    }
    case 'P': {
        /* DCH: the cursor's cells out, the rest of the row to the left. */
        struct line *l = screen[cur_row];
        n = n < cols - c ? n : cols - c;
        memmove(&l->c[c], &l->c[c + n], (size_t)(cols - c - n) * sizeof(struct cell));
        clear_cells(l, cols - n, cols);
        changed(cur_row);
        cur_col = c;
        break;
    }
    case 'L':
        if (cur_row >= top && cur_row <= bottom) {
            scroll_down(cur_row, bottom, n < bottom - cur_row + 1 ? n : bottom - cur_row + 1);
            cur_col = 0;
        }
        break;
    case 'M':
        if (cur_row >= top && cur_row <= bottom) {
            scroll_up(cur_row, bottom, n < bottom - cur_row + 1 ? n : bottom - cur_row + 1);
            cur_col = 0;
        }
        break;
    case 'S':
        scroll_up(top, bottom, n < bottom - top + 1 ? n : bottom - top + 1);
        break;
    case 'T':
        scroll_down(top, bottom, n < bottom - top + 1 ? n : bottom - top + 1);
        break;
    case 'r': {
        int t = (int)vt_param(&vt, 0, 1) - 1;
        int b = (int)vt_param(&vt, 1, (uint)rows) - 1;
        if (t < b && b < rows) {
            top = t;
            bottom = b;
            cur_row = 0;
            cur_col = 0;
        }
        break;
    }
    case 's':
        saved_row = cur_row;
        saved_col = cur_col;
        break;
    case 'u':
        cur_row = clamp(saved_row, 0, rows - 1);
        cur_col = clamp(saved_col, 0, cols - 1);
        break;
    case 'n':
        if (vt.params[0] == 6) {
            char buf[32];
            snprintf(buf, sizeof(buf), "\033[%d;%dR", cur_row + 1, c + 1);
            reply(buf);
        } else if (vt.params[0] == 5) {
            reply("\033[0n");
        }
        break;
    case 'c':
        reply("\033[?1;2c"); /* a VT100 with the advanced video option */
        break;
    default:
        break;
    }
}

static void esc(uchar final) {
    if (vt.intermediate != 0) {
        return; /* character sets (ESC ( B...): the one there is */
    }
    switch (final) {
    case '7':
        saved_row = cur_row;
        saved_col = cur_col;
        saved_fg = pen_fg;
        saved_bg = pen_bg;
        saved_attr = pen_attr;
        break;
    case '8':
        cur_row = clamp(saved_row, 0, rows - 1);
        cur_col = clamp(saved_col, 0, cols);
        pen_fg = saved_fg;
        pen_bg = saved_bg;
        pen_attr = saved_attr;
        break;
    case 'D':
        index_down();
        break;
    case 'E':
        cur_col = 0;
        index_down();
        break;
    case 'M':
        index_up();
        break;
    case 'c':
        reset();
        break;
    default:
        break;
    }
}

static void execute(uchar ch) {
    switch (ch) {
    case '\n':
    case '\v':
    case '\f':
        index_down();
        break;
    case '\r':
        cur_col = 0;
        break;
    case '\b':
        if (cur_col >= cols) {
            cur_col = cols - 1;
        }
        if (cur_col > 0) {
            cur_col--;
        }
        break;
    case '\t':
        if (cur_col < cols) {
            cur_col = (cur_col / 8 + 1) * 8;
            if (cur_col > cols - 1) {
                cur_col = cols - 1;
            }
        }
        break;
    default:
        break; /* BEL and the rest: nothing */
    }
}

/* A byte the program wrote. */
static void output_byte(uchar b) {
    int action = vt_parse(&vt, b);
    if (action == VT_PRINT) {
        if (b < 0x80) {
            utf8_left = 0;
            put_char(b);
        } else if ((b & 0xc0) == 0x80 && utf8_left > 0) {
            utf8_code = utf8_code << 6 | (b & 0x3f);
            if (--utf8_left == 0) {
                put_char(utf8_code);
            }
        } else if ((b & 0xe0) == 0xc0) {
            utf8_code = b & 0x1f;
            utf8_left = 1;
        } else if ((b & 0xf0) == 0xe0) {
            utf8_code = b & 0x0f;
            utf8_left = 2;
        } else if ((b & 0xf8) == 0xf0) {
            utf8_code = b & 0x07;
            utf8_left = 3;
        } else {
            utf8_left = 0;
            put_char(0xfffd);
        }
        return;
    }
    if (action != VT_NONE) {
        utf8_left = 0;
    }
    int row = cur_row;
    if (action == VT_EXECUTE) {
        execute(b);
    } else if (action == VT_CSI) {
        csi(b);
    } else if (action == VT_ESC) {
        esc(b);
    }
    if (cur_row != row) {
        dirty_row(row + view);
    }
}

/* ---- Drawing ---- */

static int selection_has(int line, int col) {
    if (!has_selection) {
        return 0;
    }
    int l0 = sel_line0;
    int c0 = sel_col0;
    int l1 = sel_line1;
    int c1 = sel_col1;
    if (l1 < l0 || (l1 == l0 && c1 < c0)) {
        l0 = sel_line1;
        c0 = sel_col1;
        l1 = sel_line0;
        c1 = sel_col0;
    }
    if (line < l0 || line > l1) {
        return 0;
    }
    if (line == l0 && col < c0) {
        return 0;
    }
    if (line == l1 && col >= c1) {
        return 0;
    }
    return 1;
}

/* The view's row r: which line it shows. */
static int view_line(int r) {
    return nhist - view + r;
}

static void draw_row(struct gui_buffer *b, int r) {
    int y = PAD + r * font.height;
    struct gui_rect area = gui_rect(0, y, b->width, font.height);
    if (r == rows - 1) {
        area.h = b->height - y; /* the padding below the last row too */
    }
    gui_fill(b, area, GUI_RGB(0, 0, 0));
    int n = view_line(r);
    struct line *l = n >= 0 ? line_at(n) : NULL;
    int show_cursor = n == nhist + cur_row && cursor_shown;
    int cc = cur_col < cols ? cur_col : cols - 1;
    for (int col = 0; col < cols && l != NULL; col++) {
        struct cell *c = &l->c[col];
        uint fg = c->fg;
        uint bg = c->bg;
        if (c->attr & A_BOLD && fg < 8) {
            fg += 8;
        }
        uint fgc = GUI_RGB(0, 0, 0) | palette[fg];
        uint bgc = GUI_RGB(0, 0, 0) | palette[bg];
        if (c->attr & A_REVERSE) {
            uint t = fgc;
            fgc = bgc;
            bgc = t;
        }
        if (selection_has(n, col)) {
            bgc = SELECTION_BG;
            fgc = GUI_RGB(0xff, 0xff, 0xff);
        }
        int cursor_here = show_cursor && col == cc;
        if (cursor_here && focused) {
            uint t = fgc;
            fgc = bgc;
            bgc = t == bgc ? GUI_RGB(0xaa, 0xaa, 0xaa) : t;
        }
        int x = PAD + col * font.width;
        struct gui_rect cell = gui_rect(x, y, font.width, font.height);
        gui_fill(b, cell, bgc);
        if (c->ch != ' ' && c->ch != 0) {
            char s[5];
            s[gui_utf8_put(s, c->ch)] = 0;
            gui_text(b, &font, x, y, s, fgc, 0);
        }
        if (c->attr & A_UNDERLINE) {
            gui_fill(b, gui_rect(x, y + font.height - 1, font.width, 1), fgc);
        }
        if (cursor_here && !focused) {
            gui_outline(b, cell, GUI_RGB(0xaa, 0xaa, 0xaa));
        }
    }
    gui_window_damage(win, area);
}

/* -v's line: the cursor's line up to the cursor, as UTF-8. */
static void say_drawn(void) {
    char text[MAX_COLS * 4 + 1];
    int n = 0;
    for (int c = 0; c < cur_col && c < cols; c++) {
        uint ch = screen[cur_row]->c[c].ch;
        n += gui_utf8_put(text + n, ch != 0 ? ch : ' ');
    }
    text[n] = 0;
    fprintf(2, "term \"%s\": drawn \"%s\"\n", title, text);
}

/* The window's size the grid was last laid out for (resize). */
static int laid_w;
static int laid_h;
static void resize(int width, int height);

static void draw(void) {
    struct gui_buffer *b = gui_window_buffer(win);
    if (b == NULL) {
        quit = 1;
        return;
    }
    /* A configure that came while it waited for a free buffer has the
     * buffer at the new size: the grid follows first, all of it drawn. */
    if (b->width != laid_w || b->height != laid_h) {
        resize(b->width, b->height);
    }
    if (all_dirty) {
        gui_fill(b, gui_rect(0, 0, b->width, PAD), GUI_RGB(0, 0, 0));
        gui_window_damage(win, gui_rect(0, 0, b->width, PAD));
    }
    /* The rows the cursor left and came to. */
    if (drawn_row >= 0 && (drawn_row != cur_row + view || drawn_col != cur_col)) {
        dirty_row(drawn_row);
    }
    dirty_row(cur_row + view);
    for (int r = 0; r < rows; r++) {
        if (all_dirty || row_dirty[r]) {
            draw_row(b, r);
        }
        row_dirty[r] = 0;
    }
    all_dirty = 0;
    drawn_row = cur_row + view;
    drawn_col = cur_col;
    if (gui_window_commit(win) < 0) {
        quit = 1;
    }
    frame_ready = 0;
    if (verbose) {
        say_drawn();
    }
}

/* ---- The size ---- */

/* The window is `width` x `height` pixels: the grid fits it, and the
 * program is told. */
static void resize(int width, int height) {
    laid_w = width;
    laid_h = height;
    int c = (width - 2 * PAD) / font.width;
    int r = (height - 2 * PAD) / font.height;
    c = clamp(c, 2, MAX_COLS);
    r = clamp(r, 2, MAX_ROWS);
    if (c == cols && r == rows) {
        dirty_all();
        return;
    }
    /* Fewer rows: the top ones go into the scrollback, as far as the
     * cursor allows; more: lines come back from it. */
    while (r < rows) {
        if (cur_row > 0 && cur_row == rows - 1) {
            struct line *gone = screen[0];
            push_history(gone);
            for (int i = 0; i < rows - 1; i++) {
                screen[i] = screen[i + 1];
            }
            screen[rows - 1] = gone;
            cur_row--;
        }
        rows--;
        clear_cells(screen[rows], 0, MAX_COLS);
    }
    while (r > rows) {
        clear_cells(screen[rows], 0, MAX_COLS);
        rows++;
        if (nhist > 0 && view == 0) {
            struct line *l = screen[rows - 1];
            for (int i = rows - 1; i > 0; i--) {
                screen[i] = screen[i - 1];
            }
            nhist--;
            *l = hist[(hist_start + nhist) % SCROLLBACK];
            screen[0] = l;
            cur_row++;
        }
    }
    for (int i = 0; i < rows; i++) {
        if (c > cols) {
            clear_cells(screen[i], cols, c);
        }
    }
    cols = c;
    top = 0;
    bottom = rows - 1;
    cur_row = clamp(cur_row, 0, rows - 1);
    cur_col = clamp(cur_col, 0, cols);
    if (view > nhist) {
        view = nhist;
    }
    has_selection = 0;
    struct winsize ws;
    ws.ws_row = (ushort)rows;
    ws.ws_col = (ushort)cols;
    ws.ws_xpixel = (ushort)(cols * font.width);
    ws.ws_ypixel = (ushort)(rows * font.height);
    ioctl(master, TIOCSWINSZ, &ws);
    dirty_all();
}

/* ---- The clipboard ---- */

/* The selection's text: its lines' cells, trailing blanks cut, a '\n'
 * between lines. */
static char *selection_text(size_t *len) {
    int l0 = sel_line0;
    int c0 = sel_col0;
    int l1 = sel_line1;
    int c1 = sel_col1;
    if (l1 < l0 || (l1 == l0 && c1 < c0)) {
        l0 = sel_line1;
        c0 = sel_col1;
        l1 = sel_line0;
        c1 = sel_col0;
    }
    size_t cap = (size_t)(l1 - l0 + 1) * (MAX_COLS * 4 + 1) + 1;
    char *s = malloc(cap);
    if (s == NULL) {
        return NULL;
    }
    size_t n = 0;
    for (int line = l0; line <= l1; line++) {
        struct line *l = line_at(line);
        int from = line == l0 ? c0 : 0;
        int to = line == l1 ? c1 : cols;
        int end = to;
        while (end > from && (l->c[end - 1].ch == ' ' || l->c[end - 1].ch == 0)) {
            end--;
        }
        for (int col = from; col < end; col++) {
            n += (size_t)gui_utf8_put(s + n, l->c[col].ch != 0 ? l->c[col].ch : ' ');
        }
        if (line < l1) {
            s[n++] = '\n';
        }
    }
    s[n] = 0;
    *len = n;
    return s;
}

static void copy(void) {
    if (!has_selection) {
        return;
    }
    size_t len;
    char *s = selection_text(&len);
    if (s != NULL) {
        if (len > 0) {
            gui_clipboard_set(display, s, len);
        }
        free(s);
    }
}

static void paste(void) {
    char *s;
    size_t len;
    if (gui_clipboard_get(display, &s, &len) < 0) {
        return;
    }
    /* A new line is Enter, as typed. */
    for (size_t i = 0; i < len; i++) {
        if (s[i] == '\n') {
            s[i] = '\r';
        }
    }
    send_input(s, len);
    free(s);
    if (view != 0) {
        view = 0;
        dirty_all();
    }
}

/* ---- Input ---- */

static void type(const char *s) {
    send_input(s, strlen(s));
    if (view != 0) {
        view = 0;
        dirty_all();
    }
}

static void scroll_view(int lines) {
    int v = clamp(view + lines, 0, nhist);
    if (v != view) {
        view = v;
        dirty_all();
    }
}

/* The sequence of a key that types no character, or NULL. */
static const char *key_sequence(uint code) {
    switch (code) {
    case KEY_UP:
        return app_cursor ? "\033OA" : "\033[A";
    case KEY_DOWN:
        return app_cursor ? "\033OB" : "\033[B";
    case KEY_RIGHT:
        return app_cursor ? "\033OC" : "\033[C";
    case KEY_LEFT:
        return app_cursor ? "\033OD" : "\033[D";
    case KEY_HOME:
        return "\033[H";
    case KEY_END:
        return "\033[F";
    case KEY_INSERT:
        return "\033[2~";
    case KEY_DELETE:
        return "\033[3~";
    case KEY_PAGEUP:
        return "\033[5~";
    case KEY_PAGEDOWN:
        return "\033[6~";
    case KEY_F1:
        return "\033OP";
    case KEY_F2:
        return "\033OQ";
    case KEY_F3:
        return "\033OR";
    case KEY_F4:
        return "\033OS";
    case KEY_ENTER:
        return "\r";
    case KEY_BACKSPACE:
        return "\177";
    case KEY_TAB:
        return "\t";
    case KEY_ESC:
        return "\033";
    }
    return NULL;
}

/* A key pressed (or repeating): what it types. */
static void key_press(uint code) {
    uint mods = kbd.mods;
    int shift = (mods & GUI_MOD_SHIFT) != 0;
    int ctrl = (mods & GUI_MOD_CTRL) != 0;
    int alt = (mods & GUI_MOD_ALT) != 0;
    if (shift && code == KEY_PAGEUP) {
        scroll_view(rows / 2);
        return;
    }
    if (shift && code == KEY_PAGEDOWN) {
        scroll_view(-(rows / 2));
        return;
    }
    if (shift && code == KEY_INSERT) {
        paste();
        return;
    }
    if (ctrl && shift && code == KEY_C) {
        copy();
        return;
    }
    if (ctrl && shift && code == KEY_V) {
        paste();
        return;
    }
    const char *seq = key_sequence(code);
    if (seq != NULL) {
        if (alt) {
            type("\033");
        }
        type(seq);
        return;
    }
    uint ch = gui_keymap_char(kbd.keymap, code, mods);
    if (ch == 0) {
        return;
    }
    char buf[8];
    int n = 0;
    if (alt) {
        buf[n++] = '\033';
    }
    if (ctrl) {
        /* Ctrl+@ to Ctrl+_ (and the letters either case) as C0 controls,
         * Ctrl+? as DEL. */
        if (ch >= 'a' && ch <= 'z') {
            ch -= 'a' - 'A';
        }
        if (ch >= '@' && ch <= '_') {
            buf[n++] = (char)(ch - '@');
        } else if (ch == ' ' || ch == '2') {
            buf[n++] = 0;
        } else if (ch == '?') {
            buf[n++] = 0x7f;
        } else {
            return;
        }
    } else {
        n += gui_utf8_put(buf + n, ch);
    }
    send_input(buf, (size_t)n);
    if (view != 0) {
        view = 0;
        dirty_all();
    }
}

static int is_modifier(uint code) {
    return code == KEY_LEFTSHIFT || code == KEY_RIGHTSHIFT || code == KEY_LEFTCTRL || code == KEY_RIGHTCTRL ||
           code == KEY_LEFTALT || code == KEY_RIGHTALT || code == KEY_LEFTMETA || code == KEY_RIGHTMETA ||
           code == KEY_CAPSLOCK;
}

static void on_key(struct gui_window *w, uint code, int pressed) {
    (void)w;
    gui_keyboard_key(&kbd, code, pressed);
    if (is_modifier(code)) {
        return;
    }
    if (!pressed) {
        if (code == repeat_code) {
            repeat_code = 0;
        }
        return;
    }
    key_press(code);
    repeat_code = code;
    repeat_at = now_ms() + REPEAT_DELAY;
}

static void on_modifiers(struct gui_window *w, uint held, uint locked) {
    (void)w;
    gui_keyboard_modifiers(&kbd, held, locked);
}

static void on_focus(struct gui_window *w, int f) {
    (void)w;
    focused = f;
    if (!f) {
        gui_keyboard_modifiers(&kbd, 0, kbd.mods);
        repeat_code = 0;
    }
    changed(cur_row);
}

/* The cell at window pixel x, y: its line and column. */
static void cell_at(int x, int y, int *line, int *col) {
    int r = clamp((y - PAD) / font.height, 0, rows - 1);
    int c = clamp((x - PAD + font.width / 2) / font.width, 0, cols);
    *line = view_line(r);
    *col = c;
}

static void on_pointer(struct gui_window *w, int x, int y) {
    (void)w;
    ptr_x = x;
    ptr_y = y;
    if (selecting) {
        int line;
        int col;
        cell_at(x, y, &line, &col);
        if (line != sel_line1 || col != sel_col1) {
            sel_line1 = line;
            sel_col1 = col;
            has_selection = sel_line0 != sel_line1 || sel_col0 != sel_col1;
            dirty_all();
        }
    }
}

static void on_button(struct gui_window *w, uint button, int pressed) {
    (void)w;
    if (button == BTN_LEFT) {
        if (pressed) {
            int line;
            int col;
            cell_at(ptr_x, ptr_y, &line, &col);
            selecting = 1;
            if (has_selection) {
                dirty_all();
            }
            has_selection = 0;
            sel_line0 = sel_line1 = line;
            sel_col0 = sel_col1 = col;
        } else if (selecting) {
            selecting = 0;
            copy();
        }
    } else if (button == BTN_MIDDLE && pressed) {
        paste();
    }
}

static void on_scroll(struct gui_window *w, int amount) {
    (void)w;
    scroll_view(-amount * 3);
}

static void on_close(struct gui_window *w) {
    (void)w;
    quit = 1;
}

static void on_frame(struct gui_window *w) {
    (void)w;
    frame_ready = 1;
}

static const struct gui_window_handlers handlers = {
    .close = on_close,
    .pointer = on_pointer,
    .button = on_button,
    .scroll = on_scroll,
    .key = on_key,
    .modifiers = on_modifiers,
    .focus = on_focus,
    .frame = on_frame,
};

/* ---- The program ---- */

/* Starts argv's program on a new pty's slave: the master, or an error. */
static int start(char **argv) {
    int m = posix_openpt(O_RDWR);
    if (m < 0) {
        return m;
    }
    char *name = ptsname(m);
    if (name == NULL || unlockpt(m) != 0) {
        close(m);
        return -EIO;
    }
    int s = open(name, O_RDWR);
    if (s < 0) {
        close(m);
        return s;
    }
    fcntl(m, F_SETFD, FD_CLOEXEC);
    struct winsize ws;
    ws.ws_row = (ushort)rows;
    ws.ws_col = (ushort)cols;
    ws.ws_xpixel = (ushort)(cols * font.width);
    ws.ws_ypixel = (ushort)(rows * font.height);
    ioctl(s, TIOCSWINSZ, &ws);
    /* The slave as 0, 1 and 2 for the program, then ours back. */
    int saved[3];
    for (int fd = 0; fd < 3; fd++) {
        saved[fd] = dup(fd);
    }
    for (int fd = 0; fd < 3; fd++) {
        close(fd);
        dup(s);
    }
    child = spawn(argv[0], argv, environ);
    for (int fd = 0; fd < 3; fd++) {
        close(fd);
        dup(saved[fd]);
        close(saved[fd]);
    }
    close(s);
    if (child < 0) {
        close(m);
        return child;
    }
    return m;
}

/* What the master has: 0, or -1 once the program is gone. */
static int read_master(void) {
    uchar buf[1024];
    long n = read(master, buf, sizeof(buf));
    if (n <= 0) {
        return -1;
    }
    for (long i = 0; i < n; i++) {
        output_byte(buf[i]);
    }
    return 0;
}

int main(int argc, char **argv) {
    char *sh_argv[] = {"sh", NULL};
    int first = 1;
    for (; first < argc && argv[first][0] == '-'; first++) {
        if (strcmp(argv[first], "-v") == 0) {
            verbose = 1;
        } else if (strcmp(argv[first], "-T") == 0 && first + 1 < argc) {
            title = argv[++first];
        } else {
            fprintf(2, "usage: term [-v] [-T TITLE] [PROGRAM ARGS...]\n");
            return 2;
        }
    }
    char **prog = argc > first ? argv + first : sh_argv;
    char path[128];
    if (strchr(prog[0], '/') == NULL) {
        snprintf(path, sizeof(path), "/bin/%s", prog[0]);
    } else {
        snprintf(path, sizeof(path), "%s", prog[0]);
    }
    if (gui_font_load(&font, FONT) < 0) {
        fprintf(2, "term: no font at %s\n", FONT);
        return 1;
    }
    hist = malloc(sizeof(struct line) * SCROLLBACK);
    for (int r = 0; r < MAX_ROWS; r++) {
        screen[r] = malloc(sizeof(struct line));
        if (screen[r] == NULL) {
            fprintf(2, "term: out of memory\n");
            return 1;
        }
    }
    rows = ROWS;
    cols = COLS;
    vt_init(&vt);
    gui_keyboard_init(&kbd, &gui_keymap_us);
    reset();
    int err;
    display = gui_connect(NULL, &err);
    if (display == NULL) {
        fprintf(2, "term: can't connect to the display server: %s\n", strerror(-err));
        return 1;
    }
    char *spawn_argv[MAXARG];
    int n = 0;
    spawn_argv[n++] = path;
    for (int i = 1; prog[i] != NULL && n < MAXARG - 1; i++) {
        spawn_argv[n++] = prog[i];
    }
    spawn_argv[n] = NULL;
    master = start(spawn_argv);
    if (master < 0) {
        fprintf(2, "term: can't start %s: %s\n", path, strerror(-master));
        return 1;
    }
    int tid = thread_start(writer, NULL, writer_stack, sizeof(writer_stack));
    if (tid < 0) {
        fprintf(2, "term: can't start its writer: %s\n", strerror(-tid));
        kill(child);
        return 1;
    }
    win = gui_window_open(display, title, COLS * font.width + 2 * PAD, ROWS * font.height + 2 * PAD,
                          &handlers, NULL);
    if (win == NULL) {
        fprintf(2, "term: can't open a window\n");
        kill(child);
        return 1;
    }
    resize(gui_window_width(win), gui_window_height(win));
    while (!quit) {
        int w = gui_window_width(win);
        int h = gui_window_height(win);
        if (w != laid_w || h != laid_h) {
            resize(w, h);
        }
        int something = all_dirty || cur_row + view != drawn_row || cur_col != drawn_col;
        for (int r = 0; r < rows && !something; r++) {
            something = row_dirty[r];
        }
        if (frame_ready && something) {
            draw();
        }
        int timeout = -1;
        if (repeat_code != 0) {
            int left = (int)(repeat_at - now_ms());
            timeout = left > 0 ? left : 0;
        }
        struct pollfd fds[2];
        fds[0].fd = gui_display_fd(display);
        fds[0].events = POLLIN;
        fds[0].revents = 0;
        fds[1].fd = master;
        fds[1].events = POLLIN;
        fds[1].revents = 0;
        poll(fds, 2, timeout);
        if (fds[1].revents & (POLLIN | POLLHUP | POLLERR)) {
            if (read_master() < 0) {
                break;
            }
        }
        if (gui_dispatch(display, 0) < 0) {
            break;
        }
        if (repeat_code != 0 && (int)(now_ms() - repeat_at) >= 0) {
            key_press(repeat_code);
            repeat_at = now_ms() + REPEAT_RATE;
        }
    }
    if (child > 0) {
        kill(child);
        wait(child, NULL);
    }
    gui_window_destroy(win);
    gui_disconnect(display);
    return 0;
}
