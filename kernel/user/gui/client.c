/* The Wayland client (gui.h, "The Wayland client"): a connection to the
 * display server, its globals, windows drawn into wl_shm buffers, and the
 * events passed on to each window's handlers.
 *
 * Connecting: wl_display.get_registry, then a roundtrip (wl_display.sync
 * and its wl_callback.done) for the globals, each bound as it is
 * announced, then another for what binding them sent (the seat's
 * capabilities, which get the pointer and the keyboard).
 *
 * A window: a wl_surface, its xdg_surface and xdg_toplevel, committed once
 * without a buffer so that the server configures it; xdg_surface.configure
 * is acknowledged with the next commit. Its two buffers are halves of one
 * memfd, a wl_shm_pool, made again when the size changes. A frame is drawn
 * into the buffer the server isn't holding (wl_buffer.release gave it
 * back), after the rectangles the last frame damaged are copied into it
 * from the other: then it is the last frame, and only what changes needs
 * drawing. Committing attaches it, sends its damage (wl_surface.
 * damage_buffer), asks for a frame callback, and commits. */
#include "../gui.h"
#include "protocol.h"

/* Ids the client reuses once the server has said so (wl_display.delete_id);
 * past them it takes new ones. */
#define FREE_IDS 32

struct shm_buffer {
    uint id;
    int busy; /* the server holds it */
    struct gui_buffer buf;
};

struct gui_window {
    struct gui_display *d;
    struct gui_window *next;
    const struct gui_window_handlers *h;
    void *data;
    uint surface;
    uint xdg_surface;
    uint toplevel;
    uint pool;
    uint frame; /* the frame callback asked for, 0 if none */
    struct shm_buffer bufs[2];
    int fd; /* the memfd, -1 when there is none */
    void *mem;
    size_t mem_size;
    int width; /* the buffers' size */
    int height;
    int want_w; /* what the server configured (0: the window's choice) */
    int want_h;
    uint serial;  /* xdg_surface.configure's, to acknowledge */
    int configured;
    int acked;
    int committed; /* the buffer last committed, or -1 */
    int drawing;   /* the buffer gui_window_buffer handed out, or -1 */
    struct gui_damage damage;
    struct gui_damage last;
};

struct gui_display {
    struct gui_wire wire;
    uint next_id;
    uint free_ids[FREE_IDS];
    int nfree;
    uint registry;
    uint compositor;
    uint compositor_version;
    uint shm;
    uint wm_base;
    uint seat;
    uint pointer;
    uint keyboard;
    uint sync;
    int synced;
    uint button_serial; /* the last button press's, for a move */
    struct gui_window *windows;
    struct gui_window *pointer_focus;
    struct gui_window *keyboard_focus;
    int error;
};

static uint new_id(struct gui_display *d) {
    if (d->nfree > 0) {
        return d->free_ids[--d->nfree];
    }
    return d->next_id++;
}

static void msg_end(struct gui_display *d) {
    int r = gui_wire_end(&d->wire);
    if (r < 0 && d->error == 0) {
        d->error = r;
    }
}

/* A request with no arguments. */
static void request0(struct gui_display *d, uint object, uint opcode) {
    gui_wire_begin(&d->wire, object, opcode);
    msg_end(d);
}

/* A request of one uint (an object, a new id, a serial). */
static void request1(struct gui_display *d, uint object, uint opcode, uint arg) {
    gui_wire_begin(&d->wire, object, opcode);
    gui_wire_uint(&d->wire, arg);
    msg_end(d);
}

/* ---- Windows' buffers ---- */

static void free_buffers(struct gui_window *w) {
    struct gui_display *d = w->d;
    for (int i = 0; i < 2; i++) {
        if (w->bufs[i].id != 0) {
            request0(d, w->bufs[i].id, WL_BUFFER_DESTROY);
            w->bufs[i].id = 0;
        }
        w->bufs[i].busy = 0;
    }
    if (w->pool != 0) {
        request0(d, w->pool, WL_SHM_POOL_DESTROY);
        w->pool = 0;
    }
    if (w->mem != NULL) {
        munmap(w->mem, w->mem_size);
        w->mem = NULL;
    }
    if (w->fd >= 0) {
        close(w->fd);
        w->fd = -1;
    }
    w->committed = -1;
    w->drawing = -1;
    gui_damage_clear(&w->last);
}

