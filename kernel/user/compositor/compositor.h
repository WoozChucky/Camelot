/* The display server (docs/excalibur-spec.md section 10, "The display
 * server"): a Wayland compositor of the core protocol's subset and
 * xdg-shell's toplevel windows, in one process and one thread, around a
 * poll loop (main.c). It owns /dev/fb0, draws into
 * the frame the display isn't showing and flips at the vertical blank
 * (compose.c); it grabs the keyboard and the pointer and is the window
 * manager (wm.c); its clients connect to /run/wayland-0 and speak the wire
 * format, every message checked against the protocol's tables before it
 * is acted on (client.c), and the requests of each interface are core.c's
 * and xdg.c's, and the desktop shell's, the layer shell, the foreign
 * toplevel manager and the clipboard, shell.c's.
 *
 * Nothing a client sends is trusted: an id must name one of its own
 * objects of the interface the request says, a new id must be free, a
 * size, an offset, a stride must fit what they describe. A client that
 * breaks the protocol gets wl_display.error and is disconnected; the
 * server goes on. */
#ifndef CAMELOT_COMPOSITOR_H
#define CAMELOT_COMPOSITOR_H

#include "../gui.h"
#include "../gui/protocol.h"

/* Descriptors a process has (NOFILE in kernel/include/file.h). */
#define NOFILE 64
/* Clients at once: the panel, terminals and apps (a machine has 32
 * processes, NPROC). */
#define MAX_CLIENTS 8
/* The pools a client has at once (a window's is one), and the descriptors
 * it may send ahead of the messages that take them. */
#define MAX_POOLS 3
#define MAX_QUEUED_FDS 2
/* Descriptors the server holds for a client until they are sent: a pipe's
 * end a paste passes to the clipboard's source (shell.c). */
#define MAX_HELD_FDS 1
/* The server's own descriptors: the keymap's (0), standard output and
 * error, the listening socket, the framebuffer, the keyboard, the
 * pointer. With each client's socket, pools and descriptors sent ahead,
 * they leave one free, for accept and for a descriptor that comes with a
 * message, so the server never runs out. */
#define SERVER_FDS 7
_Static_assert(SERVER_FDS + MAX_CLIENTS * (1 + MAX_POOLS + MAX_QUEUED_FDS + MAX_HELD_FDS) <= NOFILE - 1,
               "the descriptors the server may hold");
/* Objects a client has at once. */
#define MAX_OBJECTS 256
/* A request's arguments at most (wl_shm_pool.create_buffer has 6). */
#define MAX_ARGS 8
/* The ids a client may make: the server's start at 0xff000000. */
#define CLIENT_ID_MAX 0xfeffffffu
#define SERVER_ID_START 0xff000000u
/* Frame callbacks a surface holds at each stage. */
#define MAX_FRAMES 8
/* Windows at once, layer surfaces, and the biggest buffer and pool. */
#define MAX_WINDOWS 16
#define MAX_LAYERS 8
#define MAX_SIDE 4096
#define MAX_POOL (64 * 1024 * 1024)
/* A window that doesn't go this long (ticks, 10 a second) after it was
 * asked to close has its client killed. */
#define CLOSE_TICKS 30
/* How long a connection that couldn't be accepted waits for another try. */
#define ACCEPT_RETRY_MS 1000
/* The longest title kept. */
#define TITLE_MAX 64
/* The types a clipboard source offers, at most, and their longest name. */
#define MAX_MIMES 4
#define MIME_MAX 64

/* The window decorations: a title bar above the content, a border round
 * both, and a close button at the title bar's right end. */
#define TITLE_H 20
#define BORDER 1
#define CLOSE_SIZE 14
/* How far outside its border the pointer takes a window's edge, to resize
 * it, and the smallest a window is made. */
#define RESIZE_GRIP 6
#define MIN_WIDTH 64
#define MIN_HEIGHT 32

#define SOCKET_PATH "/run/wayland-0"
#define FONT_PATH "/usr/share/fonts/spleen/8x16.psfu"
#define CURSOR_PATH "/usr/share/images/cursor.qoi"
#define SELF_PATH "/bin/compositor"

/* sysexits.h's EX_CONFIG: no display or input to serve (init leaves the
 * service). */
#define EX_CONFIG 78

struct client;
struct window;

/* One of a client's objects: its id, its interface and version, and the
 * interface's own state (NULL for those that have none). */
struct object {
    uint id;
    const struct wl_interface *iface;
    int version;
    void *data;
};

