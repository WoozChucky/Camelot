/* The window manager and the seat: the keyboard and the pointer, grabbed
 * (EVIOCGRAB) so only the server reads them, their events turned into
 * wl_pointer's and wl_keyboard's for the client they belong to.
 *
 * - A click on a window gives it the keyboard and raises it to the top.
 *   On its title bar the window follows the pointer until the button is
 *   let go; on its close button the client is asked to close it
 *   (xdg_toplevel.close), and killed if it hasn't within CLOSE_TICKS; on
 *   its border, or just outside it (RESIZE_GRIP), left, right or below,
 *   it is resized with the pointer; on its content the click is the
 *   client's. A click on a layer surface is its client's, and gives it
 *   the keyboard if it takes it.
 * - Windows are placed and moved in the work area, the screen less the
 *   layer surfaces' exclusive zones (the panel's bar).
 * - The pointer's events go to the surface under it, or to the one a
 *   button was pressed on while any is held (the implicit grab), in its
 *   own coordinates; enter and leave as it changes.
 * - The keyboard's go to the window that has it: enter (with the keys
 *   held), leave, key, modifiers. Alt+Tab raises the lowest window and
 *   gives it the keyboard, so each press goes round them all; the logo key
 *   (Super) starts the launcher. Neither reaches a client. */
#include "compositor.h"

#define BIT(set, n) ((set)[(n) / 32] & (1u << ((n) % 32)))
#define SET(set, n) ((set)[(n) / 32] |= 1u << ((n) % 32))
#define CLEAR(set, n) ((set)[(n) / 32] &= ~(1u << ((n) % 32)))

static uint event_ms(const struct input_event *e) {
    return (uint)(e->time.tv_sec * 1000 + e->time.tv_usec / 1000);
}

void input_init(void) {
    gui_keyboard_init(&srv.kbd, &gui_keymap_us);
    srv.kbd_fd = open("/dev/input/event0", O_RDONLY);
    srv.ptr_fd = open("/dev/input/event1", O_RDONLY);
    if (srv.kbd_fd >= 0 && ioctl(srv.kbd_fd, EVIOCGRAB, 1) < 0) {
        fprintf(2, "compositor: the keyboard is someone else's\n");
    }
    if (srv.ptr_fd >= 0 && ioctl(srv.ptr_fd, EVIOCGRAB, 1) < 0) {
        fprintf(2, "compositor: the pointer is someone else's\n");
    }
}

/* ---- Sending to a client's seat objects ---- */

static void each_keyboard_leave(struct window *w) {
    struct client *c = w->c;
    if (w->surface == NULL) {
        return;
    }
    uint serial = next_serial();
    for (int i = 0; i < c->nobjects; i++) {
        if (c->objects[i].iface == &wl_keyboard_interface) {
            event_begin(c, c->objects[i].id, WL_KEYBOARD_LEAVE);
            gui_wire_uint(&c->wire, serial);
            gui_wire_uint(&c->wire, w->surface->id);
            event_end(c);
        }
    }
}

static void keyboard_enter(struct client *c, struct object *k, struct surface *s) {
    uint keys[KEY_CNT];
    uint n = 0;
    for (uint code = 0; code < KEY_CNT; code++) {
        if (BIT(srv.held, code) && !BIT(srv.swallowed, code)) {
            keys[n++] = code;
        }
    }
    event_begin(c, k->id, WL_KEYBOARD_ENTER);
    gui_wire_uint(&c->wire, next_serial());
    gui_wire_uint(&c->wire, s->id);
    gui_wire_array(&c->wire, keys, n * 4);
    event_end(c);
    event_begin(c, k->id, WL_KEYBOARD_MODIFIERS);
    gui_wire_uint(&c->wire, next_serial());
    gui_wire_uint(&c->wire, srv.kbd.mods & ~GUI_MOD_CAPS);
    gui_wire_uint(&c->wire, 0);
    gui_wire_uint(&c->wire, srv.kbd.mods & GUI_MOD_CAPS);
    gui_wire_uint(&c->wire, 0);
    event_end(c);
}

