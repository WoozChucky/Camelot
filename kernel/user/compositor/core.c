/* The core protocol's requests (protocols/wayland.xml), the subset the
 * server has: wl_display, wl_registry and wl_callback; wl_compositor,
 * wl_surface and wl_region; wl_shm, wl_shm_pool and wl_buffer (XRGB8888
 * and ARGB8888); wl_seat with wl_pointer and wl_keyboard (and a wl_touch
 * that never touches); wl_output. client.c has checked each request's
 * arguments against its signature: what is checked here is what the
 * protocol says about their values. */
#include "compositor.h"

/* The globals, as wl_registry announces them: their names are their
 * places here, from 1. The versions are those the server implements. */
static const struct {
    const struct wl_interface *iface;
    int version;
} globals[] = {
    {&wl_compositor_interface, 4},
    {&wl_shm_interface, 1},
    {&wl_seat_interface, 1},
    {&wl_output_interface, 2},
    {&xdg_wm_base_interface, 1},
};

#define NGLOBALS ((int)(sizeof(globals) / sizeof(globals[0])))

/* ---- wl_display, wl_registry ---- */

void display_request(struct client *c, struct object *o, uint op, struct args *a) {
    if (op == WL_DISPLAY_SYNC) {
        uint id = a->u[0];
        if (object_new(c, id, &wl_callback_interface, 1, NULL) == NULL) {
            return;
        }
        event_begin(c, id, WL_CALLBACK_DONE);
        gui_wire_uint(&c->wire, next_serial());
        event_end(c);
        object_destroy(c, object_find(c, id));
    } else if (op == WL_DISPLAY_GET_REGISTRY) {
        uint id = a->u[0];
        if (object_new(c, id, &wl_registry_interface, 1, NULL) == NULL) {
            return;
        }
        for (int i = 0; i < NGLOBALS; i++) {
            event_begin(c, id, WL_REGISTRY_GLOBAL);
            gui_wire_uint(&c->wire, (uint)(i + 1));
            gui_wire_string(&c->wire, globals[i].iface->name);
            gui_wire_uint(&c->wire, (uint)globals[i].version);
            event_end(c);
        }
    }
    (void)o;
}

/* What a global sends a client that binds it. */
static void bound(struct client *c, struct object *o) {
    if (o->iface == &wl_shm_interface) {
        event_begin(c, o->id, WL_SHM_FORMAT);
        gui_wire_uint(&c->wire, WL_SHM_FORMAT_ARGB8888);
        event_end(c);
        event_begin(c, o->id, WL_SHM_FORMAT);
        gui_wire_uint(&c->wire, WL_SHM_FORMAT_XRGB8888);
        event_end(c);
    } else if (o->iface == &wl_seat_interface) {
        event_begin(c, o->id, WL_SEAT_CAPABILITIES);
        gui_wire_uint(&c->wire, WL_SEAT_CAPABILITY_POINTER | WL_SEAT_CAPABILITY_KEYBOARD);
        event_end(c);
    } else if (o->iface == &wl_output_interface) {
        event_begin(c, o->id, WL_OUTPUT_GEOMETRY);
        gui_wire_int(&c->wire, 0);
        gui_wire_int(&c->wire, 0);
        gui_wire_int(&c->wire, 0); /* millimetres: unknown */
        gui_wire_int(&c->wire, 0);
        gui_wire_int(&c->wire, WL_OUTPUT_SUBPIXEL_UNKNOWN);
        gui_wire_string(&c->wire, "Camelot");
        gui_wire_string(&c->wire, "Grail display");
        gui_wire_int(&c->wire, WL_OUTPUT_TRANSFORM_NORMAL);
        event_end(c);
        event_begin(c, o->id, WL_OUTPUT_MODE);
        gui_wire_uint(&c->wire, WL_OUTPUT_MODE_CURRENT | WL_OUTPUT_MODE_PREFERRED);
        gui_wire_int(&c->wire, srv.width);
        gui_wire_int(&c->wire, srv.height);
        gui_wire_int(&c->wire, 60000);
        event_end(c);
        if (o->version >= 2) {
            event_begin(c, o->id, WL_OUTPUT_SCALE);
            gui_wire_int(&c->wire, 1);
            event_end(c);
            event_begin(c, o->id, WL_OUTPUT_DONE);
            event_end(c);
        }
    }
}

