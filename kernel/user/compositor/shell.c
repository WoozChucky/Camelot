/* What the desktop shell needs of the server (#185): wlroots' layer shell
 * and foreign toplevel management (protocols/wlr-*.xml), version 1 of
 * each, and the clipboard, wl_data_device_manager version 1.
 *
 * - A layer surface is a struct window with a layer (0 to 3) and no
 *   decorations, kept in srv.layers: drawn under the windows (background,
 *   bottom) or over them (top, overlay), placed at the edges it is anchored
 *   to. One with an exclusive zone takes that much of its edge from the
 *   work area (srv.work), where windows are placed and moved; one whose
 *   zone is 0 is placed inside it, clear of the others. One that asks for
 *   the keyboard gets it when it is shown and when it is clicked, and
 *   loses it as a window does.
 * - Every client that binds the foreign toplevel manager is told of each
 *   window shown (a handle, an object the server makes: its title, whether
 *   it is activated, done), of the changes, and of its end (closed). A
 *   handle's activate raises the window and gives it the keyboard; close
 *   asks its client to close it, as its close button does.
 * - The clipboard: a client sets the selection to a wl_data_source of its
 *   own, with the types it offers; the client with the keyboard is offered
 *   it (a wl_data_offer the server makes, its types, then
 *   wl_data_device.selection) when it gets the keyboard and when the
 *   selection changes. An offer's receive passes the descriptor it came
 *   with (a pipe's end) to the source's client in wl_data_source.send:
 *   the server holds it until that is sent (compositor.h's MAX_HELD_FDS).
 *   A source replaced is cancelled. Dragging is not had: start_drag does
 *   nothing. */
#include "compositor.h"

/* ---- Server-made objects ---- */

/* A new object of the server's, for client c (no delete_id when it goes:
 * libwayland's server ids). */
static struct object *server_object(struct client *c, const struct wl_interface *iface, int version, void *data) {
    if (c->next_server_id == 0) {
        c->next_server_id = SERVER_ID_START;
    }
    return object_new(c, c->next_server_id++, iface, version, data);
}

/* ---- The layer shell ---- */

static int is_layer_object(const struct object *o) {
    return o->iface == &zwlr_layer_surface_v1_interface;
}

void layer_shell_request(struct client *c, struct object *o, uint op, struct args *a) {
    if (op == ZWLR_LAYER_SHELL_V1_DESTROY) {
        object_destroy(c, o);
        return;
    }
    /* get_layer_surface: id, surface, output (the one there is), layer,
     * namespace. */
    uint id = a->u[0];
    struct surface *s = a->o[1]->data;
    uint layer = a->u[3];
    if (s->window != NULL) {
        post_error(c, o->id, ZWLR_LAYER_SHELL_V1_ERROR_ROLE, "the surface has a role already");
        return;
    }
    if (layer > ZWLR_LAYER_SHELL_V1_LAYER_OVERLAY) {
        post_error(c, o->id, ZWLR_LAYER_SHELL_V1_ERROR_INVALID_LAYER, "invalid layer");
        return;
    }
    if (s->has_content || (s->attached && s->buffer != NULL)) {
        post_error(c, o->id, ZWLR_LAYER_SHELL_V1_ERROR_ALREADY_CONSTRUCTED, "the surface has a buffer already");
        return;
    }
    struct window *w = malloc(sizeof(struct window));
    if (w == NULL) {
        post_error(c, 1, WL_DISPLAY_ERROR_NO_MEMORY, "no memory for a layer surface");
        return;
    }
    memset(w, 0, sizeof(struct window));
    w->c = c;
    w->surface = s;
    w->layer = (int)layer;
    w->layer_id = id;
    snprintf(w->title, sizeof(w->title), "%s", a->s[4]);
    if (object_new(c, id, &zwlr_layer_surface_v1_interface, o->version, w) == NULL) {
        free(w);
        return;
    }
    s->window = w;
}

#define ANCHOR_H (ZWLR_LAYER_SURFACE_V1_ANCHOR_LEFT | ZWLR_LAYER_SURFACE_V1_ANCHOR_RIGHT)
#define ANCHOR_V (ZWLR_LAYER_SURFACE_V1_ANCHOR_TOP | ZWLR_LAYER_SURFACE_V1_ANCHOR_BOTTOM)

/* The edge a layer surface's exclusive zone is at, as the protocol says
 * (anchored to one edge, or to one and both of those beside it), or 0. */
