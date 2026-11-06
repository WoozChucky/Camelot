/* The framebuffer console (fbcon.h): a grid of cells, an ANSI parser in
 * front of it, and the code that keeps VRAM showing the grid.
 *
 * VRAM and the grid: `shown` is what each cell of VRAM shows, a cell's
 * character and attribute as drawn, so a cell is drawn only when the grid
 * wants something else there; `ink` is how many columns of each row of
 * cells may have a pixel other than black, so a scroll copies, and a clear
 * blanks, only those. Both are true of VRAM except while a scroll waits
 * (`scroll` rows), when VRAM is still where it was: then nothing is drawn
 * until fbcon_flush() has moved the pixels up by as many rows. */
#include "fb.h"
#include "fbcon.h"
#include "spinlock.h"
#include "string.h"
#include "trap.h"
#include "uart.h"

/* The glyphs: 8 pixels wide, a byte a row, the leftmost pixel the top bit. */
#define GLYPH_W 8
#define GLYPH_H 16

/* A cell: the character in the low byte, then the attribute, the
 * foreground's colour in the low nibble and the background's in the high
 * one. */
#define CELL(ch, attr) ((u16)((u8)(ch) | (u16)(attr) << 8))
#define CELL_CH(cell) ((u8)(cell))
#define CELL_ATTR(cell) ((u8)((cell) >> 8))
#define ATTR(fg, bg) ((u8)((fg) | (bg) << 4))
#define ATTR_FG(attr) ((attr) & 15)
#define ATTR_BG(attr) ((attr) >> 4)
/* Swaps foreground and background: the reverse video of SGR 7, and the
 * cursor. */
#define ATTR_SWAP(attr) ((u8)((attr) >> 4 | ((attr) & 15) << 4))

#define FG_DEFAULT 7
#define BG_DEFAULT 0
#define BLANK CELL(' ', ATTR(FG_DEFAULT, BG_DEFAULT))

/* The 16 colours, in ANSI's order (black, red, green, yellow, blue,
 * magenta, cyan, white, then the bright ones), XRGB8888: Linux's console
 * palette (drivers/tty/vt/vt.c, default_red and the others). */
static const u32 palette[16] = {
    0x000000, 0xaa0000, 0x00aa00, 0xaa5500, 0x0000aa, 0xaa00aa, 0x00aaaa, 0xaaaaaa,
    0x555555, 0xff5555, 0x55ff55, 0xffff55, 0x5555ff, 0xff55ff, 0x55ffff, 0xffffff,
};

/* The parser's states: text, after ESC, and in a control sequence (ESC [). */
#define ST_TEXT 0
#define ST_ESC 1
#define ST_CSI 2
#define MAX_PARAMS 8
#define PARAM_MAX 9999

static bool active;     /* fbcon_init found a display to draw on */
static bool suspended;  /* a program has the display */
static bool need_clear; /* VRAM is not the grid at all: clear and draw it all */
static bool dirty;      /* the grid or the cursor changed since the last flush */
static bool unpresented; /* VRAM changed since the last FB_PRESENT */

static u32 *vram;
static u32 pitch;     /* pixels a row of VRAM */
static u32 vram_rows; /* and the mode's rows */
static const u8 *glyphs;
static u32 glyph_bytes;
static u32 nglyphs;

static u16 grid[FBCON_ROWS][FBCON_COLS];
static u16 shown[FBCON_ROWS][FBCON_COLS];
static u8 ink[FBCON_ROWS];
static u32 dirty_rows; /* rows whose cells may differ from `shown`: a bit each */
static u32 scroll;     /* rows the grid scrolled that VRAM hasn't */
static bool grid_ready;

/* Where the next character goes: col is FBCON_COLS after the last column
 * was written, a wrap that the next character makes (as a VT100 does). */
static u32 row;
static u32 col;
/* The cell where VRAM shows the cursor, if cursor_drawn. */
static u32 cursor_row;
static u32 cursor_col;
static bool cursor_drawn;

static u32 fg = FG_DEFAULT;
static u32 bg = BG_DEFAULT;
static bool bold;
static bool reverse;

static u32 state = ST_TEXT;
static u32 params[MAX_PARAMS];
static u32 nparams;
static bool private_seq; /* ESC [ ?: DEC's private modes, ignored */

