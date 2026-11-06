/* wltest: a client of the display server (#184; kernel/user/compositor),
 * for crates/xtask/tests/display.rs.
 *
 *   wltest window [-n] [-f] [-m] NAME RRGGBB WIDTH HEIGHT
 *       a window titled NAME, WIDTH by HEIGHT, of colour RRGGBB (hex),
 *       through the GUI library (gui.h), which says what comes to it, a
 *       line each: "wltest NAME: shown" once its first frame is on the
 *       screen; "pointer X Y", "leave", "button CODE 1|0", "scroll N",
 *       "key CODE 1|0", "modifiers HELD LOCKED", "focus 1|0", "close". The
 *       right button asks the server to move the window
 *       (xdg_toplevel.move); Escape ends it. It ends when asked to close,
 *       but with -n, when the server must kill it. With -f each frame is
 *       a new colour, RRGGBB and its inverse in turn, as fast as the
 *       server's frame callbacks come ("frame N" every 16th); with -m too,
 *       each frame also asks the server to move the window, a press or
 *       not, as a client shouldn't (the server must ignore it).
 *   wltest probe
 *       the server's globals, "wltest: global NAME INTERFACE VERSION".
 *   wltest stale
 *       a window made over the wire itself, which a press of the right
 *       button asks to move with the serial before the press's, then with
 *       the press's ("wltest stale: committed", "asked with the serial
 *       before", "released").
 *   wltest bad KIND
 *       breaks the protocol, one way a KIND, on a connection of its own
 *       (not the GUI library's, which never would), and says what the
 *       server answers, "wltest: error OBJECT CODE: MESSAGE", and that it
 *       hung up, "wltest: disconnected". KIND is object (a request to an
 *       object that isn't), opcode (a request the interface hasn't), size
 *       (a message too short to be one), newid (a new id in use), string
 *       (a string without its NUL), type (an object of the wrong
 *       interface), trailing (bytes past the arguments), fd (a request
 *       without the descriptor it needs), fds (descriptors no request
 *       takes), pool (a pool bigger than its memory), unsealed (a pool
 *       whose memory may shrink), pools (more pools than a client may
 *       have), buffer (a buffer past its pool's end).
 *
 * Exits 0, or 1 when something went otherwise. */
#include "gui.h"
#include "gui/protocol.h"

#define WL_DISPLAY_ID 1

/* ---- A window ---- */

static const char *name;
static uint color;
static int stubborn;
static int flashing;
static int pushy;
static int quit;
static int shown;
static int frames;
static struct gui_window *win;

static void draw(struct gui_window *w) {
    struct gui_buffer *b = gui_window_buffer(w);
    if (b == NULL) {
        quit = 1;
        return;
    }
    uint c = flashing && frames % 2 == 1 ? (~color & 0xffffff) | 0xff000000u : color;
    struct gui_rect all = gui_rect(0, 0, b->width, b->height);
    gui_fill(b, all, c);
    gui_window_damage(w, all);
    if (gui_window_commit(w) < 0) {
        quit = 1;
    }
}

static void on_close(struct gui_window *w) {
    (void)w;
    printf("wltest %s: close\n", name);
    if (!stubborn) {
        quit = 1;
    }
}

static void on_pointer(struct gui_window *w, int x, int y) {
    (void)w;
    printf("wltest %s: pointer %d %d\n", name, x, y);
}

static void on_leave(struct gui_window *w) {
    (void)w;
    printf("wltest %s: leave\n", name);
}

static void on_button(struct gui_window *w, uint button, int pressed) {
    printf("wltest %s: button %u %d\n", name, button, pressed);
    if (button == BTN_RIGHT && pressed) {
        gui_window_move(w);
    }
}

static void on_scroll(struct gui_window *w, int amount) {
    (void)w;
    printf("wltest %s: scroll %d\n", name, amount);
}

static void on_key(struct gui_window *w, uint code, int pressed) {
    (void)w;
    printf("wltest %s: key %u %d\n", name, code, pressed);
    if (code == KEY_ESC && pressed) {
        quit = 1;
    }
}

