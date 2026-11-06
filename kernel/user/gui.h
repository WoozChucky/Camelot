/* The GUI library (kernel/user/gui/, libgui.a; spec section 10, "The GUI
 * library"; gui/README.md): software drawing into XRGB8888 buffers, damage,
 * PSF2 text, QOI images, a keyboard layout, widgets, and a Wayland client.
 * Integer arithmetic only: the blending is fixed point. A program that
 * draws includes this header (which includes user.h) and links with
 * -lgui -lc. */
#ifndef CAMELOT_GUI_H
#define CAMELOT_GUI_H

#include "user.h"

/* Linux's error number for a protocol error, which the kernel never
 * returns: the Wayland client's, for a message that can't be read. */
#define EPROTO 71

/* ---- Rectangles (gui/rect.c) ----
 *
 * A rectangle is x, y, a width and a height, the pixels x <= px < x + w,
 * y <= py < y + h; one with no width or height is empty. An intersection
 * that is empty is {0, 0, 0, 0}; a union is the smallest rectangle around
 * both, an empty one left out. */
struct gui_rect {
    int x;
    int y;
    int w;
    int h;
};

struct gui_rect gui_rect(int x, int y, int w, int h);
int gui_rect_empty(struct gui_rect r);
struct gui_rect gui_rect_intersect(struct gui_rect a, struct gui_rect b);
struct gui_rect gui_rect_union(struct gui_rect a, struct gui_rect b);
int gui_rect_contains(struct gui_rect r, int x, int y);
/* 1 if b is all inside a (an empty b is inside anything). */
int gui_rect_covers(struct gui_rect a, struct gui_rect b);

/* ---- Colours ----
 *
 * A pixel is a 32-bit 0xAARRGGBB, Wayland's and DRM's XRGB8888 and
 * ARGB8888 as a little-endian word: blue in the first byte. In an XRGB
 * buffer the top byte means nothing; an image's (gui_image_load) is its
 * alpha, 0 transparent to 255 opaque, not premultiplied. */
#define GUI_RGB(r, g, b) (0xff000000u | ((uint)(r) << 16) | ((uint)(g) << 8) | (uint)(b))
#define GUI_ARGB(a, r, g, b) (((uint)(a) << 24) | ((uint)(r) << 16) | ((uint)(g) << 8) | (uint)(b))
#define GUI_ALPHA(c) ((uint)(c) >> 24)

/* src over dst: each channel (src * a + dst * (255 - a)) / 255, rounded,
 * with a src's alpha; the result is opaque. */
uint gui_mix(uint dst, uint src);

/* ---- Buffers and drawing (gui/draw.c) ----
 *
 * A buffer is pixels in rows: `stride` pixels from the start of a row to
 * the next (at least `width`), as a wl_shm buffer's stride is bytes / 4.
 * Every drawing call draws only inside the buffer's clip rectangle, which
 * gui_buffer_init sets to the whole buffer. The colour of the opaque calls
 * is stored as it is, alpha byte and all; the _blend calls mix it in
 * (gui_mix). */
struct gui_buffer {
    uint *pixels;
    int width;
    int height;
    int stride;
    struct gui_rect clip;
};

void gui_buffer_init(struct gui_buffer *b, uint *pixels, int width, int height, int stride);
/* A buffer of its own, from malloc, all zeros: 0, -EINVAL (a size not
 * above 0, or too big) or -ENOMEM. gui_buffer_free frees those pixels. */
int gui_buffer_alloc(struct gui_buffer *b, int width, int height);
void gui_buffer_free(struct gui_buffer *b);
/* Clips to r (inside the buffer), or back to the whole buffer. */
void gui_clip(struct gui_buffer *b, struct gui_rect r);
void gui_unclip(struct gui_buffer *b);
/* The pixel at x, y; 0 outside the buffer. */
uint gui_pixel(const struct gui_buffer *b, int x, int y);
void gui_plot(struct gui_buffer *b, int x, int y, uint color);

void gui_fill(struct gui_buffer *b, struct gui_rect r, uint color);
void gui_fill_blend(struct gui_buffer *b, struct gui_rect r, uint color);
/* r's outermost pixels. */
void gui_outline(struct gui_buffer *b, struct gui_rect r, uint color);
/* From x0, y0 to x1, y1, both ends drawn (Bresenham's); only the columns
 * (or rows) inside the clip are computed, however far off the ends are. */