/* A glyph row's nibble as 4 pixels, two to a u64, for one foreground and
 * background: drawing a row of 8 pixels is 4 stores. Made again when the
 * colours change. */
static u64 nibble_px[16][2];
static i32 nibble_attr = -1;

static void make_nibbles(u8 attr) {
    if ((i32)attr == nibble_attr) {
        return;
    }
    u64 f = palette[ATTR_FG(attr)];
    u64 b = palette[ATTR_BG(attr)];
    for (u32 n = 0; n < 16; n++) {
        u64 p0 = (n & 8) != 0 ? f : b;
        u64 p1 = (n & 4) != 0 ? f : b;
        u64 p2 = (n & 2) != 0 ? f : b;
        u64 p3 = (n & 1) != 0 ? f : b;
        /* Little-endian: the pixel on the left in the low half. */
        nibble_px[n][0] = p0 | p1 << 32;
        nibble_px[n][1] = p2 | p3 << 32;
    }
    nibble_attr = attr;
}

static void init_grid(void) {
    for (u32 r = 0; r < FBCON_ROWS; r++) {
        for (u32 c = 0; c < FBCON_COLS; c++) {
            grid[r][c] = BLANK;
        }
    }
    grid_ready = true;
}

/* The attribute of what is written now: SGR's colours, bold the bright
 * ones, reverse swapped. */
static u8 cur_attr(void) {
    u32 f = fg;
    if (bold && f < 8) {
        f = f + 8;
    }
    u8 a = ATTR(f, bg);
    return reverse ? ATTR_SWAP(a) : a;
}

/* An erased cell: blank, in the background's colour (as a VT100 erases). */
static u16 blank_cell(void) {
    u8 a = cur_attr();
    return CELL(' ', ATTR(FG_DEFAULT, ATTR_BG(a)));
}

/* --- VRAM --- */

/* The pixel loops, in assembly (fbblit.S). A group is 4 u64s, 8 pixels: a
 * cell's width. */
void fbcon_glyph(u64 *dst, const u8 *rows, const u64 *nibbles, u64 stride, u64 height);
void fbcon_fill(u64 *dst, u64 groups, u64 rows, u64 stride, u64 value);
void fbcon_copy(u64 *dst, const u64 *src, u64 groups, u64 rows, u64 stride);
u64 fbcon_diff(const u16 *a, const u16 *b, u64 n);

static bool can_draw(void) {
    return active && !suspended && !need_clear && scroll == 0;
}

/* The first pixel of cell (r, c). */
static u64 *cell_px(u32 r, u32 c) {
    return (u64 *)(vram + (usize)r * GLYPH_H * pitch + (usize)c * GLYPH_W);
}

/* Draws `cell` at (r, c) and notes it in `shown` and `ink`. */
static void draw_cell(u32 r, u32 c, u16 cell) {
    u8 ch = CELL_CH(cell);
    make_nibbles(CELL_ATTR(cell));
    const u8 *g = glyphs + (usize)(ch < nglyphs ? ch : '?') * glyph_bytes;
    fbcon_glyph(cell_px(r, c), g, &nibble_px[0][0], (u64)pitch * 4, GLYPH_H);
    shown[r][c] = cell;
    if (cell != BLANK && ink[r] < c + 1) {
        ink[r] = (u8)(c + 1);
    }
    unpresented = true;
}

/* Draws the cells of row `r` that differ from the grid's. */
static void sync_row(u32 r) {
    u32 c = 0;
    for (;;) {
        c = c + (u32)fbcon_diff(&grid[r][c], &shown[r][c], FBCON_COLS - c);
        if (c >= FBCON_COLS) {
            return;
        }
        draw_cell(r, c, grid[r][c]);
        c++;
    }
}

/* A cell of the grid changed: drawn now if it can be, else at the flush. The
 * cursor is left to the flush, so text is drawn once, not with the cursor
 * and again without it. */
static void touch(u32 r, u32 c) {
    dirty = true;
    dirty_rows |= 1u << r;
    if (can_draw() && shown[r][c] != grid[r][c]) {
        draw_cell(r, c, grid[r][c]);
    }
}