static void on_modifiers(struct gui_window *w, uint held, uint locked) {
    (void)w;
    printf("wltest %s: modifiers %u %u\n", name, held, locked);
}

static void on_focus(struct gui_window *w, int focused) {
    (void)w;
    printf("wltest %s: focus %d\n", name, focused);
}

static void on_frame(struct gui_window *w) {
    if (!shown) {
        shown = 1;
        printf("wltest %s: shown\n", name);
    }
    if (flashing) {
        frames++;
        if (frames % 16 == 0) {
            printf("wltest %s: frame %d\n", name, frames);
        }
        if (pushy) {
            gui_window_move(w);
        }
        draw(w);
    }
}

static const struct gui_window_handlers handlers = {
    .close = on_close,
    .pointer = on_pointer,
    .leave = on_leave,
    .button = on_button,
    .scroll = on_scroll,
    .key = on_key,
    .modifiers = on_modifiers,
    .focus = on_focus,
    .frame = on_frame,
};

static int window(int argc, char **argv) {
    int i = 0;
    while (i < argc && argv[i][0] == '-') {
        if (strcmp(argv[i], "-n") == 0) {
            stubborn = 1;
        } else if (strcmp(argv[i], "-f") == 0) {
            flashing = 1;
        } else if (strcmp(argv[i], "-m") == 0) {
            pushy = 1;
        } else {
            return 2;
        }
        i++;
    }
    if (argc - i != 4) {
        return 2;
    }
    name = argv[i];
    color = 0xff000000u | (uint)strtol(argv[i + 1], NULL, 16);
    int width = atoi(argv[i + 2]);
    int height = atoi(argv[i + 3]);
    int err;
    struct gui_display *d = gui_connect(NULL, &err);
    if (d == NULL) {
        printf("wltest %s: can't connect: %s\n", name, strerror(-err));
        return 1;
    }
    win = gui_window_open(d, name, width, height, &handlers, NULL);
    if (win == NULL) {
        printf("wltest %s: no window\n", name);
        return 1;
    }
    draw(win);
    while (!quit) {
        int r = gui_dispatch(d, -1);
        if (r < 0) {
            printf("wltest %s: disconnected\n", name);
            return 1;
        }
    }
    gui_window_destroy(win);
    gui_disconnect(d);
    return 0;
}

/* ---- A connection of its own ---- */

static struct gui_wire wire;
/* The globals' names, by interface: 0 if the server hasn't it. */
static uint compositor_name;
static uint shm_name;
static uint wm_base_name;
static uint seat_name;

static int raw_connect(void) {
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un a;
    memset(&a, 0, sizeof(a));
    a.sun_family = AF_UNIX;
    strcpy(a.sun_path, "/run/wayland-0");
    if (fd < 0 || connect(fd, &a) < 0) {
        printf("wltest: can't connect\n");
        return -1;
    }
    gui_wire_init(&wire, fd);
    return 0;
}

static void send0(uint object, uint opcode) {
    gui_wire_begin(&wire, object, opcode);
    gui_wire_end(&wire);
}

static void send1(uint object, uint opcode, uint arg) {
    gui_wire_begin(&wire, object, opcode);
    gui_wire_uint(&wire, arg);
    gui_wire_end(&wire);
}

/* Reads events until `stop` (a callback's done, 0: the hang-up), printing
 * the globals with `print` and any error: 1 if the server hung up. */