static void each_keyboard_enter(struct window *w) {
    struct client *c = w->c;
    if (w->surface == NULL) {
        return;
    }
    for (int i = 0; i < c->nobjects; i++) {
        if (c->objects[i].iface == &wl_keyboard_interface) {
            keyboard_enter(c, &c->objects[i], w->surface);
        }
    }
}

void keyboard_bound(struct client *c, struct object *kbd) {
    if (srv.focus != NULL && srv.focus->c == c && srv.focus->surface != NULL) {
        keyboard_enter(c, kbd, srv.focus->surface);
    }
}

/* The pointer's place in surface s, 24.8 fixed point. */
static int local_x(struct surface *s) {
    return (srv.px - s->window->x) * 256;
}

static int local_y(struct surface *s) {
    return (srv.py - s->window->y) * 256;
}

static void pointer_enter(struct client *c, struct object *p, struct surface *s, uint serial) {
    event_begin(c, p->id, WL_POINTER_ENTER);
    gui_wire_uint(&c->wire, serial);
    gui_wire_uint(&c->wire, s->id);
    gui_wire_fixed(&c->wire, local_x(s));
    gui_wire_fixed(&c->wire, local_y(s));
    event_end(c);
}

void pointer_bound(struct client *c, struct object *ptr) {
    struct surface *s = srv.pointer_surface;
    if (s != NULL && s->c == c) {
        pointer_enter(c, ptr, s, next_serial());
    }
}

/* Sends a pointer event of `opcode` with up to four words to surface s's
 * client's wl_pointers; the serial, if the event has one, is made here
 * (word 0 given as ~0u). */
static void each_pointer(struct surface *s, uint opcode, int nwords, uint w0, uint w1, uint w2, uint w3) {
    struct client *c = s->c;
    if (w0 == ~0u) {
        w0 = next_serial();
    }
    for (int i = 0; i < c->nobjects; i++) {
        if (c->objects[i].iface != &wl_pointer_interface) {
            continue;
        }
        if (opcode == WL_POINTER_ENTER) {
            pointer_enter(c, &c->objects[i], s, w0);
            continue;
        }
        event_begin(c, c->objects[i].id, opcode);
        uint words[4];
        words[0] = w0;
        words[1] = w1;
        words[2] = w2;
        words[3] = w3;
        for (int j = 0; j < nwords; j++) {
            gui_wire_uint(&c->wire, words[j]);
        }
        event_end(c);
    }
}

/* ---- Windows ---- */

/* A window's frame and the grip around it (none above: the title bar
 * moves it). */
static struct gui_rect grip_rect(const struct window *w) {
    struct gui_rect r = window_frame(w);
    return gui_rect(r.x - RESIZE_GRIP, r.y, r.w + 2 * RESIZE_GRIP, r.h + RESIZE_GRIP);
}

/* The topmost window or layer surface at x, y, or NULL: the layers over
 * the windows, the windows (their grips too), the layers under them. */
static struct window *window_at(int x, int y) {
    for (int i = srv.nlayers - 1; i >= 0; i--) {
        struct window *l = srv.layers[i];
        if (l->layer >= ZWLR_LAYER_SHELL_V1_LAYER_TOP && gui_rect_contains(window_frame(l), x, y)) {
            return l;
        }
    }
    for (int i = srv.nstack - 1; i >= 0; i--) {
        if (gui_rect_contains(grip_rect(srv.stack[i]), x, y)) {
            return srv.stack[i];
        }
    }
    for (int i = srv.nlayers - 1; i >= 0; i--) {
        struct window *l = srv.layers[i];
        if (l->layer < ZWLR_LAYER_SHELL_V1_LAYER_TOP && gui_rect_contains(window_frame(l), x, y)) {
            return l;
        }
    }
    return NULL;
}

/* The edges of window w the pointer at x, y would resize it by
 * (xdg_toplevel's resize_edge), or 0: in the content or the title bar. */
static uint resize_edges_at(struct window *w, int x, int y) {
    struct gui_rect c = window_content(w);
    uint edges = 0;
    if (x < c.x) {
        edges |= XDG_TOPLEVEL_RESIZE_EDGE_LEFT;
    } else if (x >= c.x + c.w) {
        edges |= XDG_TOPLEVEL_RESIZE_EDGE_RIGHT;
    }
    if (y >= c.y + c.h) {
        edges |= XDG_TOPLEVEL_RESIZE_EDGE_BOTTOM;
    }
    return edges;
}