/* Blanks the first `cols` cells of row `r`. */
static void clear_row(u32 r, u32 cols) {
    fbcon_fill(cell_px(r, 0), cols, GLYPH_H, (u64)pitch * 4, 0);
    for (u32 c = 0; c < FBCON_COLS; c++) {
        shown[r][c] = BLANK;
    }
    ink[r] = 0;
}

/* Brings VRAM to the grid: the whole screen cleared if need_clear, the
 * pixels moved up by the rows the grid scrolled, then the cells that
 * differ drawn, and the cursor; and the frame presented. */
static void flush_locked(void) {
    if (!active || suspended || !dirty) {
        return;
    }
    if (need_clear) {
        /* All of the mode's rows, not only the grid's: a program may have
         * drawn anywhere. */
        fbcon_fill((u64 *)vram, pitch / GLYPH_W, vram_rows, (u64)pitch * 4, 0);
        for (u32 r = 0; r < FBCON_ROWS; r++) {
            for (u32 c = 0; c < FBCON_COLS; c++) {
                shown[r][c] = BLANK;
            }
            ink[r] = 0;
        }
        need_clear = false;
        scroll = 0;
        cursor_drawn = false;
        dirty_rows = ~0u;
        unpresented = true;
    }
    if (scroll > 0) {
        /* Each row's pixels from n rows below, as wide as the ink of
         * either; the rows that come in at the bottom blanked. */
        u32 n = scroll < FBCON_ROWS ? scroll : FBCON_ROWS;
        for (u32 r = 0; r + n < FBCON_ROWS; r++) {
            u32 w = ink[r] > ink[r + n] ? ink[r] : ink[r + n];
            fbcon_copy(cell_px(r, 0), cell_px(r + n, 0), w, GLYPH_H, (u64)pitch * 4);
            memmove(shown[r], shown[r + n], sizeof shown[r]);
            ink[r] = ink[r + n];
        }
        for (u32 r = FBCON_ROWS - n; r < FBCON_ROWS; r++) {
            clear_row(r, ink[r]);
        }
        if (cursor_drawn) {
            /* It moved up with the pixels, or off the top. */
            if (cursor_row >= n) {
                cursor_row = cursor_row - n;
            } else {
                cursor_drawn = false;
            }
        }
        scroll = 0;
        unpresented = true;
    }
    for (u32 r = 0; r < FBCON_ROWS; r++) {
        if ((dirty_rows >> r & 1) != 0) {
            sync_row(r);
        }
    }
    dirty_rows = 0;
    /* The cursor: the old one's cell back to the grid's, the new one's
     * drawn inverse. */
    u32 cc = col < FBCON_COLS ? col : FBCON_COLS - 1;
    if (cursor_drawn && (cursor_row != row || cursor_col != cc) &&
        shown[cursor_row][cursor_col] != grid[cursor_row][cursor_col]) {
        draw_cell(cursor_row, cursor_col, grid[cursor_row][cursor_col]);
    }
    u16 cell = grid[row][cc];
    cell = CELL(CELL_CH(cell), ATTR_SWAP(CELL_ATTR(cell)));
    if (shown[row][cc] != cell) {
        draw_cell(row, cc, cell);
    }
    cursor_row = row;
    cursor_col = cc;
    cursor_drawn = true;
    dirty = false;
    if (unpresented) {
        fb_present_frame();
        unpresented = false;
    }
}

/* --- The grid --- */

/* Scrolls the grid up a row: VRAM follows at the flush. */
static void scroll_up(void) {
    memmove(grid[0], grid[1], sizeof grid[0] * (FBCON_ROWS - 1));
    u16 b = blank_cell();
    for (u32 c = 0; c < FBCON_COLS; c++) {
        grid[FBCON_ROWS - 1][c] = b;
    }
    if (scroll < FBCON_ROWS) {
        scroll++;
    }
    /* The rows changed since the flush move up with the grid; the new last
     * row is one. */
    dirty = true;
    dirty_rows = dirty_rows >> 1 | 1u << (FBCON_ROWS - 1);
}

static void newline(void) {
    col = 0;
    if (row + 1 < FBCON_ROWS) {
        row++;
    } else {
        scroll_up();
    }
    dirty = true;
}