static uint zone_edge(const struct window *w) {
    uint a = w->anchor;
    static const uint edges[] = {
        ZWLR_LAYER_SURFACE_V1_ANCHOR_TOP,
        ZWLR_LAYER_SURFACE_V1_ANCHOR_BOTTOM,
        ZWLR_LAYER_SURFACE_V1_ANCHOR_LEFT,
        ZWLR_LAYER_SURFACE_V1_ANCHOR_RIGHT,
    };
    for (int i = 0; i < 4; i++) {
        uint e = edges[i];
        uint beside = (e & ANCHOR_V) ? ANCHOR_H : ANCHOR_V;
        if (a == e || a == (e | beside)) {
            return e;
        }
    }
    return 0;
}

/* The area a layer surface is placed in: the screen, or the work area for
 * one whose zone is 0. */
static struct gui_rect layer_area(const struct window *w) {
    if (w->zone == 0) {
        return srv.work;
    }
    return gui_rect(0, 0, srv.width, srv.height);
}

/* The size a layer surface is configured to: what it asked, the area's
 * between two edges it is anchored to for a 0. */
static void layer_size(const struct window *w, int *width, int *height) {
    struct gui_rect area = layer_area(w);
    *width = w->req_w;
    *height = w->req_h;
    if (*width == 0) {
        *width = area.w - w->margin[1] - w->margin[3];
    }
    if (*height == 0) {
        *height = area.h - w->margin[0] - w->margin[2];
    }
    if (*width < 1) {
        *width = 1;
    }
    if (*height < 1) {
        *height = 1;
    }
}

/* Places a layer surface by its anchors and margins, in its area. */
static void layer_place(struct window *w) {
    struct gui_rect area = layer_area(w);
    struct gui_rect c = window_content(w);
    uint a = w->anchor;
    int top = w->margin[0];
    int right = w->margin[1];
    int bottom = w->margin[2];
    int left = w->margin[3];
    if ((a & ANCHOR_H) == ZWLR_LAYER_SURFACE_V1_ANCHOR_LEFT) {
        w->x = area.x + left;
    } else if ((a & ANCHOR_H) == ZWLR_LAYER_SURFACE_V1_ANCHOR_RIGHT) {
        w->x = area.x + area.w - right - c.w;
    } else {
        w->x = area.x + left + (area.w - left - right - c.w) / 2;
    }
    if ((a & ANCHOR_V) == ZWLR_LAYER_SURFACE_V1_ANCHOR_TOP) {
        w->y = area.y + top;
    } else if ((a & ANCHOR_V) == ZWLR_LAYER_SURFACE_V1_ANCHOR_BOTTOM) {
        w->y = area.y + area.h - bottom - c.h;
    } else {
        w->y = area.y + top + (area.h - top - bottom - c.h) / 2;
    }
}

/* The work area again, from the layer surfaces shown, and each placed. */
void layers_arrange(void) {
    struct gui_rect work = gui_rect(0, 0, srv.width, srv.height);
    for (int i = 0; i < srv.nlayers; i++) {
        struct window *w = srv.layers[i];
        uint e = zone_edge(w);
        if (w->zone <= 0 || e == 0) {
            continue;
        }
        if (e == ZWLR_LAYER_SURFACE_V1_ANCHOR_TOP) {
            int n = w->zone + w->margin[0];
            work.y += n;
            work.h -= n;
        } else if (e == ZWLR_LAYER_SURFACE_V1_ANCHOR_BOTTOM) {
            work.h -= w->zone + w->margin[2];
        } else if (e == ZWLR_LAYER_SURFACE_V1_ANCHOR_LEFT) {
            int n = w->zone + w->margin[3];
            work.x += n;
            work.w -= n;
        } else {
            work.w -= w->zone + w->margin[1];
        }
    }
    if (work.w < 1 || work.h < 1) {
        work = gui_rect(0, 0, srv.width, srv.height); /* zones that take it all: none */
    }
    srv.work = work;
    for (int i = 0; i < srv.nlayers; i++) {
        struct window *w = srv.layers[i];
        struct gui_rect before = window_frame(w);
        layer_place(w);
        struct gui_rect after = window_frame(w);
        if (before.x != after.x || before.y != after.y) {
            damage(before);
            damage(after);
        }
    }
}

/* Sends a layer surface's configure, if its size is not the last one
 * sent (or none was). */