static int stack_index(struct window *w) {
    for (int i = 0; i < srv.nstack; i++) {
        if (srv.stack[i] == w) {
            return i;
        }
    }
    return -1;
}

static void raise_window(struct window *w) {
    int i = stack_index(w);
    if (i < 0 || i == srv.nstack - 1) {
        return;
    }
    memmove(srv.stack + i, srv.stack + i + 1, (size_t)(srv.nstack - 1 - i) * sizeof(struct window *));
    srv.stack[srv.nstack - 1] = w;
    damage(window_frame(w));
}

void window_focus(struct window *w) {
    raise_window(w);
    if (srv.focus == w) {
        return;
    }
    struct window *old = srv.focus;
    if (old != NULL) {
        each_keyboard_leave(old);
        damage(window_title(old));
    }
    srv.focus = w;
    if (old != NULL && old->layer < 0 && old->mapped) {
        toplevel_changed(old);
    }
    if (w != NULL) {
        each_keyboard_enter(w);
        damage(window_title(w));
        if (w->layer < 0) {
            toplevel_changed(w);
        }
        if (old == NULL || old->c != w->c) {
            selection_offer(w->c); /* the clipboard goes with the keyboard */
        }
        if (srv.verbose) {
            printf("compositor: focus \"%s\"\n", w->title);
        }
    }
}

void window_map(struct window *w) {
    if (w->mapped) {
        return;
    }
    if (w->layer >= 0) {
        layer_map(w);
        return;
    }
    if (srv.nstack == MAX_WINDOWS) {
        post_error(w->c, 1, WL_DISPLAY_ERROR_NO_MEMORY, "too many windows");
        return;
    }
    /* Each window down and to the right of the last in the work area,
     * back to its top left corner every eight; one that would go past
     * its right or bottom edge goes back in, as far as it can. */
    int n = srv.placed++ % 8;
    struct gui_rect c = window_content(w);
    struct gui_rect work = srv.work;
    w->x = work.x + 48 + n * 32;
    w->y = work.y + 48 + n * 32;
    if (w->x + c.w + BORDER > work.x + work.w) {
        w->x = work.x + work.w - c.w - BORDER;
    }
    if (w->y + c.h + BORDER > work.y + work.h) {
        w->y = work.y + work.h - c.h - BORDER;
    }
    if (w->x < work.x + BORDER) {
        w->x = work.x + BORDER;
    }
    if (w->y < work.y + TITLE_H + BORDER) {
        w->y = work.y + TITLE_H + BORDER;
    }
    w->mapped = 1;
    srv.stack[srv.nstack++] = w;
    damage(window_frame(w));
    window_focus(w);
    toplevel_announce(w);
}

void window_unmap(struct window *w) {
    if (!w->mapped) {
        return;
    }
    w->mapped = 0;
    w->closing = 0; /* gone as asked: shown again, it is asked again */
    damage(window_frame(w));
    int i = stack_index(w);
    if (i >= 0) {
        memmove(srv.stack + i, srv.stack + i + 1, (size_t)(srv.nstack - 1 - i) * sizeof(struct window *));
        srv.nstack--;
    }
    if (srv.moving == w) {
        srv.moving = NULL;
    }
    if (srv.resizing == w) {
        srv.resizing = NULL;
    }
    if (w->layer >= 0) {
        layer_unmap(w);
    } else {
        toplevel_closed(w);
    }
    if (w->surface != NULL) {
        wm_forget_surface(w->surface);
    }
    if (srv.focus == w) {
        srv.focus = NULL;
        if (w->surface != NULL && !w->c->dead) {
            each_keyboard_leave(w);
        }
        /* The keyboard goes to the window on top. */
        if (srv.nstack > 0) {
            window_focus(srv.stack[srv.nstack - 1]);
        }
    }
}

void wm_forget_surface(struct surface *s) {
    if (srv.pointer_surface == s) {
        srv.pointer_surface = NULL;
    }
    if (srv.grab == s) {
        srv.grab = NULL;
    }
}