/* A request's arguments, as the signature gives them: numbers, objects
 * and new ids in `u` (an object's in `o` too, NULL for a null one), strings
 * in `s`, arrays in `a` and `alen`, descriptors in `fd` (-1 once a handler
 * keeps one: the rest are closed after it). */
struct args {
    int n;
    uint u[MAX_ARGS];
    struct object *o[MAX_ARGS];
    const char *s[MAX_ARGS];
    const void *a[MAX_ARGS];
    uint alen[MAX_ARGS];
    int fd[MAX_ARGS];
};

/* A wl_shm_pool's memory: mapped from the client's memfd, whose
 * descriptor is kept for wl_shm_pool.resize. The pool object and each of
 * its buffers hold a reference: the last one unmaps it. */
struct pool {
    struct client *c; /* whose: it counts among its MAX_POOLS */
    int fd;
    uchar *mem;
    size_t size;
    int refs;
};

struct buffer {
    struct pool *pool;
    int offset;
    int width;
    int height;
    int stride; /* bytes */
    uint format;
};

/* A wl_surface: what the next commit applies (pending), what it shows
 * (`content`, a copy the server keeps: the buffer goes back to the client
 * at once), and its frame callbacks: requested, committed (waiting for the
 * next frame drawn), shown (waiting for that frame's flip). */
struct surface {
    struct client *c;
    uint id;
    struct buffer *buffer; /* pending: attached since the last commit */
    uint buffer_id;
    int attached;
    struct gui_damage damage;
    uint frames[MAX_FRAMES];
    int nframes;
    uint committed[MAX_FRAMES];
    int ncommitted;
    uint shown[MAX_FRAMES];
    int nshown;
    struct gui_buffer content;
    int has_content;
    int argb;
    struct window *window; /* its role, an xdg_surface's */
};

/* An xdg_surface, and its toplevel once it has one: a window, shown once a
 * buffer is committed after the client acked the first configure. Or a
 * layer surface (shell.c): `layer` 0 to 3, no xdg_surface, no toplevel. */
struct window {
    struct client *c;
    struct surface *surface;
    int layer; /* -1: a toplevel window */
    uint xdg_id;
    uint toplevel_id;
    uint layer_id;
    /* A layer surface's: its anchors, exclusive zone, margins (top, right,
     * bottom, left), whether it takes the keyboard, the size it asked. */
    uint anchor;
    int zone;
    int margin[4];
    int keyboard;
    int req_w;
    int req_h;
    /* The size last configured (0: the client's choice), and the one a
     * resize waits to configure until the last is acked. */
    int cfg_w;
    int cfg_h;
    int pending_w;
    int pending_h;
    /* The last configure sent and the last the client acked (0: none
     * yet): serials only grow, so an ack may name any configure after the
     * last acked, the client not having seen the later ones yet, as
     * xdg_surface allows. Buffers may come once one is acked; `acked` says
     * the last one sent is. */
    uint configure_serial;
    uint acked_serial;
    int acked;
    int mapped;
    int x; /* the content's top left on the screen */
    int y;
    /* What an interactive resize from the left or the top keeps: those
     * edges (xdg_toplevel's resize_edge bits), and the content's right
     * and bottom when it started. Kept until the client commits after
     * acking the configure that ended it (`keep_until`; 0 while the
     * resize goes on), since the last sizes come after the button. */
    uint keep_edges;
    int keep_right;
    int keep_bottom;
    uint keep_until;
    char title[TITLE_MAX];
    int closing;
    uint close_deadline;
};

/* A wl_data_source: what its client offers to the clipboard. */
struct source {
    struct client *c;
    uint id;
    int used; /* set as the selection once: it can't be again */
    int nmimes;
    char mimes[MAX_MIMES][MIME_MAX];
};

struct client {
    int fd;
    int pid; /* SO_PEERCRED's: killed when its window won't close */
    int number; /* for messages */
    int dead; /* sent wl_display.error: disconnected once that is flushed */
    int npools;
    /* The wl_pointer whose motion is the last event queued, unsent (the
     * wire's last 20 bytes), or 0: the next motion to it replaces that
     * one, so a client that reads slowly gets where the pointer is, not
     * every step it took. */
    uint motion_tail;
    struct gui_wire wire;
    uint next_server_id; /* the next object the server makes for it */
    int held_fds[MAX_HELD_FDS];
    int nheld;
    int nobjects;
    struct object objects[MAX_OBJECTS];
};