/* Two buffers of width x height in a new pool. */
static int make_buffers(struct gui_window *w, int width, int height) {
    struct gui_display *d = w->d;
    free_buffers(w);
    if (width <= 0 || height <= 0 || width > 4096 || height > 4096) {
        return -EINVAL;
    }
    int stride = width * 4;
    size_t one = (size_t)stride * (size_t)height;
    w->fd = memfd_create("gui-window", MFD_CLOEXEC);
    if (w->fd < 0) {
        return w->fd;
    }
    int r = ftruncate(w->fd, (off_t)(2 * one));
    if (r < 0) {
        return r;
    }
    void *mem = mmap(NULL, 2 * one, PROT_READ | PROT_WRITE, MAP_SHARED, w->fd, 0);
    if ((long)mem < 0) {
        return (int)(long)mem;
    }
    w->mem = mem;
    w->mem_size = 2 * one;
    w->pool = new_id(d);
    gui_wire_begin(&d->wire, d->shm, WL_SHM_CREATE_POOL);
    gui_wire_uint(&d->wire, w->pool);
    gui_wire_fd(&d->wire, w->fd);
    gui_wire_int(&d->wire, (int)(2 * one));
    msg_end(d);
    for (int i = 0; i < 2; i++) {
        struct shm_buffer *b = &w->bufs[i];
        b->id = new_id(d);
        gui_wire_begin(&d->wire, w->pool, WL_SHM_POOL_CREATE_BUFFER);
        gui_wire_uint(&d->wire, b->id);
        gui_wire_int(&d->wire, (int)(i * one));
        gui_wire_int(&d->wire, width);
        gui_wire_int(&d->wire, height);
        gui_wire_int(&d->wire, stride);
        gui_wire_uint(&d->wire, WL_SHM_FORMAT_XRGB8888);
        msg_end(d);
        gui_buffer_init(&b->buf, (uint *)((uchar *)mem + i * one), width, height, width);
    }
    w->width = width;
    w->height = height;
    return d->error;
}

/* ---- Events ---- */

static struct gui_window *window_of_surface(struct gui_display *d, uint surface) {
    for (struct gui_window *w = d->windows; w != NULL; w = w->next) {
        if (w->surface == surface) {
            return w;
        }
    }
    return NULL;
}

static void bind_global(struct gui_display *d, uint name, const char *interface, uint version, uint *id) {
    *id = new_id(d);
    gui_wire_begin(&d->wire, d->registry, WL_REGISTRY_BIND);
    gui_wire_uint(&d->wire, name);
    gui_wire_string(&d->wire, interface);
    gui_wire_uint(&d->wire, version);
    gui_wire_uint(&d->wire, *id);
    msg_end(d);
}

static uint lower(uint a, uint b) {
    return a < b ? a : b;
}

static void registry_event(struct gui_display *d, struct gui_message *m) {
    if (m->opcode != WL_REGISTRY_GLOBAL) {
        return; /* global_remove: nothing this client binds goes away */
    }
    uint name = gui_msg_uint(m);
    const char *interface = gui_msg_string(m);
    uint version = gui_msg_uint(m);
    if (m->bad || interface == NULL || version == 0) {
        return;
    }
    if (strcmp(interface, "wl_compositor") == 0 && d->compositor == 0) {
        d->compositor_version = lower(version, WL_COMPOSITOR_VERSION);
        bind_global(d, name, interface, d->compositor_version, &d->compositor);
    } else if (strcmp(interface, "wl_shm") == 0 && d->shm == 0) {
        bind_global(d, name, interface, lower(version, WL_SHM_VERSION), &d->shm);
    } else if (strcmp(interface, "xdg_wm_base") == 0 && d->wm_base == 0) {
        bind_global(d, name, interface, lower(version, XDG_WM_BASE_VERSION), &d->wm_base);
    } else if (strcmp(interface, "wl_seat") == 0 && d->seat == 0) {
        bind_global(d, name, interface, lower(version, WL_SEAT_VERSION), &d->seat);
    }
}