void window_move_start(struct window *w) {
    srv.moving = w;
    srv.move_dx = srv.px - w->x;
    srv.move_dy = srv.py - w->y;
    if (srv.verbose) {
        printf("compositor: moving \"%s\"\n", w->title);
    }
}

void window_resize_start(struct window *w, uint edges) {
    struct gui_rect c = window_content(w);
    srv.resizing = w;
    srv.resize_edges = edges;
    srv.resize_x0 = srv.px;
    srv.resize_y0 = srv.py;
    srv.resize_w0 = c.w;
    srv.resize_h0 = c.h;
    srv.resize_right = c.x + c.w;
    w->pending_w = w->cfg_w;
    w->pending_h = w->cfg_h;
    if (srv.verbose) {
        printf("compositor: resizing \"%s\"\n", w->title);
    }
}

/* The size the pointer makes of the window being resized: configured now,
 * or once the last configure is acked. */
static void resize_motion(void) {
    struct window *w = srv.resizing;
    uint e = srv.resize_edges;
    int dx = srv.px - srv.resize_x0;
    int dy = srv.py - srv.resize_y0;
    int width = srv.resize_w0;
    int height = srv.resize_h0;
    if (e & XDG_TOPLEVEL_RESIZE_EDGE_RIGHT) {
        width += dx;
    } else if (e & XDG_TOPLEVEL_RESIZE_EDGE_LEFT) {
        width -= dx;
    }
    if (e & XDG_TOPLEVEL_RESIZE_EDGE_BOTTOM) {
        height += dy;
    } else if (e & XDG_TOPLEVEL_RESIZE_EDGE_TOP) {
        height -= dy;
    }
    int max_w = srv.work.w - 2 * BORDER;
    int max_h = srv.work.h - TITLE_H - 2 * BORDER;
    width = width > max_w ? max_w : width;
    height = height > max_h ? max_h : height;
    width = width < MIN_WIDTH ? MIN_WIDTH : width;
    height = height < MIN_HEIGHT ? MIN_HEIGHT : height;
    if (width == w->pending_w && height == w->pending_h) {
        return;
    }
    w->pending_w = width;
    w->pending_h = height;
    if (w->acked) {
        toplevel_configure(w, width, height, 1);
    }
}

/* The button let go: the last size, out of the resizing state. */
static void resize_end(void) {
    struct window *w = srv.resizing;
    srv.resizing = NULL;
    if (w->mapped && w->pending_w > 0 && w->pending_h > 0) {
        toplevel_configure(w, w->pending_w, w->pending_h, 0);
    }
}

void window_resized(struct window *w) {
    if (srv.resizing == w && (srv.resize_edges & XDG_TOPLEVEL_RESIZE_EDGE_LEFT)) {
        w->x = srv.resize_right - window_content(w).w;
    }
}

void window_close(struct window *w) {
    if (w->closing) {
        return;
    }
    w->closing = 1;
    w->close_deadline = uptime() + CLOSE_TICKS;
    struct client *c = w->c;
    event_begin(c, w->toplevel_id, XDG_TOPLEVEL_CLOSE);
    event_end(c);
}

int close_deadlines(void) {
    int next = -1;
    uint t = uptime();
    for (int i = 0; i < srv.nstack; i++) {
        struct window *w = srv.stack[i];
        if (!w->closing) {
            continue;
        }
        if ((int)(t - w->close_deadline) >= 0) {
            struct client *c = w->c;
            printf("compositor: client %d (pid %d) didn't close its window: killed\n", c->number, c->pid);
            if (c->pid > 0) {
                kill(c->pid);
            }
            c->dead = 1; /* disconnected by the main loop */
            continue;
        }
        int left = (int)(w->close_deadline - t);
        if (next < 0 || left < next) {
            next = left;
        }
    }
    return next;
}

/* ---- The pointer ---- */

/* The surface whose content is at the pointer, if the topmost window
 * there shows it there (not its decorations). */
static struct surface *surface_at_pointer(void) {
    struct window *w = window_at(srv.px, srv.py);
    if (w != NULL && w->surface != NULL && gui_rect_contains(window_content(w), srv.px, srv.py)) {
        return w->surface;
    }
    return NULL;
}