void registry_request(struct client *c, struct object *o, uint op, struct args *a) {
    char what[96];
    (void)op; /* bind, its one request */
    uint name = a->u[0];
    const char *iface = a->s[1];
    uint version = a->u[2];
    uint id = a->u[3];
    if (name < 1 || name > (uint)NGLOBALS || strcmp(iface, globals[name - 1].iface->name) != 0) {
        snprintf(what, sizeof(what), "invalid global %s (%u)", iface, name);
        post_error(c, o->id, WL_DISPLAY_ERROR_INVALID_OBJECT, what);
        return;
    }
    int have = globals[name - 1].version;
    if (version < 1 || version > (uint)have) {
        snprintf(what, sizeof(what), "invalid version for global %s (%u): have %d, wanted %u", iface, name, have,
                 version);
        post_error(c, o->id, WL_DISPLAY_ERROR_INVALID_OBJECT, what);
        return;
    }
    struct object *b = object_new(c, id, globals[name - 1].iface, (int)version, NULL);
    if (b != NULL) {
        bound(c, b);
    }
}

/* ---- wl_compositor, wl_region ---- */

void compositor_request(struct client *c, struct object *o, uint op, struct args *a) {
    uint id = a->u[0];
    if (op == WL_COMPOSITOR_CREATE_SURFACE) {
        struct surface *s = malloc(sizeof(struct surface));
        if (s == NULL) {
            post_error(c, 1, WL_DISPLAY_ERROR_NO_MEMORY, "no memory for a surface");
            return;
        }
        memset(s, 0, sizeof(struct surface));
        s->c = c;
        s->id = id;
        if (object_new(c, id, &wl_surface_interface, o->version, s) == NULL) {
            free(s);
        }
    } else if (op == WL_COMPOSITOR_CREATE_REGION) {
        /* Regions are taken and not used: the whole surface is opaque
         * as its pixels say, and takes input everywhere. */
        object_new(c, id, &wl_region_interface, o->version, NULL);
    }
}

void region_request(struct client *c, struct object *o, uint op, struct args *a) {
    (void)a;
    if (op == WL_REGION_DESTROY) {
        object_destroy(c, o);
    }
}

/* ---- wl_surface ---- */

/* A rectangle a client damaged, inside the largest surface there is. */
static void add_damage(struct surface *s, struct args *a) {
    long x = (int)a->u[0];
    long y = (int)a->u[1];
    long w = (int)a->u[2];
    long h = (int)a->u[3];
    if (w <= 0 || h <= 0) {
        return;
    }
    long x0 = x < 0 ? 0 : x;
    long y0 = y < 0 ? 0 : y;
    long x1 = x + w > MAX_SIDE ? MAX_SIDE : x + w;
    long y1 = y + h > MAX_SIDE ? MAX_SIDE : y + h;
    if (x1 > x0 && y1 > y0) {
        gui_damage_add(&s->damage, gui_rect((int)x0, (int)y0, (int)(x1 - x0), (int)(y1 - y0)));
    }
}

/* The buffer's damaged pixels copied into the surface's content (all of
 * them if the size changed): 0, or -1 with no memory for them. */
static int copy_buffer(struct surface *s, struct buffer *b) {
    struct gui_damage *d = &s->damage;
    struct gui_damage all;
    if (!s->has_content || s->content.width != b->width || s->content.height != b->height) {
        gui_buffer_free(&s->content);
        s->has_content = 0;
        if (gui_buffer_alloc(&s->content, b->width, b->height) < 0) {
            return -1;
        }
        s->has_content = 1;
        gui_damage_clear(&all);
        gui_damage_add(&all, gui_rect(0, 0, b->width, b->height));
        d = &all;
    }
    struct gui_buffer src;
    gui_buffer_init(&src, (uint *)(b->pool->mem + b->offset), b->width, b->height, b->stride / 4);
    struct gui_rect whole = gui_rect(0, 0, b->width, b->height);
    for (int i = 0; i < d->count; i++) {
        struct gui_rect r = gui_rect_intersect(d->rects[i], whole);
        if (!gui_rect_empty(r)) {
            gui_blit(&s->content, r.x, r.y, &src, r);
        }
    }
    s->argb = b->format == WL_SHM_FORMAT_ARGB8888;
    return 0;
}