void gui_line(struct gui_buffer *b, int x0, int y0, int x1, int y1, uint color);
/* r with its corners rounded to a quarter circle of `radius` pixels (at
 * most half of r's smaller side): filled, or its edge only. */
void gui_fill_round(struct gui_buffer *b, struct gui_rect r, int radius, uint color);
void gui_outline_round(struct gui_buffer *b, struct gui_rect r, int radius, uint color);
/* src's rectangle `from` to x, y in b: copied as it is (b may be src,
 * the two rectangles overlapping: a scroll), or mixed in by each pixel's
 * alpha (src another buffer). */
void gui_blit(struct gui_buffer *b, int x, int y, const struct gui_buffer *src, struct gui_rect from);
void gui_blend(struct gui_buffer *b, int x, int y, const struct gui_buffer *src, struct gui_rect from);

/* ---- Damage (gui/damage.c) ----
 *
 * What changed since the last frame, as up to GUI_DAMAGE_RECTS rectangles,
 * no two overlapping or touching: one that overlaps or touches another is
 * merged with it into their union, and when there are too many, the new
 * one is merged with the one their union grows least (the fewest pixels
 * added to it). A compositor or a window recomposites or
 * sends only those. */
#define GUI_DAMAGE_RECTS 8

struct gui_damage {
    int count;
    struct gui_rect rects[GUI_DAMAGE_RECTS];
};

void gui_damage_clear(struct gui_damage *d);
void gui_damage_add(struct gui_damage *d, struct gui_rect r);
/* The union of every rectangle. */
struct gui_rect gui_damage_bounds(const struct gui_damage *d);

/* ---- Text (gui/font.c) ----
 *
 * A PSF2 font, as Linux's console fonts are (the desktop image's
 * /usr/share/fonts/spleen/8x16.psfu), its Unicode table read when it has
 * one (else glyph n is code point n). Every glyph has the font's width:
 * text is measured in cells. GUI_BOLD draws a glyph twice, one pixel
 * apart; GUI_LARGE twice as wide and as high. A code point the font has
 * no glyph for is drawn as U+FFFD, or '?'. */
#define GUI_BOLD 1
#define GUI_LARGE 2

/* A code point's glyph (gui_font_parse sorts them). */
struct gui_glyph_map {
    uint code;
    uint glyph;
};

struct gui_font {
    int width;
    int height;
    int glyphs;
    int bytes_per_glyph;
    const uchar *bitmaps;
    short low[256];            /* the glyphs of U+0000 to U+00FF, -1: none */
    struct gui_glyph_map *map; /* the rest of the Unicode table, sorted */
    int nmap;
    int fallback;              /* U+FFFD's glyph, or '?''s */
    void *file;                /* what gui_font_load read */
};

/* Reads the font in `data` (which must stay while it is used): 0, -EINVAL
 * (not a PSF2 font, or a broken one) or -ENOMEM. */
int gui_font_parse(struct gui_font *f, const void *data, size_t len);
/* Reads the file and parses it: 0, a negative error number. */
int gui_font_load(struct gui_font *f, const char *path);
void gui_font_free(struct gui_font *f);
/* The glyph of code point `code` (the fallback when there is none). */
int gui_font_glyph(const struct gui_font *f, uint code);
/* The next code point of UTF-8 text at *s, *s moved past it; 0 at the NUL.
 * A byte that does not start a well-formed sequence (an overlong one, a
 * surrogate, past U+10FFFF) is U+FFFD, and only that byte is passed. */
uint gui_utf8_next(const char **s);
/* Writes `code` as UTF-8 into out (4 bytes at most), returns how many. */
int gui_utf8_put(char *out, uint code);
/* Draws UTF-8 text with its cells' top left at x, y; returns the x past
 * it. Only the glyphs' set pixels are drawn. */
int gui_text(struct gui_buffer *b, const struct gui_font *f, int x, int y, const char *s, uint color, int style);
int gui_text_width(const struct gui_font *f, const char *s, int style);
int gui_text_height(const struct gui_font *f, int style);

/* ---- Images (gui/image.c) ----
 *
 * QOI ("the Quite OK Image format", qoiformat.org), as `cargo xtask fs`
 * converts the committed sources of kernel/images into the desktop image's
 * /usr/share/images. The image is an ARGB buffer of its own, its pixels
 * as the chunks give them whatever the header's channels say, as the
 * reference decoder does (an RGB encoder's are opaque). 0, -EINVAL (not
 * QOI, cut short, too big) or -ENOMEM; free it with gui_buffer_free. */