static void put32(uchar *p, uint v) {
    p[0] = (uchar)v;
    p[1] = (uchar)(v >> 8);
    p[2] = (uchar)(v >> 16);
    p[3] = (uchar)(v >> 24);
}

/* wl_pointer.motion to each of the client's pointers: one still queued,
 * the last event of all, is brought up to date instead (compositor.h's
 * motion_tail). */
static void send_motion(struct surface *s, uint time) {
    struct client *c = s->c;
    uint x = (uint)local_x(s);
    uint y = (uint)local_y(s);
    for (int i = 0; i < c->nobjects; i++) {
        struct object *o = &c->objects[i];
        if (o->iface != &wl_pointer_interface) {
            continue;
        }
        if (c->motion_tail == o->id && c->wire.out_len >= 20) {
            uchar *args = &c->wire.out[c->wire.out_len - 12];
            put32(args, time);
            put32(args + 4, x);
            put32(args + 8, y);
            continue;
        }
        event_begin(c, o->id, WL_POINTER_MOTION);
        gui_wire_uint(&c->wire, time);
        gui_wire_uint(&c->wire, x);
        gui_wire_uint(&c->wire, y);
        event_end(c);
        c->motion_tail = o->id;
    }
}

/* The pointer moved (or what is under it did): enter, leave, motion. */
static void pointer_focus(uint time, int moved) {
    struct surface *s = srv.grab != NULL ? srv.grab : surface_at_pointer();
    if (s != srv.pointer_surface) {
        struct surface *old = srv.pointer_surface;
        if (old != NULL) {
            each_pointer(old, WL_POINTER_LEAVE, 2, ~0u, old->id, 0, 0);
        }
        srv.pointer_surface = s;
        if (s != NULL) {
            each_pointer(s, WL_POINTER_ENTER, 0, ~0u, 0, 0, 0);
        }
    } else if (s != NULL && moved) {
        send_motion(s, time);
    }
}

void pointer_repick(void) {
    if (srv.moving == NULL && srv.resizing == NULL) {
        pointer_focus(now_ms(), 0);
    }
}

static void pointer_motion(int x, int y, uint time) {
    if (x < 0) {
        x = 0;
    }
    if (y < 0) {
        y = 0;
    }
    if (x >= srv.width) {
        x = srv.width - 1;
    }
    if (y >= srv.height) {
        y = srv.height - 1;
    }
    if (x == srv.px && y == srv.py) {
        return;
    }
    damage(cursor_rect());
    srv.px = x;
    srv.py = y;
    damage(cursor_rect());
    if (srv.resizing != NULL) {
        resize_motion();
        return;
    }
    if (srv.moving != NULL) {
        struct window *w = srv.moving;
        damage(window_frame(w));
        w->x = srv.px - srv.move_dx;
        w->y = srv.py - srv.move_dy;
        /* The title bar stays in the work area, to be taken again. */
        if (w->y < srv.work.y + TITLE_H + BORDER) {
            w->y = srv.work.y + TITLE_H + BORDER;
        }
        if (w->y > srv.height - 1) {
            w->y = srv.height - 1;
        }
        damage(window_frame(w));
        return;
    }
    pointer_focus(time, 1);
}

static int button_bit(uint code) {
    return code == BTN_LEFT ? 1 : code == BTN_RIGHT ? 2 : code == BTN_MIDDLE ? 4 : 0;
}