void layer_configure(struct window *w) {
    int width;
    int height;
    layer_size(w, &width, &height);
    if (w->configure_serial != 0 && width == w->cfg_w && height == w->cfg_h) {
        return;
    }
    struct client *c = w->c;
    w->cfg_w = width;
    w->cfg_h = height;
    w->configure_serial = next_serial();
    w->acked = 0;
    event_begin(c, w->layer_id, ZWLR_LAYER_SURFACE_V1_CONFIGURE);
    gui_wire_uint(&c->wire, w->configure_serial);
    gui_wire_uint(&c->wire, (uint)width);
    gui_wire_uint(&c->wire, (uint)height);
    event_end(c);
}

/* A layer surface's first frame: shown, placed, the keyboard its if it
 * asked. */
void layer_map(struct window *w) {
    if (srv.nlayers == MAX_LAYERS) {
        post_error(w->c, 1, WL_DISPLAY_ERROR_NO_MEMORY, "too many layer surfaces");
        return;
    }
    /* In layer order, a new one over those of its layer. */
    int at = srv.nlayers;
    while (at > 0 && srv.layers[at - 1]->layer > w->layer) {
        at--;
    }
    memmove(srv.layers + at + 1, srv.layers + at, (size_t)(srv.nlayers - at) * sizeof(struct window *));
    srv.layers[at] = w;
    srv.nlayers++;
    w->mapped = 1;
    layers_arrange();
    layer_place(w);
    damage(window_frame(w));
    if (w->keyboard) {
        window_focus(w);
    }
}

void layer_unmap(struct window *w) {
    for (int i = 0; i < srv.nlayers; i++) {
        if (srv.layers[i] == w) {
            memmove(srv.layers + i, srv.layers + i + 1, (size_t)(srv.nlayers - 1 - i) * sizeof(struct window *));
            srv.nlayers--;
            break;
        }
    }
    layers_arrange();
}

void layer_surface_request(struct client *c, struct object *o, uint op, struct args *a) {
    struct window *w = o->data;
    if (op == ZWLR_LAYER_SURFACE_V1_DESTROY) {
        object_destroy(c, o);
        return;
    }
    if (w == NULL) {
        return;
    }
    switch (op) {
    case ZWLR_LAYER_SURFACE_V1_SET_SIZE:
        if (a->u[0] > MAX_SIDE || a->u[1] > MAX_SIDE) {
            post_error(c, o->id, ZWLR_LAYER_SURFACE_V1_ERROR_INVALID_SIZE, "too big");
            return;
        }
        w->req_w = (int)a->u[0];
        w->req_h = (int)a->u[1];
        break;
    case ZWLR_LAYER_SURFACE_V1_SET_ANCHOR:
        if (a->u[0] > (ANCHOR_H | ANCHOR_V)) {
            post_error(c, o->id, ZWLR_LAYER_SURFACE_V1_ERROR_INVALID_ANCHOR, "invalid anchor");
            return;
        }
        w->anchor = a->u[0];
        break;
    case ZWLR_LAYER_SURFACE_V1_SET_EXCLUSIVE_ZONE: {
        int zone = (int)a->u[0];
        w->zone = zone < -1 ? -1 : zone > MAX_SIDE ? MAX_SIDE : zone;
        break;
    }
    case ZWLR_LAYER_SURFACE_V1_SET_MARGIN:
        for (int i = 0; i < 4; i++) {
            int m = (int)a->u[i];
            w->margin[i] = m < -MAX_SIDE ? -MAX_SIDE : m > MAX_SIDE ? MAX_SIDE : m;
        }
        break;
    case ZWLR_LAYER_SURFACE_V1_SET_KEYBOARD_INTERACTIVITY:
        if (a->u[0] > 1) {
            post_error(c, o->id, ZWLR_LAYER_SURFACE_V1_ERROR_INVALID_KEYBOARD_INTERACTIVITY, "invalid value");
            return;
        }
        w->keyboard = (int)a->u[0];
        break;
    case ZWLR_LAYER_SURFACE_V1_GET_POPUP:
        post_error(c, 1, WL_DISPLAY_ERROR_IMPLEMENTATION, "popups are not supported");
        break;
    case ZWLR_LAYER_SURFACE_V1_ACK_CONFIGURE:
        if (w->configure_serial == 0 || a->u[0] != w->configure_serial) {
            post_error(c, o->id, ZWLR_LAYER_SURFACE_V1_ERROR_INVALID_SURFACE_STATE, "a serial no configure had");
            return;
        }
        w->acked = 1;
        break;
    }
}