int gui_qoi_decode(struct gui_buffer *img, const void *data, size_t len);
int gui_image_load(struct gui_buffer *img, const char *path);
/* Reads a whole file (16 MiB at most) into memory from malloc: 0 with
 * *data and *len set, or a negative error number (-EFBIG: an empty or
 * bigger file). */
int gui_read_file(const char *path, uchar **data, size_t *len);

/* ---- The keyboard (gui/keymap.c) ----
 *
 * Linux's key codes (KEY_A...) to characters through a layout's tables, as
 * a Wayland client does with no keymap (wl_keyboard's NO_KEYMAP format):
 * US first. The modifier bits are X's, as wl_keyboard.modifiers sends
 * them (Shift, Lock, Control, Mod1 for Alt, Mod4 for the logo key). */
#define GUI_MOD_SHIFT 0x01
#define GUI_MOD_CAPS 0x02
#define GUI_MOD_CTRL 0x04
#define GUI_MOD_ALT 0x08
#define GUI_MOD_LOGO 0x40

#define GUI_KEYMAP_KEYS 128

/* A layout: each key's character, and with Shift; 0, none. Caps Lock
 * swaps the two for a key whose shifted character is the plain one's
 * capital letter. */
struct gui_keymap {
    const char *name;
    ushort plain[GUI_KEYMAP_KEYS];
    ushort shift[GUI_KEYMAP_KEYS];
};

extern const struct gui_keymap gui_keymap_us;

/* What the modifier keys held say, and which are held (GUI_MOD_ bits:
 * `left` and `right` keep the two Shift, Ctrl and Alt keys apart). */
struct gui_keyboard {
    const struct gui_keymap *keymap;
    uint mods;
    uint left;
    uint right;
};

void gui_keyboard_init(struct gui_keyboard *k, const struct gui_keymap *keymap);
/* A key went down (pressed 1, or 2 repeating) or up: the modifiers follow
 * it; returns the character it types, or 0 (a release, a modifier, a key
 * with none; with Ctrl or Alt held too, as those make shortcuts). */
uint gui_keyboard_key(struct gui_keyboard *k, uint code, int pressed);
/* The modifiers the display server says are held (wl_keyboard.modifiers'
 * depressed | latched) and locked, which it sends when the keyboard enters
 * a window and as they change: a modifier not held is let go of, one held
 * that no key here pressed is counted as a left key's; Caps Lock is as
 * `locked` says. When the keyboard leaves, gui_keyboard_modifiers(k, 0,
 * k->mods) lets go of every key, whose release goes to another window. */
void gui_keyboard_modifiers(struct gui_keyboard *k, uint held, uint locked);
/* The character of `code` with modifiers `mods`, Ctrl and Alt aside. */
uint gui_keymap_char(const struct gui_keymap *m, uint code, uint mods);

/* ---- Widgets (gui/widget.c, gui/layout.c) ----
 *
 * A tree of widgets in a struct gui_ui: rows and columns lay their
 * children out side by side or one under the other; labels, buttons, text
 * fields, lists with their scroll bars, and menus in a row (a menu bar).
 * gui_ui_layout places them in a rectangle; gui_ui_draw redraws the ones
 * that changed, adding their rectangles to the ui's damage; the
 * gui_ui_pointer, gui_ui_button, gui_ui_scroll and gui_ui_key calls are the
 * input. The widget under the pointer is found by hit-testing the tree; a
 * click on a widget that takes the keyboard (a button, a field, a list)
 * gives it the focus, and Tab and Shift+Tab move it. */
enum gui_kind {
    GUI_ROW,
    GUI_COLUMN,
    GUI_LABEL,
    GUI_BUTTON,
    GUI_FIELD,
    GUI_LIST,
    GUI_SCROLLBAR,
    GUI_MENU
};

/* A widget's flags. */
#define GUI_FOCUSABLE 0x01 /* takes the keyboard */
#define GUI_HOVER 0x02     /* the pointer is over it */
#define GUI_PRESSED 0x04   /* a button held down on it */
#define GUI_DIRTY 0x08     /* to be drawn again */
#define GUI_OPEN 0x10      /* a menu showing its items */
#define GUI_GROW 0x20      /* takes a share of a row's or column's spare room */

struct gui_ui;
struct gui_widget;