static void seat_event(struct gui_display *d, struct gui_message *m) {
    if (m->opcode != WL_SEAT_CAPABILITIES) {
        return;
    }
    uint caps = gui_msg_uint(m);
    if ((caps & WL_SEAT_CAPABILITY_POINTER) && d->pointer == 0) {
        d->pointer = new_id(d);
        request1(d, d->seat, WL_SEAT_GET_POINTER, d->pointer);
    }
    if ((caps & WL_SEAT_CAPABILITY_KEYBOARD) && d->keyboard == 0) {
        d->keyboard = new_id(d);
        request1(d, d->seat, WL_SEAT_GET_KEYBOARD, d->keyboard);
    }
}

/* A fixed-point coordinate's whole pixels (24.8, rounded down). */
static int pixels(int fixed) {
    return fixed >> 8;
}

/* One notch of a wheel, as the server sends it: 10, as Weston does. */
#define AXIS_NOTCH (10 * 256)

static void pointer_event(struct gui_display *d, struct gui_message *m) {
    struct gui_window *w = d->pointer_focus;
    switch (m->opcode) {
    case WL_POINTER_ENTER: {
        gui_msg_uint(m);
        uint surface = gui_msg_uint(m);
        int x = gui_msg_fixed(m);
        int y = gui_msg_fixed(m);
        w = window_of_surface(d, surface);
        d->pointer_focus = w;
        if (w != NULL && w->h->pointer != NULL) {
            w->h->pointer(w, pixels(x), pixels(y));
        }
        break;
    }
    case WL_POINTER_LEAVE:
        d->pointer_focus = NULL;
        if (w != NULL && w->h->leave != NULL) {
            w->h->leave(w);
        }
        break;
    case WL_POINTER_MOTION: {
        gui_msg_uint(m);
        int x = gui_msg_fixed(m);
        int y = gui_msg_fixed(m);
        if (w != NULL && w->h->pointer != NULL) {
            w->h->pointer(w, pixels(x), pixels(y));
        }
        break;
    }
    case WL_POINTER_BUTTON: {
        uint serial = gui_msg_uint(m);
        gui_msg_uint(m);
        uint button = gui_msg_uint(m);
        uint state = gui_msg_uint(m);
        if (state == WL_POINTER_BUTTON_STATE_PRESSED) {
            d->button_serial = serial;
        }
        if (w != NULL && w->h->button != NULL) {
            w->h->button(w, button, state == WL_POINTER_BUTTON_STATE_PRESSED);
        }
        break;
    }
    case WL_POINTER_AXIS: {
        gui_msg_uint(m);
        uint axis = gui_msg_uint(m);
        int value = gui_msg_fixed(m);
        if (w != NULL && w->h->scroll != NULL && axis == WL_POINTER_AXIS_VERTICAL_SCROLL && value != 0) {
            int notches = value / AXIS_NOTCH;
            if (notches == 0) {
                notches = value > 0 ? 1 : -1;
            }
            w->h->scroll(w, notches);
        }
        break;
    }
    }
}

static void keyboard_event(struct gui_display *d, struct gui_message *m) {
    struct gui_window *w = d->keyboard_focus;
    switch (m->opcode) {
    case WL_KEYBOARD_KEYMAP: {
        /* NO_KEYMAP from Camelot's server: the client maps the codes
         * itself (gui/keymap.c). Any keymap's descriptor is closed. */
        gui_msg_uint(m);
        int fd = gui_msg_fd(m);
        if (fd >= 0) {
            close(fd);
        }
        break;
    }
    case WL_KEYBOARD_ENTER: {
        gui_msg_uint(m);
        uint surface = gui_msg_uint(m);
        w = window_of_surface(d, surface);
        d->keyboard_focus = w;
        if (w != NULL && w->h->focus != NULL) {
            w->h->focus(w, 1);
        }
        break;
    }
    case WL_KEYBOARD_LEAVE:
        d->keyboard_focus = NULL;
        if (w != NULL && w->h->focus != NULL) {
            w->h->focus(w, 0);
        }
        break;
    case WL_KEYBOARD_KEY: {
        gui_msg_uint(m);
        gui_msg_uint(m);
        uint key = gui_msg_uint(m);
        uint state = gui_msg_uint(m);
        if (w != NULL && w->h->key != NULL) {
            w->h->key(w, key, state == WL_KEYBOARD_KEY_STATE_PRESSED);
        }
        break;
    }
    }
}

