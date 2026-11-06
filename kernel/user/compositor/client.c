/* A client's connection: its objects, and each message it sends checked
 * against the protocol's tables (gui/protocol.c, generated from the XML)
 * before the interface's handler sees it, as libwayland's
 * wl_closure_demarshal and wl_closure_lookup_objects check them:
 *
 * - the object exists, and the opcode is one of its interface's requests,
 *   of a version the object has;
 * - each argument is there, in the message's bytes: a string ends with
 *   its NUL and has no other, an array fits, a descriptor came with it;
 * - an object argument names an object of the client's, of the interface
 *   the request says, or is null where the request allows it;
 * - a new id is the client's to make (1 to 0xfeffffff) and free;
 * - nothing is left over.
 *
 * Anything else is a protocol error: wl_display.error, naming the object
 * and saying what was wrong, and the client is disconnected. */
#include "compositor.h"

/* Each interface's handler. */
static const struct {
    const struct wl_interface *iface;
    void (*handler)(struct client *c, struct object *o, uint op, struct args *a);
} handlers[] = {
    {&wl_display_interface, display_request},
    {&wl_registry_interface, registry_request},
    {&wl_compositor_interface, compositor_request},
    {&wl_surface_interface, surface_request},
    {&wl_region_interface, region_request},
    {&wl_shm_interface, shm_request},
    {&wl_shm_pool_interface, pool_request},
    {&wl_buffer_interface, buffer_request},
    {&wl_seat_interface, seat_request},
    {&wl_pointer_interface, pointer_request},
    {&wl_keyboard_interface, keyboard_request},
    {&wl_touch_interface, touch_request},
    {&wl_output_interface, output_request},
    {&xdg_wm_base_interface, wm_base_request},
    {&xdg_positioner_interface, positioner_request},
    {&xdg_surface_interface, xdg_surface_request},
    {&xdg_toplevel_interface, toplevel_request},
};

#define NHANDLERS ((int)(sizeof(handlers) / sizeof(handlers[0])))

uint next_serial(void) {
    srv.serial++;
    if (srv.serial == 0) {
        srv.serial = 1;
    }
    return srv.serial;
}

struct client *client_new(int fd) {
    struct client *c = malloc(sizeof(struct client));
    if (c == NULL) {
        return NULL;
    }
    memset(c, 0, sizeof(struct client));
    c->fd = fd;
    c->number = ++srv.next_number;
    gui_wire_init(&c->wire, fd);
    gui_wire_nonblock(&c->wire);
    struct ucred cred;
    uint len = sizeof(cred);
    if (getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &cred, &len) == 0 && len == sizeof(cred)) {
        c->pid = cred.pid;
    }
    /* wl_display is object 1 from the start. */
    c->objects[0].id = 1;
    c->objects[0].iface = &wl_display_interface;
    c->objects[0].version = 1;
    c->nobjects = 1;
    if (srv.verbose) {
        printf("compositor: client %d (pid %d) connected\n", c->number, c->pid);
    }
    return c;
}

void client_free(struct client *c) {
    if (srv.verbose) {
        printf("compositor: client %d (pid %d) gone\n", c->number, c->pid);
    }
    /* Windows and surfaces first, then the rest: a buffer a surface has
     * pending is forgotten with the surface. */
    for (int pass = 0; pass < 3; pass++) {
        for (int i = 0; i < c->nobjects; i++) {
            struct object *o = &c->objects[i];
            int first = o->iface == &xdg_toplevel_interface || o->iface == &xdg_surface_interface;
            int second = o->iface == &wl_surface_interface;
            if (o->data != NULL && ((pass == 0 && first) || (pass == 1 && second) || pass == 2)) {
                object_free(c, o);
                o->data = NULL;
            }
        }
    }
    gui_wire_release(&c->wire);
    close(c->fd);
    free(c);
}

struct object *object_find(struct client *c, uint id) {
    for (int i = 0; i < c->nobjects; i++) {
        if (c->objects[i].id == id) {
            return &c->objects[i];
        }
    }
    return NULL;
}