static int events(uint stop, int print) {
    gui_wire_flush(&wire);
    for (;;) {
        struct gui_message m;
        int r;
        while ((r = gui_wire_next(&wire, &m)) == 1) {
            if (m.object == WL_DISPLAY_ID && m.opcode == WL_DISPLAY_ERROR) {
                uint object = gui_msg_uint(&m);
                uint code = gui_msg_uint(&m);
                const char *msg = gui_msg_string(&m);
                printf("wltest: error %u %u: %s\n", object, code, msg != NULL ? msg : "");
            } else if (m.object == 2 && m.opcode == WL_REGISTRY_GLOBAL) {
                uint global = gui_msg_uint(&m);
                const char *iface = gui_msg_string(&m);
                uint version = gui_msg_uint(&m);
                if (iface == NULL) {
                    continue;
                }
                if (print) {
                    printf("wltest: global %u %s %u\n", global, iface, version);
                }
                if (strcmp(iface, "wl_compositor") == 0) {
                    compositor_name = global;
                } else if (strcmp(iface, "wl_shm") == 0) {
                    shm_name = global;
                } else if (strcmp(iface, "xdg_wm_base") == 0) {
                    wm_base_name = global;
                } else if (strcmp(iface, "wl_seat") == 0) {
                    seat_name = global;
                }
            } else if (stop != 0 && m.object == stop && m.opcode == WL_CALLBACK_DONE) {
                return 0;
            }
        }
        if (r < 0) {
            printf("wltest: a message that can't be read\n");
            return 1;
        }
        r = gui_wire_read(&wire, 0);
        if (r == 0 || r == -ECONNRESET) {
            printf("wltest: disconnected\n");
            return 1;
        }
        if (r < 0) {
            printf("wltest: read: %s\n", strerror(-r));
            return 1;
        }
    }
}

/* The registry as object 2, and its globals, synced with callback 3. */
static int registry(int print) {
    send1(WL_DISPLAY_ID, WL_DISPLAY_GET_REGISTRY, 2);
    send1(WL_DISPLAY_ID, WL_DISPLAY_SYNC, 3);
    return events(3, print);
}

static void bind_global(uint global, const char *iface, uint version, uint id) {
    gui_wire_begin(&wire, 2, WL_REGISTRY_BIND);
    gui_wire_uint(&wire, global);
    gui_wire_string(&wire, iface);
    gui_wire_uint(&wire, version);
    gui_wire_uint(&wire, id);
    gui_wire_end(&wire);
}

/* A message's raw words, the header's size and opcode word as given. */
static void raw(const uint *words, int n) {
    send(wire.fd, words, (size_t)n * 4);
}

static int probe(void) {
    if (raw_connect() < 0 || registry(1) != 0) {
        return 1;
    }
    return 0;
}

/* A pool of `size` bytes over a memfd of `mem` bytes, sealed against
 * shrinking or not, as object `id`, from wl_shm, object 4. */
static void make_pool(uint id, int mem, int size, int sealed) {
    int fd = memfd_create("wltest", sealed ? MFD_ALLOW_SEALING : 0);
    ftruncate(fd, mem);
    if (sealed) {
        fcntl(fd, F_ADD_SEALS, F_SEAL_SHRINK);
    }
    gui_wire_begin(&wire, 4, WL_SHM_CREATE_POOL);
    gui_wire_uint(&wire, id);
    gui_wire_fd(&wire, fd);
    gui_wire_int(&wire, size);
    gui_wire_end(&wire);
    gui_wire_flush(&wire);
    close(fd);
}