/* An event for a window's objects; 0 if it is none of theirs. */
static int window_event(struct gui_display *d, struct gui_message *m) {
    for (struct gui_window *w = d->windows; w != NULL; w = w->next) {
        if (m->object == w->xdg_surface && m->opcode == XDG_SURFACE_CONFIGURE) {
            w->serial = gui_msg_uint(m);
            w->configured = 1;
            w->acked = 0;
            return 1;
        }
        if (m->object == w->toplevel) {
            if (m->opcode == XDG_TOPLEVEL_CONFIGURE) {
                w->want_w = gui_msg_int(m);
                w->want_h = gui_msg_int(m);
                if (w->want_w < 0 || w->want_h < 0) {
                    w->want_w = 0;
                    w->want_h = 0;
                }
            } else if (m->opcode == XDG_TOPLEVEL_CLOSE && w->h->close != NULL) {
                w->h->close(w);
            }
            return 1;
        }
        if (m->object == w->frame && m->opcode == WL_CALLBACK_DONE) {
            w->frame = 0;
            if (w->h->frame != NULL) {
                w->h->frame(w);
            }
            return 1;
        }
        for (int i = 0; i < 2; i++) {
            if (w->bufs[i].id != 0 && m->object == w->bufs[i].id && m->opcode == WL_BUFFER_RELEASE) {
                w->bufs[i].busy = 0;
                return 1;
            }
        }
        if (m->object == w->surface) {
            return 1; /* enter, leave: outputs, which a window doesn't need */
        }
    }
    return 0;
}

static void display_event(struct gui_display *d, struct gui_message *m) {
    if (m->opcode == WL_DISPLAY_ERROR) {
        uint object = gui_msg_uint(m);
        uint code = gui_msg_uint(m);
        const char *message = gui_msg_string(m);
        fprintf(2, "gui: the display server: object %u, error %u: %s\n", object, code,
                message != NULL ? message : "");
        d->error = -EPROTO;
    } else if (m->opcode == WL_DISPLAY_DELETE_ID) {
        uint id = gui_msg_uint(m);
        if (!m->bad && id > WL_DISPLAY_ID && id < d->next_id && d->nfree < FREE_IDS) {
            d->free_ids[d->nfree++] = id;
        }
    }
}

static void handle(struct gui_display *d, struct gui_message *m) {
    if (m->object == 0) {
        fprintf(2, "gui: an event for object 0\n");
        d->error = -EPROTO;
        return;
    }
    if (m->object == WL_DISPLAY_ID) {
        display_event(d, m);
    } else if (m->object == d->registry) {
        registry_event(d, m);
    } else if (m->object == d->sync && d->sync != 0) {
        d->synced = 1;
        d->sync = 0;
    } else if (m->object == d->wm_base && m->opcode == XDG_WM_BASE_PING) {
        request1(d, d->wm_base, XDG_WM_BASE_PONG, gui_msg_uint(m));
    } else if (m->object == d->seat) {
        seat_event(d, m);
    } else if (m->object == d->pointer && d->pointer != 0) {
        pointer_event(d, m);
    } else if (m->object == d->keyboard && d->keyboard != 0) {
        keyboard_event(d, m);
    } else {
        /* wl_shm.format, and events for objects already destroyed. */
        window_event(d, m);
    }
    if (m->bad && d->error == 0) {
        fprintf(2, "gui: a broken event for object %u\n", m->object);
        d->error = -EPROTO;
    }
}

/* Handles every whole message received: how many. */
static int handle_all(struct gui_display *d) {
    int n = 0;
    struct gui_message m;
    int r;
    while (d->error == 0 && (r = gui_wire_next(&d->wire, &m)) != 0) {
        if (r < 0) {
            d->error = r;
            break;
        }
        handle(d, &m);
        n++;
    }
    return n;
}

int gui_dispatch(struct gui_display *d, int timeout) {
    if (d->error < 0) {
        return d->error;
    }
    int r = gui_wire_flush(&d->wire);
    if (r < 0) {
        return d->error = r;
    }
    int n = handle_all(d);
    if (n > 0 || d->error < 0) {
        gui_wire_flush(&d->wire);
        return d->error < 0 ? d->error : n;
    }
    struct pollfd p;
    p.fd = d->wire.fd;
    p.events = POLLIN;
    p.revents = 0;
    r = poll(&p, 1, timeout);
    if (r < 0) {
        return d->error = r;
    }
    if (r == 0) {
        return 0;
    }
    r = gui_wire_read(&d->wire, 1);
    if (r == 0) {
        return d->error = -EPIPE;
    }
    if (r < 0 && r != -EAGAIN) {
        return d->error = r;
    }
    n = handle_all(d);
    r = gui_wire_flush(&d->wire);
    if (r < 0 && d->error == 0) {
        d->error = r;
    }
    return d->error < 0 ? d->error : n;
}