static void send_configure(struct window *w) {
    struct client *c = w->c;
    event_begin(c, w->toplevel_id, XDG_TOPLEVEL_CONFIGURE);
    gui_wire_int(&c->wire, 0); /* the client chooses its size */
    gui_wire_int(&c->wire, 0);
    gui_wire_array(&c->wire, NULL, 0);
    event_end(c);
    w->configure_serial = next_serial();
    w->acked = 0;
    event_begin(c, w->xdg_id, XDG_SURFACE_CONFIGURE);
    gui_wire_uint(&c->wire, w->configure_serial);
    event_end(c);
}

static void commit(struct client *c, struct surface *s) {
    struct window *w = s->window;
    int new_buffer = s->attached && s->buffer != NULL;
    if (w != NULL && w->toplevel_id == 0) {
        post_error(c, w->xdg_id, XDG_SURFACE_ERROR_NOT_CONSTRUCTED, "a commit before the xdg_surface has a role");
        return;
    }
    if (w != NULL && new_buffer && (w->configure_serial == 0 || !w->acked)) {
        post_error(c, w->xdg_id, XDG_SURFACE_ERROR_UNCONFIGURED_BUFFER, "a buffer before the configure was acked");
        return;
    }
    if (w != NULL && w->configure_serial == 0) {
        send_configure(w); /* the first commit: the window's first configure */
    }
    struct gui_rect before = w != NULL && w->mapped ? window_frame(w) : gui_rect(0, 0, 0, 0);
    if (new_buffer) {
        if (copy_buffer(s, s->buffer) < 0) {
            post_error(c, 1, WL_DISPLAY_ERROR_NO_MEMORY, "no memory for a surface's pixels");
            return;
        }
        /* Copied: the client may draw into it again at once. */
        event_begin(c, s->buffer_id, WL_BUFFER_RELEASE);
        event_end(c);
    } else if (s->attached) {
        gui_buffer_free(&s->content);
        s->has_content = 0;
    }
    if (w != NULL && w->mapped) {
        if (!s->has_content) {
            window_unmap(w);
        } else {
            struct gui_rect after = window_frame(w);
            if (after.w != before.w || after.h != before.h) {
                damage(before);
                damage(after);
            } else {
                struct gui_rect content = window_content(w);
                for (int i = 0; i < s->damage.count; i++) {
                    struct gui_rect r = s->damage.rects[i];
                    damage(gui_rect_intersect(gui_rect(r.x + w->x, r.y + w->y, r.w, r.h), content));
                }
            }
        }
    } else if (w != NULL && s->has_content) {
        window_map(w);
    }
    /* The frame callbacks wait for the next frame drawn; one the last
     * frames haven't taken yet is done at once. */
    for (int i = 0; i < s->nframes; i++) {
        if (s->ncommitted == MAX_FRAMES) {
            struct object *cb = object_find(c, s->committed[0]);
            if (cb != NULL) {
                event_begin(c, cb->id, WL_CALLBACK_DONE);
                gui_wire_uint(&c->wire, now_ms());
                event_end(c);
                object_destroy(c, cb);
            }
            s->ncommitted--;
            memmove(s->committed, s->committed + 1, (size_t)s->ncommitted * sizeof(uint));
        }
        s->committed[s->ncommitted++] = s->frames[i];
    }
    if (s->nframes > 0) {
        srv.repaint = 1;
    }
    s->nframes = 0;
    s->attached = 0;
    s->buffer = NULL;
    s->buffer_id = 0;
    gui_damage_clear(&s->damage);
}