/* The checks and the configure of a layer surface's commit: 0, or -1 with
 * an error posted. */
int layer_commit(struct window *w, int new_buffer) {
    struct client *c = w->c;
    int both_h = (w->anchor & ANCHOR_H) == ANCHOR_H;
    int both_v = (w->anchor & ANCHOR_V) == ANCHOR_V;
    if ((w->req_w == 0 && !both_h) || (w->req_h == 0 && !both_v)) {
        post_error(c, w->layer_id, ZWLR_LAYER_SURFACE_V1_ERROR_INVALID_SIZE,
                   "a size of 0 needs the two edges it stretches between");
        return -1;
    }
    if (new_buffer && (w->configure_serial == 0 || !w->acked)) {
        post_error(c, w->layer_id, ZWLR_LAYER_SURFACE_V1_ERROR_INVALID_SURFACE_STATE,
                   "a buffer before the configure was acked");
        return -1;
    }
    layer_configure(w);
    return 0;
}

/* ---- Foreign toplevel management ---- */

static int is_toplevel_window(const struct window *w) {
    return w->layer < 0 && w->toplevel_id != 0 && w->mapped;
}

/* Sends handle h of window w its title. */
static void handle_title(struct client *c, struct object *h, struct window *w) {
    event_begin(c, h->id, ZWLR_FOREIGN_TOPLEVEL_HANDLE_V1_TITLE);
    gui_wire_string(&c->wire, w->title);
    event_end(c);
}

static void handle_state(struct client *c, struct object *h, struct window *w) {
    uint states[1];
    uint n = 0;
    if (srv.focus == w) {
        states[n++] = ZWLR_FOREIGN_TOPLEVEL_HANDLE_V1_STATE_ACTIVATED;
    }
    event_begin(c, h->id, ZWLR_FOREIGN_TOPLEVEL_HANDLE_V1_STATE);
    gui_wire_array(&c->wire, states, n * 4);
    event_end(c);
}

static void handle_done(struct client *c, struct object *h) {
    event_begin(c, h->id, ZWLR_FOREIGN_TOPLEVEL_HANDLE_V1_DONE);
    event_end(c);
}

/* A new handle of window w for the manager m of client c. */
static void announce_to(struct client *c, struct object *m, struct window *w) {
    struct object *h = server_object(c, &zwlr_foreign_toplevel_handle_v1_interface, m->version, w);
    if (h == NULL) {
        return;
    }
    uint id = h->id;
    event_begin(c, m->id, ZWLR_FOREIGN_TOPLEVEL_MANAGER_V1_TOPLEVEL);
    gui_wire_uint(&c->wire, id);
    event_end(c);
    h = object_find(c, id);
    handle_title(c, h, w);
    handle_state(c, h, w);
    handle_done(c, h);
}

void toplevels_bound(struct client *c, struct object *m) {
    uint id = m->id;
    for (int i = 0; i < srv.nstack; i++) {
        if (is_toplevel_window(srv.stack[i])) {
            /* object_new may move m: find it again each time. */
            struct object *mm = object_find(c, id);
            if (mm == NULL) {
                return;
            }
            announce_to(c, mm, srv.stack[i]);
        }
    }
}

void toplevel_announce(struct window *w) {
    struct client *c;
    struct object *m;
    /* Handles are added as this goes: each client's managers first. */
    for (int ci = 0; ci < MAX_CLIENTS; ci++) {
        c = srv.clients[ci];
        if (c == NULL || c->dead) {
            continue;
        }
        uint ids[8];
        int n = 0;
        for (int oi = 0; oi < c->nobjects && n < 8; oi++) {
            if (c->objects[oi].iface == &zwlr_foreign_toplevel_manager_v1_interface) {
                ids[n++] = c->objects[oi].id;
            }
        }
        for (int i = 0; i < n; i++) {
            m = object_find(c, ids[i]);
            if (m != NULL) {
                announce_to(c, m, w);
            }
        }
    }
}

/* What changed of window w (its title, whether it is activated) to every
 * handle of it. */