static void pointer_button(uint code, int pressed, uint time) {
    uint bit = (uint)button_bit(code);
    if (bit == 0) {
        return;
    }
    if (pressed) {
        if (srv.buttons & bit) {
            return;
        }
        int first = srv.buttons == 0;
        srv.buttons |= bit;
        struct window *w = window_at(srv.px, srv.py);
        if (first && srv.grab == NULL && w != NULL && w->layer >= 0) {
            if (w->keyboard) {
                window_focus(w);
            }
        } else if (first && srv.grab == NULL && w != NULL) {
            window_focus(w);
            if (gui_rect_contains(window_close_button(w), srv.px, srv.py)) {
                srv.consumed |= bit;
                window_close(w);
                return;
            }
            uint edges = resize_edges_at(w, srv.px, srv.py);
            if (edges != 0) {
                srv.consumed |= bit;
                window_resize_start(w, edges);
                return;
            }
            if (gui_rect_contains(window_title(w), srv.px, srv.py) ||
                !gui_rect_contains(window_content(w), srv.px, srv.py)) {
                srv.consumed |= bit;
                window_move_start(w);
                return;
            }
        }
        if (srv.moving != NULL || srv.resizing != NULL) {
            srv.consumed |= bit;
            return;
        }
        struct surface *s = srv.grab != NULL ? srv.grab : surface_at_pointer();
        if (s == NULL) {
            srv.consumed |= bit;
            return;
        }
        srv.grab = s;
        pointer_focus(time, 0);
        srv.grab_serial = next_serial();
        each_pointer(s, WL_POINTER_BUTTON, 4, srv.grab_serial, time, code, WL_POINTER_BUTTON_STATE_PRESSED);
        return;
    }
    if (!(srv.buttons & bit)) {
        return;
    }
    srv.buttons &= ~bit;
    if (srv.consumed & bit) {
        srv.consumed &= ~bit;
    } else if (srv.grab != NULL) {
        each_pointer(srv.grab, WL_POINTER_BUTTON, 4, ~0u, time, code, WL_POINTER_BUTTON_STATE_RELEASED);
    }
    if (srv.buttons == 0) {
        srv.moving = NULL;
        if (srv.resizing != NULL) {
            resize_end();
        }
        srv.grab = NULL;
        pointer_focus(time, 0); /* the pointer may be over another surface now */
    }
}

static void pointer_wheel(int notches, uint time) {
    struct surface *s = srv.pointer_surface;
    if (s == NULL || notches == 0 || srv.moving != NULL || srv.resizing != NULL) {
        return;
    }
    /* REL_WHEEL is positive up, wl_pointer.axis positive down; a notch is
     * 10, as Weston's. */
    each_pointer(s, WL_POINTER_AXIS, 3, time, WL_POINTER_AXIS_VERTICAL_SCROLL, (uint)(-notches * 10 * 256), 0);
}

/* What a packet of the pointer's events (up to SYN_REPORT) says. */
struct packet {
    int have_x;
    int x;
    int have_y;
    int y;
    int nbuttons;
    uint codes[8];
    int states[8];
    int wheel;
};

static struct packet packet;

void pointer_input(void) {
    struct input_event ev[32];
    long n = read(srv.ptr_fd, ev, sizeof(ev));
    if (n <= 0) {
        return;
    }
    for (int i = 0; i < (int)(n / (long)sizeof(struct input_event)); i++) {
        struct input_event *e = &ev[i];
        if (e->type == EV_ABS && e->code == ABS_X) {
            packet.have_x = 1;
            packet.x = e->value;
        } else if (e->type == EV_ABS && e->code == ABS_Y) {
            packet.have_y = 1;
            packet.y = e->value;
        } else if (e->type == EV_KEY && packet.nbuttons < 8) {
            packet.codes[packet.nbuttons] = e->code;
            packet.states[packet.nbuttons++] = e->value != 0;
        } else if (e->type == EV_REL && e->code == REL_WHEEL) {
            packet.wheel += e->value;
        } else if (e->type == EV_SYN && e->code == SYN_REPORT) {
            /* Where it went first, then the buttons there, then the wheel. */
            uint time = event_ms(e);
            pointer_motion(packet.have_x ? packet.x : srv.px, packet.have_y ? packet.y : srv.py, time);
            for (int j = 0; j < packet.nbuttons; j++) {
                pointer_button(packet.codes[j], packet.states[j], time);
            }
            pointer_wheel(packet.wheel, time);
            memset(&packet, 0, sizeof(packet));
        } else if (e->type == EV_SYN && e->code == SYN_DROPPED) {
            /* Events were lost: start the packet again from the next, and
             * let go of the buttons the device says are up, as the
             * keyboard's keys (a lost release would keep a grab). */
            memset(&packet, 0, sizeof(packet));
            uchar bits[KEY_CNT / 8];
            memset(bits, 0, sizeof(bits));
            ioctl(srv.ptr_fd, EVIOCGKEY(sizeof(bits)), bits);
            for (uint code = BTN_LEFT; code <= BTN_MIDDLE; code++) {
                if (!(bits[code / 8] & (1 << (code % 8)))) {
                    pointer_button(code, 0, event_ms(e));
                }
            }
        }
    }
}