struct object *object_new(struct client *c, uint id, const struct wl_interface *iface, int version, void *data) {
    if (c->nobjects == MAX_OBJECTS) {
        post_error(c, 1, WL_DISPLAY_ERROR_NO_MEMORY, "too many objects");
        return NULL;
    }
    struct object *o = &c->objects[c->nobjects++];
    o->id = id;
    o->iface = iface;
    o->version = version;
    o->data = data;
    return o;
}

void object_destroy(struct client *c, struct object *o) {
    uint id = o->id;
    if (o->data != NULL) {
        object_free(c, o);
    }
    /* The last object takes its slot: a pointer to an object is good only
     * until the next one goes. */
    int i = (int)(o - c->objects);
    c->objects[i] = c->objects[--c->nobjects];
    event_begin(c, 1, WL_DISPLAY_DELETE_ID);
    gui_wire_uint(&c->wire, id);
    event_end(c);
}

void event_begin(struct client *c, uint object, uint opcode) {
    gui_wire_begin(&c->wire, object, opcode);
}

void event_end(struct client *c) {
    if (gui_wire_end(&c->wire) < 0 && !c->dead) {
        if (srv.verbose || c->wire.error != -EPIPE) {
            printf("compositor: client %d: can't send it events (error %d)\n", c->number, -c->wire.error);
        }
        c->dead = 1;
    }
}

void post_error(struct client *c, uint object, uint code, const char *msg) {
    if (c->dead) {
        return;
    }
    printf("compositor: client %d (pid %d): object %u: error %u: %s\n", c->number, c->pid, object, code, msg);
    event_begin(c, 1, WL_DISPLAY_ERROR);
    gui_wire_uint(&c->wire, object);
    gui_wire_uint(&c->wire, code);
    gui_wire_string(&c->wire, msg);
    event_end(c);
    c->dead = 1;
}

/* The descriptors the request came with that no handler kept. */
static void close_fds(struct args *a) {
    for (int i = 0; i < a->n; i++) {
        if (a->fd[i] >= 0) {
            close(a->fd[i]);
            a->fd[i] = -1;
        }
    }
}

/* Reads message m's arguments, for request `req` of object `target`, into
 * *a: 0, or -1 with the error posted. */