void toplevel_changed(struct window *w) {
    for (int ci = 0; ci < MAX_CLIENTS; ci++) {
        struct client *c = srv.clients[ci];
        if (c == NULL || c->dead) {
            continue;
        }
        for (int oi = 0; oi < c->nobjects; oi++) {
            struct object *h = &c->objects[oi];
            if (h->iface == &zwlr_foreign_toplevel_handle_v1_interface && h->data == w) {
                handle_title(c, h, w);
                handle_state(c, h, w);
                handle_done(c, h);
            }
        }
    }
}

/* Window w is gone: each handle of it is closed, and kept, inert, until its
 * client destroys it. */
void toplevel_closed(struct window *w) {
    for (int ci = 0; ci < MAX_CLIENTS; ci++) {
        struct client *c = srv.clients[ci];
        if (c == NULL) {
            continue;
        }
        for (int oi = 0; oi < c->nobjects; oi++) {
            struct object *h = &c->objects[oi];
            if (h->iface == &zwlr_foreign_toplevel_handle_v1_interface && h->data == w) {
                h->data = NULL;
                if (!c->dead) {
                    event_begin(c, h->id, ZWLR_FOREIGN_TOPLEVEL_HANDLE_V1_CLOSED);
                    event_end(c);
                }
            }
        }
    }
}

void toplevel_manager_request(struct client *c, struct object *o, uint op, struct args *a) {
    (void)a;
    if (op == ZWLR_FOREIGN_TOPLEVEL_MANAGER_V1_STOP) {
        /* No more handles: finished (its destructor), and it is gone. */
        event_begin(c, o->id, ZWLR_FOREIGN_TOPLEVEL_MANAGER_V1_FINISHED);
        event_end(c);
        object_destroy(c, o);
    }
}

void toplevel_handle_request(struct client *c, struct object *o, uint op, struct args *a) {
    (void)a;
    struct window *w = o->data;
    if (op == ZWLR_FOREIGN_TOPLEVEL_HANDLE_V1_DESTROY) {
        object_destroy(c, o);
        return;
    }
    if (w == NULL || !w->mapped) {
        return; /* closed: requests do nothing */
    }
    if (op == ZWLR_FOREIGN_TOPLEVEL_HANDLE_V1_ACTIVATE) {
        window_focus(w);
    } else if (op == ZWLR_FOREIGN_TOPLEVEL_HANDLE_V1_CLOSE) {
        window_close(w);
    }
    /* Maximized, minimized, fullscreen and the rectangle: taken, and the
     * window stays as it is. */
}

/* ---- The clipboard ---- */

void data_manager_request(struct client *c, struct object *o, uint op, struct args *a) {
    uint id = a->u[0];
    if (op == WL_DATA_DEVICE_MANAGER_CREATE_DATA_SOURCE) {
        struct source *s = malloc(sizeof(struct source));
        if (s == NULL) {
            post_error(c, 1, WL_DISPLAY_ERROR_NO_MEMORY, "no memory for a data source");
            return;
        }
        memset(s, 0, sizeof(struct source));
        s->c = c;
        s->id = id;
        if (object_new(c, id, &wl_data_source_interface, o->version, s) == NULL) {
            free(s);
        }
    } else if (op == WL_DATA_DEVICE_MANAGER_GET_DATA_DEVICE) {
        if (object_new(c, id, &wl_data_device_interface, o->version, NULL) != NULL && srv.focus != NULL &&
            srv.focus->c == c) {
            selection_offer(c);
        }
    }
}

void data_source_request(struct client *c, struct object *o, uint op, struct args *a) {
    struct source *s = o->data;
    if (op == WL_DATA_SOURCE_DESTROY) {
        object_destroy(c, o);
        return;
    }
    if (op == WL_DATA_SOURCE_OFFER && s != NULL) {
        if (s->used) {
            post_error(c, o->id, WL_DATA_SOURCE_ERROR_INVALID_SOURCE, "offer after the source was used");
            return;
        }
        if (s->nmimes < MAX_MIMES && strlen(a->s[0]) < MIME_MAX) {
            snprintf(s->mimes[s->nmimes++], MIME_MAX, "%s", a->s[0]);
        }
    }
}

/* Every offer of the selection made stale (the selection changed). */
static void offers_stale(void) {
    for (int ci = 0; ci < MAX_CLIENTS; ci++) {
        struct client *c = srv.clients[ci];
        if (c == NULL) {
            continue;
        }
        for (int oi = 0; oi < c->nobjects; oi++) {
            if (c->objects[oi].iface == &wl_data_offer_interface) {
                c->objects[oi].data = NULL;
            }
        }
    }
}