void surface_request(struct client *c, struct object *o, uint op, struct args *a) {
    struct surface *s = o->data;
    switch (op) {
    case WL_SURFACE_DESTROY: {
        /* Its frame callbacks go with it, never done, as Weston's do. */
        uint ids[MAX_FRAMES * 3];
        int n = 0;
        for (int i = 0; i < s->nframes; i++) {
            ids[n++] = s->frames[i];
        }
        for (int i = 0; i < s->ncommitted; i++) {
            ids[n++] = s->committed[i];
        }
        for (int i = 0; i < s->nshown; i++) {
            ids[n++] = s->shown[i];
        }
        object_destroy(c, o);
        for (int i = 0; i < n; i++) {
            struct object *cb = object_find(c, ids[i]);
            if (cb != NULL) {
                object_destroy(c, cb);
            }
        }
        break;
    }
    case WL_SURFACE_ATTACH:
        s->attached = 1;
        s->buffer = a->o[0] != NULL ? a->o[0]->data : NULL;
        s->buffer_id = a->u[0];
        /* The offset (a->u[1], a->u[2]) moves nothing: the server places
         * windows. */
        break;
    case WL_SURFACE_DAMAGE:
    case WL_SURFACE_DAMAGE_BUFFER:
        /* Scale 1, no transform: the surface's coordinates are the
         * buffer's. */
        add_damage(s, a);
        break;
    case WL_SURFACE_FRAME: {
        if (s->nframes == MAX_FRAMES) {
            post_error(c, 1, WL_DISPLAY_ERROR_NO_MEMORY, "too many frame callbacks");
            return;
        }
        uint id = a->u[0];
        if (object_new(c, id, &wl_callback_interface, 1, NULL) != NULL) {
            s->frames[s->nframes++] = id;
        }
        break;
    }
    case WL_SURFACE_SET_OPAQUE_REGION:
    case WL_SURFACE_SET_INPUT_REGION:
        break; /* regions aren't used */
    case WL_SURFACE_COMMIT:
        commit(c, s);
        break;
    case WL_SURFACE_SET_BUFFER_TRANSFORM:
        if ((int)a->u[0] < WL_OUTPUT_TRANSFORM_NORMAL || (int)a->u[0] > WL_OUTPUT_TRANSFORM_FLIPPED_270) {
            post_error(c, o->id, WL_SURFACE_ERROR_INVALID_TRANSFORM, "invalid transform");
        }
        break; /* taken, and the buffer shown as it is */
    case WL_SURFACE_SET_BUFFER_SCALE:
        if ((int)a->u[0] < 1) {
            post_error(c, o->id, WL_SURFACE_ERROR_INVALID_SCALE, "invalid scale");
        }
        break; /* taken, and the buffer shown at scale 1 */
    }
}

void surface_free(struct surface *s) {
    wm_forget_surface(s);
    if (s->window != NULL) {
        window_unmap(s->window);
        s->window->surface = NULL;
    }
    gui_buffer_free(&s->content);
    free(s);
}

/* ---- wl_shm, wl_shm_pool, wl_buffer ---- */

static void pool_unref(struct pool *p) {
    if (--p->refs == 0) {
        munmap(p->mem, p->size);
        close(p->fd);
        free(p);
    }
}

/* Maps `size` bytes of memfd `fd`, which must have them: the pages are
 * touched now (MAP_POPULATE), so that a client shrinking its memfd later
 * can't make the server touch a page that isn't there. NULL if it can't. */
static uchar *map_pool(int fd, int size) {
    struct stat st;
    if (fstat(fd, &st) < 0 || st.size < size) {
        return NULL;
    }
    void *mem = mmap(NULL, (size_t)size, PROT_READ, MAP_SHARED | MAP_POPULATE, fd, 0);
    return (long)mem < 0 ? NULL : mem;
}