static void put_char(u8 ch) {
    if (col >= FBCON_COLS) {
        newline();
    }
    if (ch < 0x20 || ch > 0x7e) {
        ch = '?';
    }
    u8 a = cur_attr();
    /* A blank's foreground doesn't show: one value for all of them. */
    u16 cell = ch == ' ' ? CELL(' ', ATTR(FG_DEFAULT, ATTR_BG(a))) : CELL(ch, a);
    grid[row][col] = cell;
    touch(row, col);
    col++;
    dirty = true;
}

static void erase(u32 r, u32 from, u32 to) {
    u16 b = blank_cell();
    for (u32 c = from; c < to; c++) {
        if (grid[r][c] != b) {
            grid[r][c] = b;
            touch(r, c);
        }
    }
}

static u32 param(u32 i, u32 dflt) {
    return i < nparams && params[i] != 0 ? params[i] : dflt;
}

/* SGR: the colours and attributes of what is written from now on. */
static void sgr(void) {
    if (nparams == 0) {
        nparams = 1;
        params[0] = 0;
    }
    for (u32 i = 0; i < nparams; i++) {
        u32 p = params[i];
        if (p == 0) {
            fg = FG_DEFAULT;
            bg = BG_DEFAULT;
            bold = false;
            reverse = false;
        } else if (p == 1) {
            bold = true;
        } else if (p == 22) {
            bold = false;
        } else if (p == 7) {
            reverse = true;
        } else if (p == 27) {
            reverse = false;
        } else if (p >= 30 && p <= 37) {
            fg = p - 30;
        } else if (p == 39) {
            fg = FG_DEFAULT;
        } else if (p >= 40 && p <= 47) {
            bg = p - 40;
        } else if (p == 49) {
            bg = BG_DEFAULT;
        } else if (p >= 90 && p <= 97) {
            fg = p - 90 + 8;
        } else if (p >= 100 && p <= 107) {
            bg = p - 100 + 8;
        }
    }
}

/* A control sequence's final byte. */
static void csi(u8 final) {
    if (private_seq) {
        return;
    }
    u32 n;
    switch (final) {
    case 'm':
        sgr();
        break;
    case 'H':
    case 'f':
        row = param(0, 1) - 1;
        col = param(1, 1) - 1;
        if (row >= FBCON_ROWS) {
            row = FBCON_ROWS - 1;
        }
        if (col >= FBCON_COLS) {
            col = FBCON_COLS - 1;
        }
        break;
    case 'A':
        n = param(0, 1);
        row = n < row ? row - n : 0;
        break;
    case 'B':
        n = param(0, 1);
        row = row + n < FBCON_ROWS ? row + n : FBCON_ROWS - 1;
        break;
    case 'C':
        n = param(0, 1);
        col = col + n < FBCON_COLS ? col + n : FBCON_COLS - 1;
        break;
    case 'D':
        n = param(0, 1);
        if (col >= FBCON_COLS) {
            col = FBCON_COLS - 1;
        }
        col = n < col ? col - n : 0;
        break;
    case 'J': {
        u32 c = col < FBCON_COLS ? col : FBCON_COLS - 1;
        n = nparams > 0 ? params[0] : 0;
        if (n == 0) {
            erase(row, c, FBCON_COLS);
            for (u32 r = row + 1; r < FBCON_ROWS; r++) {
                erase(r, 0, FBCON_COLS);
            }
        } else if (n == 1) {
            for (u32 r = 0; r < row; r++) {
                erase(r, 0, FBCON_COLS);
            }
            erase(row, 0, c + 1);
        } else if (n == 2) {
            for (u32 r = 0; r < FBCON_ROWS; r++) {
                erase(r, 0, FBCON_COLS);
            }
        }
        break;
    }
    case 'K': {
        u32 c = col < FBCON_COLS ? col : FBCON_COLS - 1;
        n = nparams > 0 ? params[0] : 0;
        if (n == 0) {
            erase(row, c, FBCON_COLS);
        } else if (n == 1) {
            erase(row, 0, c + 1);
        } else if (n == 2) {
            erase(row, 0, FBCON_COLS);
        }
        break;
    }
    default:
        break;
    }
    dirty = true;
}

