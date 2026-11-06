/* xdg-shell's requests (protocols/xdg-shell.xml), version 1: xdg_wm_base,
 * xdg_surface and xdg_toplevel, the windows; xdg_positioner is taken and
 * not used, and popups are not had. A window's size is its client's:
 * the server sends one configure, the first, of 0 by 0 (the client
 * chooses), and the window is shown once a buffer comes after its ack. */
#include "compositor.h"

void wm_base_request(struct client *c, struct object *o, uint op, struct args *a) {
    switch (op) {
    case XDG_WM_BASE_DESTROY:
        object_destroy(c, o);
        break;
    case XDG_WM_BASE_CREATE_POSITIONER:
        object_new(c, a->u[0], &xdg_positioner_interface, o->version, NULL);
        break;
    case XDG_WM_BASE_GET_XDG_SURFACE: {
        uint id = a->u[0];
        struct surface *s = a->o[1]->data;
        if (s->window != NULL) {
            post_error(c, o->id, XDG_WM_BASE_ERROR_ROLE, "the surface has a role already");
            return;
        }
        if (s->has_content || (s->attached && s->buffer != NULL)) {
            post_error(c, o->id, XDG_WM_BASE_ERROR_INVALID_SURFACE_STATE, "the surface has a buffer already");
            return;
        }
        struct window *w = malloc(sizeof(struct window));
        if (w == NULL) {
            post_error(c, 1, WL_DISPLAY_ERROR_NO_MEMORY, "no memory for a window");
            return;
        }
        memset(w, 0, sizeof(struct window));
        w->c = c;
        w->surface = s;
        w->xdg_id = id;
        if (object_new(c, id, &xdg_surface_interface, o->version, w) == NULL) {
            free(w);
            return;
        }
        s->window = w;
        break;
    }
    case XDG_WM_BASE_PONG:
        break; /* the server never pings */
    }
}

void positioner_request(struct client *c, struct object *o, uint op, struct args *a) {
    (void)a;
    if (op == XDG_POSITIONER_DESTROY) {
        object_destroy(c, o);
    }
}

void xdg_surface_request(struct client *c, struct object *o, uint op, struct args *a) {
    struct window *w = o->data;
    switch (op) {
    case XDG_SURFACE_DESTROY:
        object_destroy(c, o);
        break;
    case XDG_SURFACE_GET_TOPLEVEL: {
        uint id = a->u[0];
        if (w->toplevel_id != 0 || w->configure_serial != 0) {
            post_error(c, o->id, XDG_SURFACE_ERROR_ALREADY_CONSTRUCTED, "the xdg_surface has a role already");
            return;
        }
        if (w->surface == NULL) {
            post_error(c, o->id, XDG_SURFACE_ERROR_DEFUNCT_ROLE_OBJECT, "the surface is gone");
            return;
        }
        if (object_new(c, id, &xdg_toplevel_interface, o->version, w) != NULL) {
            w->toplevel_id = id;
            snprintf(w->title, sizeof(w->title), "client %d", c->number);
        }
        break;
    }
    case XDG_SURFACE_GET_POPUP:
        post_error(c, o->id, XDG_WM_BASE_ERROR_INVALID_POPUP_PARENT, "popups are not supported");
        break;
    case XDG_SURFACE_SET_WINDOW_GEOMETRY:
        break; /* the window is the whole surface */
    case XDG_SURFACE_ACK_CONFIGURE:
        if (w->configure_serial == 0 || a->u[0] != w->configure_serial) {
            post_error(c, o->id, XDG_SURFACE_ERROR_INVALID_SERIAL, "a serial no configure had");
            return;
        }
        w->acked = 1;
        break;
    }
}

/* The title, cut to TITLE_MAX - 1 bytes at a character's start. */
static void set_title(struct window *w, const char *s) {
    int n = 0;
    while (s[n] != 0 && n < TITLE_MAX - 1) {
        n++;
    }
    while (n > 0 && s[n] != 0 && ((uchar)s[n] & 0xc0) == 0x80) {
        n--; /* not in the middle of a UTF-8 sequence */
    }
    memcpy(w->title, s, (size_t)n);
    w->title[n] = 0;
    if (w->mapped) {
        damage(window_title(w));
    }
}

void toplevel_request(struct client *c, struct object *o, uint op, struct args *a) {
    struct window *w = o->data;
    if (op == XDG_TOPLEVEL_DESTROY) {
        object_destroy(c, o);
        return;
    }
    if (w == NULL) {
        return; /* its xdg_surface is gone: nothing to do but destroy it */
    }
    switch (op) {
    case XDG_TOPLEVEL_SET_TITLE:
        set_title(w, a->s[0]);
        break;
    case XDG_TOPLEVEL_MOVE:
        /* With a button held: the window follows the pointer until it is
         * let go (a client that draws its own title bar). */
        if (srv.buttons != 0 && w->mapped) {
            window_move_start(w);
        }
        break;
    default:
        /* set_parent, set_app_id, show_window_menu, resize, the sizes,
         * maximized, fullscreen, minimized: taken, and the window stays as
         * it is, which a client must expect of any of them. */
        break;
    }
}

void window_free(struct window *w) {
    window_unmap(w);
    if (w->surface != NULL) {
        w->surface->window = NULL;
    }
    free(w);
}