void shm_request(struct client *c, struct object *o, uint op, struct args *a) {
    (void)op; /* create_pool, version 1's one request */
    uint id = a->u[0];
    int fd = a->fd[1];
    int size = (int)a->u[2];
    if (size <= 0 || size > MAX_POOL) {
        post_error(c, o->id, WL_SHM_ERROR_INVALID_STRIDE, "invalid pool size");
        return;
    }
    /* The pool keeps its descriptor (for resize): leave two free, to take
     * a connection and a descriptor that comes with a message. */
    if (fd >= NOFILE - 2) {
        post_error(c, 1, WL_DISPLAY_ERROR_NO_MEMORY, "the server is out of descriptors");
        return;
    }
    uchar *mem = map_pool(fd, size);
    if (mem == NULL) {
        post_error(c, o->id, WL_SHM_ERROR_INVALID_FD, "can't map the pool's memory");
        return;
    }
    struct pool *p = malloc(sizeof(struct pool));
    if (p == NULL) {
        munmap(mem, (size_t)size);
        post_error(c, 1, WL_DISPLAY_ERROR_NO_MEMORY, "no memory for a pool");
        return;
    }
    p->fd = fd;
    p->mem = mem;
    p->size = (size_t)size;
    p->refs = 1;
    a->fd[1] = -1;
    if (object_new(c, id, &wl_shm_pool_interface, o->version, p) == NULL) {
        pool_unref(p);
    }
}

void pool_request(struct client *c, struct object *o, uint op, struct args *a) {
    struct pool *p = o->data;
    if (op == WL_SHM_POOL_DESTROY) {
        object_destroy(c, o); /* its buffers keep the memory */
    } else if (op == WL_SHM_POOL_RESIZE) {
        int size = (int)a->u[0];
        if (size < (int)p->size || size > MAX_POOL) {
            post_error(c, o->id, WL_SHM_ERROR_INVALID_STRIDE, "a pool can only grow");
            return;
        }
        uchar *mem = map_pool(p->fd, size);
        if (mem == NULL) {
            post_error(c, o->id, WL_SHM_ERROR_INVALID_FD, "can't map the pool's memory");
            return;
        }
        munmap(p->mem, p->size);
        p->mem = mem;
        p->size = (size_t)size;
    } else if (op == WL_SHM_POOL_CREATE_BUFFER) {
        uint id = a->u[0];
        int offset = (int)a->u[1];
        int width = (int)a->u[2];
        int height = (int)a->u[3];
        int stride = (int)a->u[4];
        uint format = a->u[5];
        if (format != WL_SHM_FORMAT_ARGB8888 && format != WL_SHM_FORMAT_XRGB8888) {
            post_error(c, o->id, WL_SHM_ERROR_INVALID_FORMAT, "a format the server hasn't");
            return;
        }
        if (width <= 0 || height <= 0 || width > MAX_SIDE || height > MAX_SIDE || offset < 0 || offset % 4 != 0 ||
            stride < width * 4 || stride % 4 != 0) {
            post_error(c, o->id, WL_SHM_ERROR_INVALID_STRIDE, "invalid width, height, stride or offset");
            return;
        }
        unsigned long end = (unsigned long)offset + (unsigned long)stride * (unsigned long)(height - 1) +
                            (unsigned long)width * 4;
        if (end > p->size) {
            post_error(c, o->id, WL_SHM_ERROR_INVALID_STRIDE, "the buffer is past the pool's end");
            return;
        }
        struct buffer *b = malloc(sizeof(struct buffer));
        if (b == NULL) {
            post_error(c, 1, WL_DISPLAY_ERROR_NO_MEMORY, "no memory for a buffer");
            return;
        }
        b->pool = p;
        b->offset = offset;
        b->width = width;
        b->height = height;
        b->stride = stride;
        b->format = format;
        p->refs++;
        if (object_new(c, id, &wl_buffer_interface, 1, b) == NULL) {
            pool_unref(p);
            free(b);
        }
    }
}

void buffer_request(struct client *c, struct object *o, uint op, struct args *a) {
    (void)op; /* destroy */
    (void)a;
    object_destroy(c, o);
}

/* ---- wl_seat, wl_pointer, wl_keyboard, wl_touch, wl_output ---- */