void fbcon_putc(char c) {
    u8 ch = (u8)c;
    if (!grid_ready) {
        init_grid();
    }
    if (state == ST_ESC) {
        if (ch == '[') {
            state = ST_CSI;
            nparams = 0;
            private_seq = false;
            for (u32 i = 0; i < MAX_PARAMS; i++) {
                params[i] = 0;
            }
        } else {
            state = ST_TEXT; /* ESC and one other byte: none we act on */
        }
        return;
    }
    if (state == ST_CSI) {
        if (ch >= '0' && ch <= '9') {
            if (nparams == 0) {
                nparams = 1;
            }
            u32 *p = &params[nparams - 1];
            *p = *p * 10 + (ch - '0');
            if (*p > PARAM_MAX) {
                *p = PARAM_MAX;
            }
        } else if (ch == ';') {
            if (nparams == 0) {
                nparams = 1;
            }
            if (nparams < MAX_PARAMS) {
                nparams++;
            }
        } else if (ch == '?' || ch == '>' || ch == '=' || ch == '<') {
            private_seq = true;
        } else if (ch >= 0x40 && ch <= 0x7e) {
            state = ST_TEXT;
            csi(ch);
        } else if (ch < 0x20 || ch > 0x7e) {
            state = ST_TEXT; /* not a sequence after all */
        }
        return;
    }
    switch (ch) {
    case '\n':
        newline();
        break;
    case '\r':
        col = 0;
        dirty = true;
        break;
    case '\b':
        if (col >= FBCON_COLS) {
            col = FBCON_COLS - 1;
        }
        if (col > 0) {
            col--;
        }
        dirty = true;
        break;
    case '\t':
        if (col < FBCON_COLS) {
            col = (col / 8 + 1) * 8;
            if (col > FBCON_COLS - 1) {
                col = FBCON_COLS - 1;
            }
        }
        dirty = true;
        break;
    case 0x1b:
        state = ST_ESC;
        break;
    default:
        /* Any other control character (C0, DEL) does nothing, as on a VT;
         * a byte past ASCII is drawn as '?'. */
        if (ch >= 0x20 && ch != 0x7f) {
            put_char(ch);
        }
        break;
    }
}

void fbcon_puts(const char *s) {
    while (*s != '\0') {
        fbcon_putc(*s);
        s++;
    }
}

static u32 le32(const u8 *p) {
    return (u32)p[0] | (u32)p[1] << 8 | (u32)p[2] << 16 | (u32)p[3] << 24;
}

void fbcon_init(void) {
    u32 width;
    u32 height;
    fb_mode_size(&width, &height);
    if (fb_vram() == NULL || width < FBCON_COLS * GLYPH_W || height < FBCON_ROWS * GLYPH_H) {
        return;
    }
    /* The font: PSF2, 8x16. */
    if (font_psf2_size < 32 || le32(font_psf2) != 0x864ab572) {
        return;
    }
    u32 header = le32(font_psf2 + 8);
    u32 n = le32(font_psf2 + 16);
    u32 bytes = le32(font_psf2 + 20);
    if (le32(font_psf2 + 24) != GLYPH_H || le32(font_psf2 + 28) != GLYPH_W || bytes != GLYPH_H || n < 128 ||
        header + (usize)n * bytes > font_psf2_size) {
        return;
    }
    acquire(&uart_tx_lock);
    if (!grid_ready) {
        init_grid();
    }
    glyphs = font_psf2 + header;
    nglyphs = n < 256 ? n : 256;
    glyph_bytes = bytes;
    vram = fb_vram();
    pitch = width;
    vram_rows = height;
    active = true;
    need_clear = true;
    dirty = true;
    flush_locked();
    release(&uart_tx_lock);
}

void fbcon_flush(void) {
    if (!active || !dirty) {
        return;
    }
    acquire(&uart_tx_lock);
    flush_locked();
    release(&uart_tx_lock);
}

void fbcon_panic_flush(void) {
    flush_locked();
}

void fbcon_suspend(void) {
    acquire(&uart_tx_lock);
    suspended = true;
    release(&uart_tx_lock);
}

void fbcon_resume(void) {
    acquire(&uart_tx_lock);
    suspended = false;
    need_clear = true;
    dirty = true;
    release(&uart_tx_lock);
}