int gui_roundtrip(struct gui_display *d) {
    d->sync = new_id(d);
    d->synced = 0;
    request1(d, WL_DISPLAY_ID, WL_DISPLAY_SYNC, d->sync);
    while (!d->synced) {
        int r = gui_dispatch(d, -1);
        if (r < 0) {
            return r;
        }
    }
    return 0;
}

/* ---- Connecting ---- */

/* The socket's path: WAYLAND_DISPLAY (wayland-0 if unset) in
 * XDG_RUNTIME_DIR (/run if unset), or WAYLAND_DISPLAY itself if it has a
 * '/', as libwayland finds it. */
static int socket_path(const char *name, char *out, size_t size) {
    if (name == NULL) {
        name = getenv("WAYLAND_DISPLAY");
    }
    if (name == NULL || *name == 0) {
        name = "wayland-0";
    }
    int n;
    if (strchr(name, '/') != NULL) {
        n = snprintf(out, size, "%s", name);
    } else {
        const char *dir = getenv("XDG_RUNTIME_DIR");
        n = snprintf(out, size, "%s/%s", dir != NULL && *dir != 0 ? dir : "/run", name);
    }
    return n < (int)size ? 0 : -ENAMETOOLONG;
}

struct gui_display *gui_connect(const char *name, int *err) {
    struct sockaddr_un a;
    memset(&a, 0, sizeof(a));
    a.sun_family = AF_UNIX;
    int r = socket_path(name, a.sun_path, sizeof(a.sun_path));
    if (r < 0) {
        *err = r;
        return NULL;
    }
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0) {
        *err = fd;
        return NULL;
    }
    r = connect(fd, &a);
    if (r < 0) {
        close(fd);
        *err = r;
        return NULL;
    }
    struct gui_display *d = calloc(1, sizeof(struct gui_display));
    if (d == NULL) {
        close(fd);
        *err = -ENOMEM;
        return NULL;
    }
    gui_wire_init(&d->wire, fd);
    d->next_id = WL_DISPLAY_ID + 1;
    d->registry = new_id(d);
    request1(d, WL_DISPLAY_ID, WL_DISPLAY_GET_REGISTRY, d->registry);
    /* The globals, then what binding them brought. */
    r = gui_roundtrip(d);
    if (r == 0) {
        r = gui_roundtrip(d);
    }
    if (r == 0 && (d->compositor == 0 || d->shm == 0 || d->wm_base == 0)) {
        fprintf(2, "gui: the display server lacks wl_compositor, wl_shm or xdg_wm_base\n");
        r = -ENOENT;
    }
    if (r < 0) {
        gui_disconnect(d);
        *err = r;
        return NULL;
    }
    *err = 0;
    return d;
}

void gui_disconnect(struct gui_display *d) {
    while (d->windows != NULL) {
        gui_window_destroy(d->windows);
    }
    gui_wire_flush(&d->wire);
    gui_wire_release(&d->wire);
    close(d->wire.fd);
    free(d);
}

int gui_display_fd(struct gui_display *d) {
    return d->wire.fd;
}

/* ---- Windows ---- */

struct gui_window *gui_window_open(struct gui_display *d, const char *title, int width, int height,
                                   const struct gui_window_handlers *h, void *data) {
    struct gui_window *w = calloc(1, sizeof(struct gui_window));
    if (w == NULL) {
        return NULL;
    }
    w->d = d;
    w->h = h;
    w->data = data;
    w->fd = -1;
    w->committed = -1;
    w->drawing = -1;
    w->width = width;
    w->height = height;
    w->next = d->windows;
    d->windows = w;
    w->surface = new_id(d);
    request1(d, d->compositor, WL_COMPOSITOR_CREATE_SURFACE, w->surface);
    w->xdg_surface = new_id(d);
    gui_wire_begin(&d->wire, d->wm_base, XDG_WM_BASE_GET_XDG_SURFACE);
    gui_wire_uint(&d->wire, w->xdg_surface);
    gui_wire_uint(&d->wire, w->surface);
    msg_end(d);
    w->toplevel = new_id(d);
    request1(d, w->xdg_surface, XDG_SURFACE_GET_TOPLEVEL, w->toplevel);
    gui_wire_begin(&d->wire, w->toplevel, XDG_TOPLEVEL_SET_TITLE);
    gui_wire_string(&d->wire, title);
    msg_end(d);
    /* The first commit, with no buffer: the server answers with the first
     * configure, which a buffer must wait for. */
    request0(d, w->surface, WL_SURFACE_COMMIT);
    while (!w->configured && d->error == 0) {
        if (gui_dispatch(d, -1) < 0) {
            break;
        }
    }
    if (d->error < 0) {
        gui_window_destroy(w);
        return NULL;
    }
    if (w->want_w > 0 && w->want_h > 0) {
        w->width = w->want_w;
        w->height = w->want_h;
    }
    return w;
}