void seat_request(struct client *c, struct object *o, uint op, struct args *a) {
    uint id = a->u[0];
    if (op == WL_SEAT_GET_POINTER) {
        struct object *p = object_new(c, id, &wl_pointer_interface, o->version, NULL);
        if (p != NULL) {
            pointer_bound(c, p);
        }
    } else if (op == WL_SEAT_GET_KEYBOARD) {
        struct object *k = object_new(c, id, &wl_keyboard_interface, o->version, NULL);
        if (k == NULL) {
            return;
        }
        /* No keymap: the client maps Linux's key codes itself (gui.h,
         * "The keyboard"). The event still carries a descriptor. */
        event_begin(c, id, WL_KEYBOARD_KEYMAP);
        gui_wire_uint(&c->wire, WL_KEYBOARD_KEYMAP_FORMAT_NO_KEYMAP);
        gui_wire_fd(&c->wire, srv.keymap_fd);
        gui_wire_uint(&c->wire, 0);
        event_end(c);
        keyboard_bound(c, object_find(c, id));
    } else if (op == WL_SEAT_GET_TOUCH) {
        object_new(c, id, &wl_touch_interface, o->version, NULL);
    }
}

void pointer_request(struct client *c, struct object *o, uint op, struct args *a) {
    /* set_cursor: the server draws its own arrow. */
    (void)c;
    (void)o;
    (void)op;
    (void)a;
}

void keyboard_request(struct client *c, struct object *o, uint op, struct args *a) {
    /* Version 1 has no requests: the opcode was refused. */
    (void)c;
    (void)o;
    (void)op;
    (void)a;
}

void touch_request(struct client *c, struct object *o, uint op, struct args *a) {
    (void)c;
    (void)o;
    (void)op;
    (void)a;
}

void output_request(struct client *c, struct object *o, uint op, struct args *a) {
    (void)c;
    (void)o;
    (void)op;
    (void)a;
}

/* ---- Freeing, and frame callbacks ---- */

void object_free(struct client *c, struct object *o) {
    if (o->iface == &wl_surface_interface) {
        surface_free(o->data);
    } else if (o->iface == &wl_shm_pool_interface) {
        pool_unref(o->data);
    } else if (o->iface == &wl_buffer_interface) {
        struct buffer *b = o->data;
        /* A surface it is attached to, not yet committed, gets none. */
        for (int i = 0; i < c->nobjects; i++) {
            if (c->objects[i].iface == &wl_surface_interface && c->objects[i].data != NULL) {
                struct surface *s = c->objects[i].data;
                if (s->buffer == b) {
                    s->buffer = NULL;
                }
            }
        }
        pool_unref(b->pool);
        free(b);
    } else if (o->iface == &xdg_surface_interface || o->iface == &xdg_toplevel_interface) {
        struct window *w = o->data;
        if (o->iface == &xdg_toplevel_interface) {
            w->toplevel_id = 0;
            window_unmap(w);
            if (w->xdg_id == 0) {
                window_free(w);
            }
        } else {
            w->xdg_id = 0;
            if (w->toplevel_id != 0) {
                struct object *t = object_find(c, w->toplevel_id);
                if (t != NULL) {
                    t->data = NULL;
                }
                w->toplevel_id = 0;
            }
            window_free(w);
        }
    }
    o->data = NULL;
}

/* The client's surfaces, collected first: destroying a callback moves
 * the objects about. */
static int surfaces(struct client *c, struct surface **out) {
    int n = 0;
    for (int i = 0; i < c->nobjects; i++) {
        if (c->objects[i].iface == &wl_surface_interface && c->objects[i].data != NULL) {
            out[n++] = c->objects[i].data;
        }
    }
    return n;
}

void frames_shown(struct client *c) {
    struct surface *list[MAX_OBJECTS];
    int n = surfaces(c, list);
    for (int i = 0; i < n; i++) {
        struct surface *s = list[i];
        for (int j = 0; j < s->ncommitted && s->nshown < MAX_FRAMES; j++) {
            s->shown[s->nshown++] = s->committed[j];
        }
        s->ncommitted = 0;
    }
}

void frames_done(struct client *c, uint time) {
    struct surface *list[MAX_OBJECTS];
    int n = surfaces(c, list);
    for (int i = 0; i < n; i++) {
        struct surface *s = list[i];
        for (int j = 0; j < s->nshown; j++) {
            struct object *cb = object_find(c, s->shown[j]);
            if (cb != NULL) {
                event_begin(c, cb->id, WL_CALLBACK_DONE);
                gui_wire_uint(&c->wire, time);
                event_end(c);
                object_destroy(c, cb);
            }
        }
        s->nshown = 0;
    }
}