struct server {
    int verbose;
    const char *launcher;
    int listen_fd;
    int accept_paused; /* accept failed: the listening socket waits for a descriptor to be freed */
    int fb_fd;
    int kbd_fd;
    int ptr_fd;
    int keymap_fd;
    struct client *clients[MAX_CLIENTS];
    int next_number;
    uint serial;

    /* The screen: two frames of the framebuffer's memory, the one drawn
     * into (`back`) and the one shown; `rows` the row the second starts. */
    int width;
    int height;
    int rows;
    int double_buffered;
    struct gui_buffer frames[2];
    int back;
    int flip_pending;
    struct gui_damage damage; /* changed since the last frame drawn */
    struct gui_damage prev;   /* what the last frame drew, which the back frame lacks */
    int repaint;
    int busy; /* something happened since the server was last idle */
    struct gui_font font;
    int have_font;
    struct gui_buffer cursor;
    int have_cursor;

    /* The windows shown, bottom to top, and the one with the keyboard. */
    struct window *stack[MAX_WINDOWS];
    int nstack;
    struct window *focus;
    int placed; /* windows placed so far, for the next one's place */
    /* The layer surfaces shown, by layer, and the work area they leave
     * the windows (shell.c). */
    struct window *layers[MAX_LAYERS];
    int nlayers;
    struct gui_rect work;
    /* The clipboard's selection, or NULL. */
    struct source *selection;

    /* The pointer: where it is, its buttons, the surface it is over (or
     * held on: the implicit grab of a button pressed on it), a window
     * being moved. */
    int px;
    int py;
    uint buttons;  /* held: bit 0 left, 1 right, 2 middle */
    uint consumed; /* held buttons the window manager took */
    struct surface *pointer_surface;
    struct surface *grab;
    uint grab_serial; /* the button press's that made the grab: a move must name it */
    struct window *moving;
    int move_dx;
    int move_dy;
    /* A window being resized: its edges (xdg_toplevel's resize_edge), the
     * pointer and the content's size where it started. */
    struct window *resizing;
    uint resize_edges;
    int resize_x0;
    int resize_y0;
    int resize_w0;
    int resize_h0;

    /* The keyboard: its modifiers, the keys held, and those the window
     * manager took (their releases are its too). */
    struct gui_keyboard kbd;
    uint held[KEY_CNT / 32];
    uint swallowed[KEY_CNT / 32];
};

extern struct server srv;

/* ---- main.c ---- */
uint now_ms(void);
void launch(const char *path);

/* ---- client.c ---- */
struct client *client_new(int fd);
void client_free(struct client *c);
/* Reads what came and acts on each whole message: 0, or -1 if the client
 * is to go (it hung up, or broke the protocol). */
int client_input(struct client *c);
/* Sends what is queued, never waiting: 0, or -1 if the client is to go. */
int client_flush(struct client *c);
struct object *object_find(struct client *c, uint id);
/* A new object, or NULL with a no_memory error posted. */
struct object *object_new(struct client *c, uint id, const struct wl_interface *iface, int version, void *data);
/* Frees the object's state and sends wl_display.delete_id. */
void object_destroy(struct client *c, struct object *o);
/* wl_display.error, and the client goes. */
void post_error(struct client *c, uint object, uint code, const char *msg);
/* Starts and ends an event (gui_wire's): a client whose wire fails goes. */
void event_begin(struct client *c, uint object, uint opcode);
void event_end(struct client *c);
uint next_serial(void);
int ack_configure(struct window *w, uint serial);

/* ---- core.c and xdg.c: each interface's requests ---- */
void display_request(struct client *c, struct object *o, uint op, struct args *a);
void registry_request(struct client *c, struct object *o, uint op, struct args *a);
void compositor_request(struct client *c, struct object *o, uint op, struct args *a);
void surface_request(struct client *c, struct object *o, uint op, struct args *a);
void region_request(struct client *c, struct object *o, uint op, struct args *a);
void shm_request(struct client *c, struct object *o, uint op, struct args *a);
void pool_request(struct client *c, struct object *o, uint op, struct args *a);
void buffer_request(struct client *c, struct object *o, uint op, struct args *a);
void seat_request(struct client *c, struct object *o, uint op, struct args *a);
void pointer_request(struct client *c, struct object *o, uint op, struct args *a);
void keyboard_request(struct client *c, struct object *o, uint op, struct args *a);
void touch_request(struct client *c, struct object *o, uint op, struct args *a);
void output_request(struct client *c, struct object *o, uint op, struct args *a);
void wm_base_request(struct client *c, struct object *o, uint op, struct args *a);
void positioner_request(struct client *c, struct object *o, uint op, struct args *a);
void xdg_surface_request(struct client *c, struct object *o, uint op, struct args *a);
void toplevel_request(struct client *c, struct object *o, uint op, struct args *a);
void layer_shell_request(struct client *c, struct object *o, uint op, struct args *a);
void layer_surface_request(struct client *c, struct object *o, uint op, struct args *a);
void toplevel_manager_request(struct client *c, struct object *o, uint op, struct args *a);
void toplevel_handle_request(struct client *c, struct object *o, uint op, struct args *a);
void data_manager_request(struct client *c, struct object *o, uint op, struct args *a);
void data_source_request(struct client *c, struct object *o, uint op, struct args *a);
void data_device_request(struct client *c, struct object *o, uint op, struct args *a);
void data_offer_request(struct client *c, struct object *o, uint op, struct args *a);
/* An xdg_toplevel.configure of width x height (0: the client's choice),
 * resizing or not, and its xdg_surface.configure. */