void gui_window_destroy(struct gui_window *w) {
    struct gui_display *d = w->d;
    free_buffers(w);
    if (w->frame != 0) {
        w->frame = 0; /* its done, if it comes, is for no one */
    }
    request0(d, w->toplevel, XDG_TOPLEVEL_DESTROY);
    request0(d, w->xdg_surface, XDG_SURFACE_DESTROY);
    request0(d, w->surface, WL_SURFACE_DESTROY);
    gui_wire_flush(&d->wire);
    for (struct gui_window **p = &d->windows; *p != NULL; p = &(*p)->next) {
        if (*p == w) {
            *p = w->next;
            break;
        }
    }
    if (d->pointer_focus == w) {
        d->pointer_focus = NULL;
    }
    if (d->keyboard_focus == w) {
        d->keyboard_focus = NULL;
    }
    free(w);
}

void *gui_window_data(struct gui_window *w) {
    return w->data;
}

int gui_window_width(struct gui_window *w) {
    return w->want_w > 0 ? w->want_w : w->width;
}

int gui_window_height(struct gui_window *w) {
    return w->want_h > 0 ? w->want_h : w->height;
}

struct gui_buffer *gui_window_buffer(struct gui_window *w) {
    struct gui_display *d = w->d;
    int width = gui_window_width(w);
    int height = gui_window_height(w);
    if (w->drawing >= 0) {
        if (width == w->width && height == w->height) {
            return &w->bufs[w->drawing].buf;
        }
        w->drawing = -1; /* configured to another size since */
    }
    if (w->mem == NULL || width != w->width || height != w->height) {
        if (make_buffers(w, width, height) < 0) {
            return NULL;
        }
    }
    int next = w->committed < 0 ? 0 : 1 - w->committed;
    while (w->bufs[next].busy) {
        if (gui_dispatch(d, -1) < 0) {
            return NULL;
        }
    }
    if (w->committed >= 0) {
        /* Bring it up to the last frame: what that one changed. */
        struct gui_buffer *from = &w->bufs[w->committed].buf;
        for (int i = 0; i < w->last.count; i++) {
            struct gui_rect r = w->last.rects[i];
            gui_blit(&w->bufs[next].buf, r.x, r.y, from, r);
        }
    }
    w->drawing = next;
    gui_damage_clear(&w->damage);
    return &w->bufs[next].buf;
}

void gui_window_damage(struct gui_window *w, struct gui_rect r) {
    gui_damage_add(&w->damage, gui_rect_intersect(r, gui_rect(0, 0, w->width, w->height)));
}