static int bad(const char *kind) {
    if (raw_connect() < 0 || registry(0) != 0) {
        return 1;
    }
    if (strcmp(kind, "object") == 0) {
        send0(99, 0);
    } else if (strcmp(kind, "opcode") == 0) {
        send0(WL_DISPLAY_ID, 7);
    } else if (strcmp(kind, "size") == 0) {
        uint words[2];
        words[0] = WL_DISPLAY_ID;
        words[1] = (4u << 16) | WL_DISPLAY_SYNC;
        gui_wire_flush(&wire);
        raw(words, 2);
    } else if (strcmp(kind, "newid") == 0) {
        send1(WL_DISPLAY_ID, WL_DISPLAY_GET_REGISTRY, 2);
    } else if (strcmp(kind, "string") == 0) {
        /* bind(name, "abcd" with no NUL in its 4 bytes, version, id) */
        uint words[7];
        words[0] = 2;
        words[1] = (28u << 16) | WL_REGISTRY_BIND;
        words[2] = compositor_name;
        words[3] = 4;
        memcpy(&words[4], "abcd", 4);
        words[5] = 1;
        words[6] = 4;
        gui_wire_flush(&wire);
        raw(words, 7);
    } else if (strcmp(kind, "type") == 0) {
        bind_global(compositor_name, "wl_compositor", 4, 4);
        send1(4, WL_COMPOSITOR_CREATE_SURFACE, 5);
        /* attach(the registry as the buffer, 0, 0) */
        gui_wire_begin(&wire, 5, WL_SURFACE_ATTACH);
        gui_wire_uint(&wire, 2);
        gui_wire_int(&wire, 0);
        gui_wire_int(&wire, 0);
        gui_wire_end(&wire);
    } else if (strcmp(kind, "trailing") == 0) {
        uint words[4];
        words[0] = WL_DISPLAY_ID;
        words[1] = (16u << 16) | WL_DISPLAY_SYNC;
        words[2] = 4;
        words[3] = 0;
        gui_wire_flush(&wire);
        raw(words, 4);
    } else if (strcmp(kind, "fd") == 0) {
        bind_global(shm_name, "wl_shm", 1, 4);
        gui_wire_begin(&wire, 4, WL_SHM_CREATE_POOL);
        gui_wire_uint(&wire, 5);
        gui_wire_int(&wire, 4096);
        gui_wire_end(&wire);
    } else if (strcmp(kind, "fds") == 0) {
        /* wl_display.sync with three descriptors, which it doesn't take */
        int fd = memfd_create("wltest", 0);
        gui_wire_begin(&wire, WL_DISPLAY_ID, WL_DISPLAY_SYNC);
        gui_wire_uint(&wire, 4);
        gui_wire_fd(&wire, fd);
        gui_wire_fd(&wire, fd);
        gui_wire_fd(&wire, fd);
        gui_wire_end(&wire);
        gui_wire_flush(&wire);
        close(fd);
    } else if (strcmp(kind, "pool") == 0) {
        bind_global(shm_name, "wl_shm", 1, 4);
        make_pool(5, 4096, 65536, 1);
    } else if (strcmp(kind, "unsealed") == 0) {
        bind_global(shm_name, "wl_shm", 1, 4);
        make_pool(5, 4096, 4096, 0);
    } else if (strcmp(kind, "pools") == 0) {
        bind_global(shm_name, "wl_shm", 1, 4);
        for (uint id = 5; id < 10; id++) {
            make_pool(id, 4096, 4096, 1);
        }
    } else if (strcmp(kind, "buffer") == 0) {
        bind_global(shm_name, "wl_shm", 1, 4);
        make_pool(5, 4096, 4096, 1);
        gui_wire_begin(&wire, 5, WL_SHM_POOL_CREATE_BUFFER);
        gui_wire_uint(&wire, 6);
        gui_wire_int(&wire, 0);
        gui_wire_int(&wire, 64);
        gui_wire_int(&wire, 64);
        gui_wire_int(&wire, 256);
        gui_wire_uint(&wire, WL_SHM_FORMAT_XRGB8888);
        gui_wire_end(&wire);
    } else {
        printf("wltest: no such kind: %s\n", kind);
        return 2;
    }
    /* The error, then the hang-up. */
    return events(0, 0) == 1 ? 0 : 1;
}

/* stale: a window of its own making, 64 by 48, titled "stale". A press of
 * the right button on it asks for a move with the serial before the
 * press's, which the server must refuse, then (after a roundtrip, so that
 * the server has acted on the first) with the press's own, which starts
 * the move; it ends when the button is let go. */
static void stale_send(uint object, uint opcode, const uint *words, int n) {
    gui_wire_begin(&wire, object, opcode);
    for (int i = 0; i < n; i++) {
        gui_wire_uint(&wire, words[i]);
    }
    gui_wire_end(&wire);
}