/* The selection, or none, offered to client c's data devices. */
void selection_offer(struct client *c) {
    if (c == NULL || c->dead) {
        return;
    }
    uint devices[8];
    int n = 0;
    for (int oi = 0; oi < c->nobjects && n < 8; oi++) {
        if (c->objects[oi].iface == &wl_data_device_interface) {
            devices[n++] = c->objects[oi].id;
        }
    }
    struct source *s = srv.selection;
    for (int i = 0; i < n; i++) {
        uint offer = 0;
        if (s != NULL) {
            struct object *of = server_object(c, &wl_data_offer_interface, 1, s);
            if (of == NULL) {
                return;
            }
            offer = of->id;
            event_begin(c, devices[i], WL_DATA_DEVICE_DATA_OFFER);
            gui_wire_uint(&c->wire, offer);
            event_end(c);
            for (int m = 0; m < s->nmimes; m++) {
                event_begin(c, offer, WL_DATA_OFFER_OFFER);
                gui_wire_string(&c->wire, s->mimes[m]);
                event_end(c);
            }
        }
        event_begin(c, devices[i], WL_DATA_DEVICE_SELECTION);
        gui_wire_uint(&c->wire, offer);
        event_end(c);
    }
}

/* The selection is now s (or none): the one before is cancelled, and the
 * client with the keyboard is offered it. */
static void set_selection(struct source *s) {
    struct source *old = srv.selection;
    if (old == s) {
        return;
    }
    srv.selection = s;
    offers_stale();
    if (old != NULL) {
        event_begin(old->c, old->id, WL_DATA_SOURCE_CANCELLED);
        event_end(old->c);
    }
    if (srv.focus != NULL) {
        selection_offer(srv.focus->c);
    }
}

void data_device_request(struct client *c, struct object *o, uint op, struct args *a) {
    switch (op) {
    case WL_DATA_DEVICE_START_DRAG:
        break; /* no dragging: the drag never starts */
    case WL_DATA_DEVICE_SET_SELECTION: {
        /* From the client with the keyboard, as Weston has it: the
         * serial is its input's. */
        struct source *s = a->o[0] != NULL ? a->o[0]->data : NULL;
        if (s != NULL && s->used) {
            post_error(c, o->id, WL_DATA_DEVICE_ERROR_USED_SOURCE, "a source used already");
            return;
        }
        if (srv.focus == NULL || srv.focus->c != c) {
            if (s != NULL) {
                event_begin(c, s->id, WL_DATA_SOURCE_CANCELLED);
                event_end(c);
            }
            return;
        }
        if (s != NULL) {
            s->used = 1;
        }
        set_selection(s);
        break;
    }
    case WL_DATA_DEVICE_RELEASE:
        object_destroy(c, o);
        break;
    }
}

void data_offer_request(struct client *c, struct object *o, uint op, struct args *a) {
    struct source *s = o->data;
    switch (op) {
    case WL_DATA_OFFER_ACCEPT:
        break;
    case WL_DATA_OFFER_RECEIVE: {
        int fd = a->fd[1];
        if (s == NULL || s->c->dead || s->c->nheld == MAX_HELD_FDS) {
            break; /* stale, or its source busy: the descriptor closes, an empty read */
        }
        struct client *sc = s->c;
        event_begin(sc, s->id, WL_DATA_SOURCE_SEND);
        gui_wire_string(&sc->wire, a->s[0]);
        gui_wire_fd(&sc->wire, fd);
        event_end(sc);
        /* Kept until it is sent: client_flush closes it. */
        sc->held_fds[sc->nheld++] = fd;
        a->fd[1] = -1;
        if (client_flush(sc) < 0) {
            sc->dead = 1;
        }
        break;
    }
    case WL_DATA_OFFER_DESTROY:
        object_destroy(c, o);
        break;
    }
}

/* What a layer surface, a source or a handle frees when it goes: 1 if o
 * was one. */
int shell_object_free(struct client *c, struct object *o) {
    (void)c;
    if (is_layer_object(o)) {
        struct window *w = o->data;
        window_free(w);
        return 1;
    }
    if (o->iface == &wl_data_source_interface) {
        struct source *s = o->data;
        if (srv.selection == s) {
            srv.selection = NULL;
            offers_stale();
            if (srv.focus != NULL && srv.focus->c != c) {
                selection_offer(srv.focus->c);
            }
        }
        free(s);
        return 1;
    }
    return 0;
}