/* What a widget does: a button clicked (value 0), Enter in a field (0), a
 * list's selection (the item), a scroll bar moved (its value), a menu's
 * item chosen (the item). */
typedef void (*gui_action)(struct gui_widget *w, int value);

struct gui_widget {
    int kind;
    int flags;
    struct gui_rect rect; /* where gui_ui_layout put it */
    int min_w;            /* the room it asks for */
    int min_h;
    struct gui_ui *ui;
    struct gui_widget *parent;
    struct gui_widget *first; /* children, in order */
    struct gui_widget *last;
    struct gui_widget *next;
    const char *text;     /* a label's, a button's, a menu's title */
    gui_action action;
    void *data;           /* the program's */
    /* rows and columns */
    int spacing;
    int padding;
    /* a field: UTF-8 text, `len` bytes of `cap`, the cursor a byte offset,
     * `scroll` the first code point shown */
    char *chars;
    int len;
    int cap;
    int cursor;
    int scroll;
    int columns;
    /* a list's or a menu's items; a list shows `rows` from `top` */
    const char *const *items;
    int count;
    int selected; /* -1: none */
    int top;
    int rows;
    /* a scroll bar: value in 0 .. range - page */
    int value;
    int range;
    int page;
    int grab_offset; /* dragging its thumb: where in the thumb it was taken */
    struct gui_widget *buddy; /* a list's scroll bar, the scroll bar's list */
};

struct gui_ui {
    struct gui_widget *root;
    struct gui_widget *focus;
    struct gui_widget *hover;
    struct gui_widget *grab; /* pressed on: gets the motion and the release */
    struct gui_widget *menu; /* the open menu */
    struct gui_rect popup;   /* where its items are */
    int popup_dirty;         /* its items to be drawn again */
    const struct gui_font *font;
    struct gui_keyboard keyboard;
    struct gui_damage damage;     /* what gui_ui_draw drew, for the window */
    struct gui_damage background; /* to be cleared to the window's colour */
    int x; /* the pointer */
    int y;
};

void gui_ui_init(struct gui_ui *ui, const struct gui_font *font);
/* Frees every widget. */
void gui_ui_free(struct gui_ui *ui);

/* Each adds a widget at the end of `parent`'s children (the root when
 * parent is NULL, which must be a row or a column: the first one made);
 * NULL when out of memory, with nothing added. A text or a title that is
 * NULL is empty. A field holds `cap` bytes of text and asks for
 * room for `columns` characters; a list shows `rows` items with a scroll bar
 * beside it (in a row of their own, which gui_list makes); a menu belongs
 * in a row along the top, its items shown under it when clicked. */
struct gui_widget *gui_row(struct gui_ui *ui, struct gui_widget *parent, int spacing, int padding);
struct gui_widget *gui_column(struct gui_ui *ui, struct gui_widget *parent, int spacing, int padding);
struct gui_widget *gui_label(struct gui_ui *ui, struct gui_widget *parent, const char *text);
struct gui_widget *gui_button(struct gui_ui *ui, struct gui_widget *parent, const char *text, gui_action action);
struct gui_widget *gui_field(struct gui_ui *ui, struct gui_widget *parent, int columns, int cap, gui_action action);
struct gui_widget *gui_list(struct gui_ui *ui, struct gui_widget *parent, const char *const *items, int count,
                            int rows, gui_action action);
struct gui_widget *gui_scrollbar(struct gui_ui *ui, struct gui_widget *parent, int range, int page,
                                 gui_action action);
struct gui_widget *gui_menu(struct gui_ui *ui, struct gui_widget *parent, const char *title,
                            const char *const *items, int count, gui_action action);

/* A label's or a button's new text (kept, not copied; NULL is empty); a field's text
 * (copied, cut to fit); a list's items; a scroll bar's position. Each
 * marks the widget to be drawn again. */
void gui_set_text(struct gui_widget *w, const char *text);
void gui_field_set(struct gui_widget *w, const char *text);
void gui_list_set(struct gui_widget *w, const char *const *items, int count);
void gui_scrollbar_set(struct gui_widget *w, int value, int range, int page);
void gui_grow(struct gui_widget *w);

/* Places the tree in r and marks it all to be drawn. */
void gui_ui_layout(struct gui_ui *ui, struct gui_rect r);
/* Draws what changed into b, adding it to ui->damage. */
void gui_ui_draw(struct gui_ui *ui, struct gui_buffer *b);
/* Marks every widget r touches to be drawn again. */
void gui_ui_invalidate(struct gui_ui *ui, struct gui_rect r);
void gui_widget_dirty(struct gui_widget *w);
/* The deepest widget at x, y (an open menu's items first), or NULL. */
struct gui_widget *gui_ui_hit(struct gui_ui *ui, int x, int y);
void gui_ui_focus(struct gui_ui *ui, struct gui_widget *w);