static int stale(void) {
    if (raw_connect() < 0 || registry(0) != 0) {
        return 1;
    }
    /* wl_shm 4, its pool 5 and buffer 9; wl_compositor 6, xdg_wm_base 7,
     * wl_seat 8, the surface 10, its xdg_surface 11 and toplevel 12, the
     * pointer 13; sync callbacks from 20. */
    bind_global(shm_name, "wl_shm", 1, 4);
    make_pool(5, 64 * 48 * 4, 64 * 48 * 4, 1);
    uint buffer[6] = {9, 0, 64, 48, 256, WL_SHM_FORMAT_XRGB8888};
    stale_send(5, WL_SHM_POOL_CREATE_BUFFER, buffer, 6);
    bind_global(compositor_name, "wl_compositor", 1, 6);
    bind_global(wm_base_name, "xdg_wm_base", 1, 7);
    bind_global(seat_name, "wl_seat", 1, 8);
    send1(6, WL_COMPOSITOR_CREATE_SURFACE, 10);
    uint xdg[2] = {11, 10};
    stale_send(7, XDG_WM_BASE_GET_XDG_SURFACE, xdg, 2);
    send1(11, XDG_SURFACE_GET_TOPLEVEL, 12);
    gui_wire_begin(&wire, 12, XDG_TOPLEVEL_SET_TITLE);
    gui_wire_string(&wire, "stale");
    gui_wire_end(&wire);
    send0(10, WL_SURFACE_COMMIT);
    send1(8, WL_SEAT_GET_POINTER, 13);
    gui_wire_flush(&wire);
    uint sync = 20;
    uint waiting = 0; /* the sync after the stale move */
    uint press = 0;
    for (;;) {
        struct gui_message m;
        int r;
        while ((r = gui_wire_next(&wire, &m)) == 1) {
            if (m.object == WL_DISPLAY_ID && m.opcode == WL_DISPLAY_ERROR) {
                uint object = gui_msg_uint(&m);
                uint code = gui_msg_uint(&m);
                const char *msg = gui_msg_string(&m);
                printf("wltest: error %u %u: %s\n", object, code, msg != NULL ? msg : "");
            } else if (m.object == 11 && m.opcode == XDG_SURFACE_CONFIGURE) {
                send1(11, XDG_SURFACE_ACK_CONFIGURE, gui_msg_uint(&m));
                uint attach[3] = {9, 0, 0};
                stale_send(10, WL_SURFACE_ATTACH, attach, 3);
                uint all[4] = {0, 0, 64, 48};
                stale_send(10, WL_SURFACE_DAMAGE, all, 4);
                send0(10, WL_SURFACE_COMMIT);
                gui_wire_flush(&wire);
                printf("wltest stale: committed\n");
            } else if (m.object == 13 && m.opcode == WL_POINTER_BUTTON) {
                uint serial = gui_msg_uint(&m);
                gui_msg_uint(&m);
                uint button = gui_msg_uint(&m);
                uint state = gui_msg_uint(&m);
                if (button != BTN_RIGHT) {
                    continue;
                }
                if (state != WL_POINTER_BUTTON_STATE_PRESSED) {
                    printf("wltest stale: released\n");
                    return 0;
                }
                press = serial;
                uint move[2] = {8, serial - 1};
                stale_send(12, XDG_TOPLEVEL_MOVE, move, 2);
                waiting = ++sync;
                send1(WL_DISPLAY_ID, WL_DISPLAY_SYNC, waiting);
                gui_wire_flush(&wire);
            } else if (waiting != 0 && m.object == waiting && m.opcode == WL_CALLBACK_DONE) {
                waiting = 0;
                printf("wltest stale: asked with the serial before\n");
                uint move[2] = {8, press};
                stale_send(12, XDG_TOPLEVEL_MOVE, move, 2);
                gui_wire_flush(&wire);
            }
        }
        if (r < 0) {
            printf("wltest: a message that can't be read\n");
            return 1;
        }
        r = gui_wire_read(&wire, 0);
        if (r <= 0) {
            printf("wltest: disconnected\n");
            return 1;
        }
    }
}

int main(int argc, char **argv) {
    int r = 2;
    if (argc >= 2 && strcmp(argv[1], "window") == 0) {
        r = window(argc - 2, argv + 2);
    } else if (argc == 2 && strcmp(argv[1], "probe") == 0) {
        r = probe();
    } else if (argc == 2 && strcmp(argv[1], "stale") == 0) {
        r = stale();
    } else if (argc == 3 && strcmp(argv[1], "bad") == 0) {
        r = bad(argv[2]);
    }
    if (r == 2) {
        fprintf(2, "usage: wltest window [-n] [-f] [-m] NAME RRGGBB WIDTH HEIGHT | probe | stale | bad KIND\n");
    }
    return r;
}