/* ---- The keyboard ---- */

static void send_modifiers(void) {
    struct window *w = srv.focus;
    if (w == NULL || w->surface == NULL) {
        return;
    }
    struct client *c = w->c;
    uint serial = next_serial();
    for (int i = 0; i < c->nobjects; i++) {
        if (c->objects[i].iface == &wl_keyboard_interface) {
            event_begin(c, c->objects[i].id, WL_KEYBOARD_MODIFIERS);
            gui_wire_uint(&c->wire, serial);
            gui_wire_uint(&c->wire, srv.kbd.mods & ~GUI_MOD_CAPS);
            gui_wire_uint(&c->wire, 0);
            gui_wire_uint(&c->wire, srv.kbd.mods & GUI_MOD_CAPS);
            gui_wire_uint(&c->wire, 0);
            event_end(c);
        }
    }
}

static void send_key(uint code, int pressed, uint time) {
    struct window *w = srv.focus;
    if (w == NULL || w->surface == NULL) {
        return;
    }
    struct client *c = w->c;
    uint serial = next_serial();
    for (int i = 0; i < c->nobjects; i++) {
        if (c->objects[i].iface == &wl_keyboard_interface) {
            event_begin(c, c->objects[i].id, WL_KEYBOARD_KEY);
            gui_wire_uint(&c->wire, serial);
            gui_wire_uint(&c->wire, time);
            gui_wire_uint(&c->wire, code);
            gui_wire_uint(&c->wire, pressed ? WL_KEYBOARD_KEY_STATE_PRESSED : WL_KEYBOARD_KEY_STATE_RELEASED);
            event_end(c);
        }
    }
}

/* Alt+Tab: the lowest window to the top, with the keyboard. */
static void cycle_windows(void) {
    if (srv.nstack > 0) {
        window_focus(srv.stack[0]);
    }
}

static void key(uint code, int value, uint time) {
    if (code >= KEY_CNT || value == 2) {
        return; /* a repeat: clients repeat keys themselves */
    }
    int pressed = value != 0;
    if (pressed == (BIT(srv.held, code) != 0)) {
        return; /* a press of a key held, a release of one that isn't */
    }
    if (pressed) {
        SET(srv.held, code);
    } else {
        CLEAR(srv.held, code);
    }
    uint mods = srv.kbd.mods;
    gui_keyboard_key(&srv.kbd, code, pressed);
    if (!pressed && BIT(srv.swallowed, code)) {
        CLEAR(srv.swallowed, code);
    } else if (pressed && code == KEY_TAB && (srv.kbd.mods & GUI_MOD_ALT)) {
        SET(srv.swallowed, code);
        cycle_windows();
    } else if (pressed && (code == KEY_LEFTMETA || code == KEY_RIGHTMETA)) {
        SET(srv.swallowed, code);
        if (srv.launcher != NULL) {
            launch(srv.launcher);
        }
    } else {
        send_key(code, pressed, time);
    }
    if (srv.kbd.mods != mods) {
        send_modifiers();
    }
}

void keyboard_input(void) {
    struct input_event ev[32];
    long n = read(srv.kbd_fd, ev, sizeof(ev));
    if (n <= 0) {
        return;
    }
    for (int i = 0; i < (int)(n / (long)sizeof(struct input_event)); i++) {
        struct input_event *e = &ev[i];
        if (e->type == EV_KEY) {
            key(e->code, e->value, event_ms(e));
        } else if (e->type == EV_SYN && e->code == SYN_DROPPED) {
            /* Events were lost: the keys held now, as the device says;
             * those it says are up are let go of. */
            uchar bits[KEY_CNT / 8];
            memset(bits, 0, sizeof(bits));
            ioctl(srv.kbd_fd, EVIOCGKEY(sizeof(bits)), bits);
            for (uint code = 0; code < KEY_CNT; code++) {
                if (BIT(srv.held, code) && !(bits[code / 8] & (1 << (code % 8)))) {
                    key(code, 0, event_ms(e));
                }
            }
        }
    }
}