/* The input: the pointer at x, y; a button (BTN_LEFT...) down or up; the
 * wheel turned `amount` notches (down positive); a key (Linux's code) down
 * (1), repeated (2) or up (0). */
void gui_ui_pointer(struct gui_ui *ui, int x, int y);
void gui_ui_leave(struct gui_ui *ui);
void gui_ui_button(struct gui_ui *ui, uint button, int pressed);
void gui_ui_scroll(struct gui_ui *ui, int amount);
void gui_ui_key(struct gui_ui *ui, uint code, int pressed);

/* ---- The Wayland wire format (gui/wire.c) ----
 *
 * A message is the object's id (32 bits), then a word of its size in
 * bytes (the top 16 bits, the header's 8 included) and its opcode (the
 * low 16), then the arguments, each a multiple of 4 bytes: an int, a uint,
 * an object id or a new id are a word; a fixed is a word of 24.8 fixed
 * point; a string is its length with the NUL, then the bytes, the NUL and
 * the padding; an array its length, then the bytes and the padding. A
 * descriptor (fd) travels beside the bytes, in SCM_RIGHTS, in order. All
 * in the machine's byte order: little-endian. A struct gui_wire is one end
 * of a connection: the client's (gui/client.c) and the display server's
 * (#184). A message is at most GUI_WIRE_MAX bytes, as libwayland's are. */
#define GUI_WIRE_MAX 4096
#define GUI_WIRE_FDS 28

struct gui_wire {
    int fd;
    int in_len;   /* bytes in `in` */
    int in_pos;   /* the next message's */
    int nin_fds;  /* descriptors received, not taken */
    int out_len;
    int nout_fds;
    int start;    /* the message being built, in `out` */
    int error;    /* the first error, sticky: -EPROTO, -EMSGSIZE, -ENOBUFS, a socket's */
    int nonblock; /* gui_wire_nonblock: the display server's end */
    int in_fds[GUI_WIRE_FDS * 2];
    int out_fds[GUI_WIRE_FDS];
    uchar in[GUI_WIRE_MAX * 2];
    uchar out[GUI_WIRE_MAX * 2];
};

/* A message received, read an argument at a time: a read past its end, a
 * string without its NUL, or an fd that did not come sets `bad`. */
struct gui_message {
    uint object;
    uint opcode;
    uint size;
    const uchar *args;
    uint pos;
    int bad;
    struct gui_wire *wire;
};

void gui_wire_init(struct gui_wire *w, int fd);
/* Makes the wire's sending never wait, as the display server's must: a
 * flush sends what the socket takes and keeps the rest (-EAGAIN, which is
 * no error), and a message that finds no room, the peer having read none
 * of the last GUI_WIRE_MAX bytes and more, is -ENOBUFS. */
void gui_wire_nonblock(struct gui_wire *w);
/* Closes the descriptors received and not taken; not w->fd. */
void gui_wire_release(struct gui_wire *w);
/* Starts a message (flushing what is queued if it might not fit), adds its
 * arguments, ends it: 0, or w->error. */
void gui_wire_begin(struct gui_wire *w, uint object, uint opcode);
void gui_wire_uint(struct gui_wire *w, uint v);
void gui_wire_int(struct gui_wire *w, int v);
void gui_wire_fixed(struct gui_wire *w, int v); /* v is 24.8 already */
void gui_wire_string(struct gui_wire *w, const char *s); /* NULL: the null string, length 0 */
void gui_wire_array(struct gui_wire *w, const void *data, uint len);
void gui_wire_fd(struct gui_wire *w, int fd); /* sent (a copy), not closed */
int gui_wire_end(struct gui_wire *w);
/* Sends everything queued: 0, or a negative error number (-EAGAIN: a
 * non-blocking wire's socket took only part of it, the rest kept). */
int gui_wire_flush(struct gui_wire *w);
/* Receives what has come (waiting for it unless `nowait`): the bytes
 * received, 0 when the peer hung up, or a negative error number (-EAGAIN:
 * nothing yet; -EPROTO: descriptors were dropped, more than the messages
 * could take or more than came through; the wire's error once it has
 * one). */