void toplevel_configure(struct window *w, int width, int height, int resizing);
/* What an object of each kind frees when it goes (object_destroy, and a
 * client's disconnection). */
void object_free(struct client *c, struct object *o);
void surface_free(struct surface *s);
void window_free(struct window *w);
/* After a frame's flip: each surface's shown callbacks done. Before a
 * frame is drawn: its committed ones become shown. */
void frames_done(struct client *c, uint time);
void frames_shown(struct client *c);

/* ---- shell.c ---- */
/* What a layer surface or a data source frees: 1 if o was one. */
int shell_object_free(struct client *c, struct object *o);
/* A layer surface's commit: its checks and configure, 0 or -1 (an error
 * posted). */
int layer_commit(struct window *w, int new_buffer);
void layer_map(struct window *w);
void layer_unmap(struct window *w);
/* The work area from the layer surfaces shown, and each configured (a
 * size of 0 follows the area) and placed again. */
void layers_arrange(void);
/* A layer surface's configure, if its size is not the last one sent. */
void layer_configure(struct window *w);
/* The foreign toplevel managers: one bound (each window announced to
 * it), a window shown, changed (title, activated) or gone. */
void toplevels_bound(struct client *c, struct object *m);
void toplevel_announce(struct window *w);
void toplevel_changed(struct window *w);
void toplevel_closed(struct window *w);
/* The clipboard's selection offered to client c (it has the keyboard). */
void selection_offer(struct client *c);

/* ---- compose.c ---- */
int screen_init(void);
/* Marks a rectangle of the screen to be drawn again. */
void damage(struct gui_rect r);
void repaint(void);
void flip_done(void);
/* A window's whole rectangle (decorations included), its title bar, its
 * close button, its content. */
struct gui_rect window_frame(const struct window *w);
struct gui_rect window_title(const struct window *w);
struct gui_rect window_close_button(const struct window *w);
struct gui_rect window_content(const struct window *w);
struct gui_rect cursor_rect(void);

/* ---- wm.c ---- */
void input_init(void);
void keyboard_input(void);
void pointer_input(void);
/* Enter and leave for what is now under the pointer (windows came, went
 * or moved under it). */
void pointer_repick(void);
/* A window's first frame: placed, raised and given the keyboard. */
void window_map(struct window *w);
/* Takes a window off the screen (unmapped, destroyed, or its client gone). */
void window_unmap(struct window *w);
void window_focus(struct window *w);
void window_move_start(struct window *w);
/* An interactive resize of `edges` (xdg_toplevel's resize_edge). */
void window_resize_start(struct window *w, uint edges);
/* The window's content changed size: a resize from the left or the top
 * keeps the edge across from it where it was, and a layer surface is
 * placed again. */
void window_resized(struct window *w);
/* A toplevel's commit is done: a resize's kept edges are let go once the
 * size that ended it has come. */
void window_committed(struct window *w);
/* Asks the window's client to close it (killed if it doesn't). */
void window_close(struct window *w);
/* A surface or a client is going: the pointer and keyboard forget it. */
void wm_forget_surface(struct surface *s);
/* The keyboard's enter for a client's new wl_keyboard, the pointer's for
 * a new wl_pointer, if the focus is theirs. */
void keyboard_bound(struct client *c, struct object *kbd);
void pointer_bound(struct client *c, struct object *ptr);
/* Kills the clients of windows asked to close that haven't; the ticks
 * until the next deadline, or -1. */
int close_deadlines(void);

#endif