static int demarshal(struct client *c, struct gui_message *m, struct object *target, const struct wl_message *req,
                     struct args *a) {
    char what[96];
    const char *sig = req->signature;
    int since = 0;
    while (*sig >= '0' && *sig <= '9') {
        since = since * 10 + (*sig - '0');
        sig++;
    }
    if (since > target->version) {
        snprintf(what, sizeof(what), "%s.%s needs version %d, the object has %d", target->iface->name, req->name,
                 since, target->version);
        post_error(c, 1, WL_DISPLAY_ERROR_INVALID_METHOD, what);
        return -1;
    }
    int nullable = 0;
    for (; *sig != 0; sig++) {
        if (*sig == '?') {
            nullable = 1;
            continue;
        }
        int i = a->n;
        if (i == MAX_ARGS) {
            post_error(c, 1, WL_DISPLAY_ERROR_IMPLEMENTATION, "a request of too many arguments");
            return -1;
        }
        const struct wl_interface *type = req->types[i];
        a->fd[i] = -1;
        a->o[i] = NULL;
        a->s[i] = NULL;
        a->a[i] = NULL;
        a->alen[i] = 0;
        a->u[i] = 0;
        a->n++;
        switch (*sig) {
        case 'i':
        case 'u':
        case 'f':
            a->u[i] = gui_msg_uint(m);
            break;
        case 's':
            a->s[i] = gui_msg_string(m);
            if (!m->bad && a->s[i] == NULL && !nullable) {
                snprintf(what, sizeof(what), "%s.%s: a null string", target->iface->name, req->name);
                post_error(c, 1, WL_DISPLAY_ERROR_INVALID_METHOD, what);
                return -1;
            }
            break;
        case 'o': {
            uint id = gui_msg_uint(m);
            a->u[i] = id;
            if (m->bad) {
                break;
            }
            if (id == 0) {
                if (!nullable) {
                    snprintf(what, sizeof(what), "%s.%s: a null object", target->iface->name, req->name);
                    post_error(c, 1, WL_DISPLAY_ERROR_INVALID_METHOD, what);
                    return -1;
                }
                break;
            }
            struct object *o = object_find(c, id);
            if (o == NULL || (type != NULL && o->iface != type)) {
                snprintf(what, sizeof(what), "%s.%s: invalid object %u", target->iface->name, req->name, id);
                post_error(c, 1, WL_DISPLAY_ERROR_INVALID_OBJECT, what);
                return -1;
            }
            a->o[i] = o;
            break;
        }
        case 'n': {
            uint id = gui_msg_uint(m);
            a->u[i] = id;
            if (!m->bad && (id == 0 || id > CLIENT_ID_MAX || object_find(c, id) != NULL)) {
                snprintf(what, sizeof(what), "%s.%s: invalid new id %u", target->iface->name, req->name, id);
                post_error(c, 1, WL_DISPLAY_ERROR_INVALID_OBJECT, what);
                return -1;
            }
            break;
        }
        case 'a':
            a->a[i] = gui_msg_array(m, &a->alen[i]);
            break;
        case 'h':
            a->fd[i] = gui_msg_fd(m);
            break;
        default:
            post_error(c, 1, WL_DISPLAY_ERROR_IMPLEMENTATION, "a signature the server can't read");
            return -1;
        }
        if (m->bad) {
            snprintf(what, sizeof(what), "%s.%s: argument %d can't be read", target->iface->name, req->name, i + 1);
            post_error(c, 1, WL_DISPLAY_ERROR_INVALID_METHOD, what);
            return -1;
        }
        nullable = 0;
    }
    if (m->pos != m->size - 8) {
        snprintf(what, sizeof(what), "%s.%s: %u bytes past its arguments", target->iface->name, req->name,
                 m->size - 8 - m->pos);
        post_error(c, 1, WL_DISPLAY_ERROR_INVALID_METHOD, what);
        return -1;
    }
    return 0;
}

static void dispatch(struct client *c, struct gui_message *m) {
    char what[96];
    struct object *o = object_find(c, m->object);
    if (o == NULL) {
        snprintf(what, sizeof(what), "invalid object %u", m->object);
        post_error(c, 1, WL_DISPLAY_ERROR_INVALID_OBJECT, what);
        return;
    }
    if (m->opcode >= (uint)o->iface->method_count) {
        snprintf(what, sizeof(what), "invalid method %u of %s@%u", m->opcode, o->iface->name, o->id);
        post_error(c, 1, WL_DISPLAY_ERROR_INVALID_METHOD, what);
        return;
    }
    struct args a;
    a.n = 0;
    int r = demarshal(c, m, o, &o->iface->methods[m->opcode], &a);
    if (r == 0) {
        for (int i = 0; i < NHANDLERS; i++) {
            if (handlers[i].iface == o->iface) {
                handlers[i].handler(c, o, m->opcode, &a);
                break;
            }
        }
    }
    close_fds(&a);
}

int client_input(struct client *c) {
    int n = gui_wire_read(&c->wire, 1);
    if (n == -EAGAIN) {
        return 0;
    }
    if (n == 0 || n == -ECONNRESET || n == -EPIPE) {
        return -1; /* hung up */
    }
    if (n < 0) {
        post_error(c, 1, WL_DISPLAY_ERROR_INVALID_METHOD, "descriptors that can't be matched to messages");
        return -1;
    }
    struct gui_message m;
    while (!c->dead) {
        int r = gui_wire_next(&c->wire, &m);
        if (r == 0) {
            break;
        }
        if (r < 0) {
            post_error(c, 1, WL_DISPLAY_ERROR_INVALID_METHOD, "a message of a size that can't be");
            break;
        }
        dispatch(c, &m);
    }
    return c->dead ? -1 : 0;
}

int client_flush(struct client *c) {
    if (c->wire.out_len == 0) {
        return 0;
    }
    int r = gui_wire_flush(&c->wire);
    if (r == -EAGAIN) {
        return 0;
    }
    return r < 0 ? -1 : 0;
}