int gui_wire_read(struct gui_wire *w, int nowait);
/* The next whole message received: 1 with *m set, 0 if there is none
 * yet, -EPROTO for a size that can't be (under 8, not a multiple of 4,
 * over GUI_WIRE_MAX), or the wire's error once it has one. */
int gui_wire_next(struct gui_wire *w, struct gui_message *m);
uint gui_msg_uint(struct gui_message *m);
int gui_msg_int(struct gui_message *m);
int gui_msg_fixed(struct gui_message *m);
/* NULL for the null string; NULL and `bad` for a broken one. */
const char *gui_msg_string(struct gui_message *m);
const void *gui_msg_array(struct gui_message *m, uint *len);
/* The next descriptor received, now the caller's; -1 and `bad` if none. */
int gui_msg_fd(struct gui_message *m);

/* ---- The Wayland client (gui/client.c) ----
 *
 * A connection to the display server (#184, docs/excalibur-spec.md section
 * 10) at $XDG_RUNTIME_DIR/$WAYLAND_DISPLAY (/run and wayland-0 when they
 * are unset; a name with a '/' is a path), with the globals it needs bound: wl_compositor, wl_shm,
 * xdg_wm_base and wl_seat. A window is a wl_surface with an xdg_toplevel,
 * drawn into two XRGB8888 buffers of one wl_shm_pool over a memfd: the one
 * being drawn into is made the last frame first (the rectangles the last
 * commit damaged copied over), then committed with its damage, and the
 * server releases the other. Events come to the window's handlers, any
 * of which may be NULL. */
struct gui_display;
struct gui_window;

struct gui_window_handlers {
    void (*configure)(struct gui_window *w, int width, int height); /* the new size */
    void (*close)(struct gui_window *w);
    void (*pointer)(struct gui_window *w, int x, int y); /* entered or moved */
    void (*leave)(struct gui_window *w);
    void (*button)(struct gui_window *w, uint button, int pressed);
    void (*scroll)(struct gui_window *w, int amount);
    void (*key)(struct gui_window *w, uint code, int pressed);
    /* wl_keyboard.modifiers: GUI_MOD_ bits held (depressed | latched) and
     * locked (gui_keyboard_modifiers). */
    void (*modifiers)(struct gui_window *w, uint held, uint locked);
    void (*focus)(struct gui_window *w, int focused);
    void (*frame)(struct gui_window *w); /* a good time to draw the next frame */
};

/* NULL and *err (a negative error number) if it can't connect, or the
 * server lacks a global. */
struct gui_display *gui_connect(const char *name, int *err);
void gui_disconnect(struct gui_display *d);
int gui_display_fd(struct gui_display *d);
/* Waits up to `timeout` ms (-1: for ever) for events, and handles them:
 * how many, 0 if none came, or a negative error number (-EPIPE: the server
 * hung up; -EPROTO: it sent what can't be read, or a wl_display.error,
 * printed). */
int gui_dispatch(struct gui_display *d, int timeout);
/* Sends what is queued, then handles events until the server has answered
 * everything sent before (wl_display.sync): 0, or as gui_dispatch. */
int gui_roundtrip(struct gui_display *d);

/* A window, `width` by `height` until the server configures another size,
 * shown once its first frame is committed. */
struct gui_window *gui_window_open(struct gui_display *d, const char *title, int width, int height,
                                   const struct gui_window_handlers *h, void *data);
void gui_window_destroy(struct gui_window *w);
void *gui_window_data(struct gui_window *w);
int gui_window_width(struct gui_window *w);
int gui_window_height(struct gui_window *w);
/* The buffer to draw the next frame into, as the last one left it; NULL
 * on an error. */
struct gui_buffer *gui_window_buffer(struct gui_window *w);
void gui_window_damage(struct gui_window *w, struct gui_rect r);
/* Sends the frame drawn: its damage, a frame callback, the commit. */
int gui_window_commit(struct gui_window *w);
/* Asks the server to move the window with the pointer (the last button
 * press's serial), as a client-drawn title bar does. */
void gui_window_move(struct gui_window *w);

/* Runs `ui` in a window titled `title` until it is closed or `quit` is
 * set: lays it out to the window's size, draws what changed each frame,
 * and passes the input on. 0, or a negative error number. */
int gui_run(struct gui_display *d, struct gui_ui *ui, const char *title, int width, int height,
            const int *quit);

#endif