int gui_window_commit(struct gui_window *w) {
    struct gui_display *d = w->d;
    if (w->drawing < 0) {
        return -EINVAL;
    }
    if (!w->acked) {
        request1(d, w->xdg_surface, XDG_SURFACE_ACK_CONFIGURE, w->serial);
        w->acked = 1;
    }
    struct shm_buffer *b = &w->bufs[w->drawing];
    gui_wire_begin(&d->wire, w->surface, WL_SURFACE_ATTACH);
    gui_wire_uint(&d->wire, b->id);
    gui_wire_int(&d->wire, 0);
    gui_wire_int(&d->wire, 0);
    msg_end(d);
    /* A new buffer's first frame is all new to the server. */
    if (w->committed < 0) {
        gui_damage_clear(&w->damage);
        gui_damage_add(&w->damage, gui_rect(0, 0, w->width, w->height));
    }
    uint op = d->compositor_version >= 4 ? WL_SURFACE_DAMAGE_BUFFER : WL_SURFACE_DAMAGE;
    for (int i = 0; i < w->damage.count; i++) {
        struct gui_rect r = w->damage.rects[i];
        gui_wire_begin(&d->wire, w->surface, op);
        gui_wire_int(&d->wire, r.x);
        gui_wire_int(&d->wire, r.y);
        gui_wire_int(&d->wire, r.w);
        gui_wire_int(&d->wire, r.h);
        msg_end(d);
    }
    if (w->frame == 0) {
        w->frame = new_id(d);
        request1(d, w->surface, WL_SURFACE_FRAME, w->frame);
    }
    request0(d, w->surface, WL_SURFACE_COMMIT);
    b->busy = 1;
    w->committed = w->drawing;
    w->drawing = -1;
    w->last = w->damage;
    gui_damage_clear(&w->damage);
    int r = gui_wire_flush(&d->wire);
    return r < 0 ? r : d->error;
}

void gui_window_move(struct gui_window *w) {
    struct gui_display *d = w->d;
    if (d->seat == 0) {
        return;
    }
    gui_wire_begin(&d->wire, w->toplevel, XDG_TOPLEVEL_MOVE);
    gui_wire_uint(&d->wire, d->seat);
    gui_wire_uint(&d->wire, d->button_serial);
    msg_end(d);
    gui_wire_flush(&d->wire);
}

/* ---- Running a ui in a window ---- */

struct run {
    struct gui_ui *ui;
    int closed;
    int frame_ready;
};

static void run_close(struct gui_window *w) {
    ((struct run *)gui_window_data(w))->closed = 1;
}

static void run_pointer(struct gui_window *w, int x, int y) {
    gui_ui_pointer(((struct run *)gui_window_data(w))->ui, x, y);
}

static void run_leave(struct gui_window *w) {
    gui_ui_leave(((struct run *)gui_window_data(w))->ui);
}

static void run_button(struct gui_window *w, uint button, int pressed) {
    gui_ui_button(((struct run *)gui_window_data(w))->ui, button, pressed);
}

static void run_scroll(struct gui_window *w, int amount) {
    gui_ui_scroll(((struct run *)gui_window_data(w))->ui, amount);
}

static void run_key(struct gui_window *w, uint code, int pressed) {
    gui_ui_key(((struct run *)gui_window_data(w))->ui, code, pressed);
}

static void run_frame(struct gui_window *w) {
    ((struct run *)gui_window_data(w))->frame_ready = 1;
}

static const struct gui_window_handlers run_handlers = {
    .close = run_close,
    .pointer = run_pointer,
    .leave = run_leave,
    .button = run_button,
    .scroll = run_scroll,
    .key = run_key,
    .frame = run_frame,
};

int gui_run(struct gui_display *d, struct gui_ui *ui, const char *title, int width, int height,
            const int *quit) {
    struct run state;
    state.ui = ui;
    state.closed = 0;
    state.frame_ready = 1;
    struct gui_window *w = gui_window_open(d, title, width, height, &run_handlers, &state);
    if (w == NULL) {
        return d->error < 0 ? d->error : -ENOMEM;
    }
    int laid_w = 0;
    int laid_h = 0;
    int r = 0;
    while (!state.closed && (quit == NULL || !*quit)) {
        int ww = gui_window_width(w);
        int wh = gui_window_height(w);
        if (ww != laid_w || wh != laid_h) {
            gui_ui_layout(ui, gui_rect(0, 0, ww, wh));
            laid_w = ww;
            laid_h = wh;
        }
        if (state.frame_ready) {
            struct gui_buffer *b = gui_window_buffer(w);
            if (b == NULL) {
                r = d->error < 0 ? d->error : -ENOMEM;
                break;
            }
            gui_ui_draw(ui, b);
            if (ui->damage.count > 0) {
                for (int i = 0; i < ui->damage.count; i++) {
                    gui_window_damage(w, ui->damage.rects[i]);
                }
                gui_damage_clear(&ui->damage);
                r = gui_window_commit(w);
                if (r < 0) {
                    break;
                }
                state.frame_ready = 0;
            }
        }
        r = gui_dispatch(d, -1);
        if (r < 0) {
            break;
        }
        r = 0;
    }
    gui_window_destroy(w);
    return r;
}
