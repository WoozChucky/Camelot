/* compositor: Camelot's display server (compositor.h; docs/excalibur-spec.md
 * section 10, "The display server"; docs/camelot-desktop.md).
 *
 *   compositor [-v] [-l LAUNCHER] [-s SOCKET]
 *
 * takes the display and the input devices and serves Wayland clients at
 * SOCKET (/run/wayland-0) until it is killed. -l names the program the
 * logo key starts (the desktop's /etc/rc gives one); -v says what the
 * clients do (connections, focus). It prints "compositor: ready" once
 * clients can connect. With no framebuffer, or another display server at
 * SOCKET, it exits with EX_CONFIG (78), which init takes as a service this
 * machine can't run.
 *
 *   compositor -x PROGRAM
 *
 * is how it starts the launcher: this second process starts PROGRAM and
 * ends at once, so that PROGRAM is init's child (an orphan), collected by
 * init when it ends, and the server, whose wait would block, has none.
 *
 * One thread, one poll loop: the listening socket, the keyboard and the
 * pointer, the framebuffer (a flip done) and every client, each read when
 * ready and its messages acted on in order; the events they cause are
 * queued on the clients' wires and sent without waiting (a client that
 * reads nothing for long is disconnected, gui_wire_nonblock). Then, unless
 * a flip is still waiting for the vertical blank, a frame is drawn of what
 * changed. */
#include "compositor.h"

struct server srv;

uint now_ms(void) {
    return uptime() * 100;
}

static void usage(void) {
    fprintf(2, "usage: compositor [-v] [-l LAUNCHER] [-s SOCKET]\n");
    exit(2);
}

/* The last part of a path. */
static const char *base_name(const char *path) {
    const char *slash = strrchr(path, '/');
    return slash != NULL ? slash + 1 : path;
}

void launch(const char *path) {
    char *argv[4];
    argv[0] = "compositor";
    argv[1] = "-x";
    argv[2] = (char *)path;
    argv[3] = NULL;
    int pid = spawn(SELF_PATH, argv, environ);
    if (pid < 0) {
        printf("compositor: can't start %s: %s\n", path, strerror(-pid));
        return;
    }
    wait(pid, NULL); /* at once: it only starts the program */
}

/* -x: starts `path` and ends. */
static int start_orphan(const char *path) {
    char *argv[2];
    argv[0] = (char *)base_name(path);
    argv[1] = NULL;
    int pid = spawn(path, argv, environ);
    if (pid < 0) {
        printf("compositor: can't start %s: %s\n", path, strerror(-pid));
        return 1;
    }
    return 0;
}

static int listen_on(const char *path) {
    struct sockaddr_un a;
    memset(&a, 0, sizeof(a));
    a.sun_family = AF_UNIX;
    if (strlen(path) >= sizeof(a.sun_path)) {
        fprintf(2, "compositor: %s: too long a path\n", path);
        return -1;
    }
    strcpy(a.sun_path, path);
    /* A server that answers there already is left alone; a name nobody
     * answers on is a server's that ended, and is taken. */
    int probe = socket(AF_UNIX, SOCK_STREAM, 0);
    if (probe >= 0) {
        int r = connect(probe, &a);
        close(probe);
        if (r == 0) {
            fprintf(2, "compositor: a display server is at %s already\n", path);
            return -1;
        }
    }
    unlink(path);
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0 || bind(fd, &a) < 0 || listen(fd, 8) < 0) {
        fprintf(2, "compositor: can't listen on %s\n", path);
        return -1;
    }
    return fd;
}

static void disconnect(int i) {
    struct client *c = srv.clients[i];
    srv.clients[i] = NULL;
    /* Best effort: an error it was sent goes before the hang-up. */
    client_flush(c);
    client_free(c);
}

static void accept_client(void) {
    int fd = accept(srv.listen_fd, NULL);
    if (fd < 0) {
        return;
    }
    /* A descriptor kept free for one that comes with a message. */
    if (fd >= NOFILE - 1) {
        printf("compositor: out of descriptors: a client refused\n");
        close(fd);
        return;
    }
    for (int i = 0; i < MAX_CLIENTS; i++) {
        if (srv.clients[i] == NULL) {
            srv.clients[i] = client_new(fd);
            if (srv.clients[i] == NULL) {
                close(fd);
            }
            return;
        }
    }
    printf("compositor: too many clients: one refused\n");
    close(fd);
}

int main(int argc, char **argv) {
    const char *path = SOCKET_PATH;
    if (argc == 3 && strcmp(argv[1], "-x") == 0) {
        return start_orphan(argv[2]);
    }
    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "-v") == 0) {
            srv.verbose = 1;
        } else if (strcmp(argv[i], "-l") == 0 && i + 1 < argc) {
            srv.launcher = argv[++i];
        } else if (strcmp(argv[i], "-s") == 0 && i + 1 < argc) {
            path = argv[++i];
        } else {
            usage();
        }
    }
    /* Standard input isn't read: its descriptor is the keymap's, an empty
     * memfd every wl_keyboard.keymap carries. */
    close(0);
    srv.keymap_fd = memfd_create("keymap", MFD_CLOEXEC);
    srv.listen_fd = listen_on(path);
    if (srv.listen_fd < 0) {
        return EX_CONFIG;
    }
    if (screen_init() < 0) {
        unlink(path);
        return EX_CONFIG;
    }
    input_init();
    printf("compositor: ready on %s (%dx%d)\n", path, srv.width, srv.height);

    for (;;) {
        struct pollfd fds[4 + MAX_CLIENTS];
        int who[4 + MAX_CLIENTS];
        int n = 0;
        fds[n].fd = srv.listen_fd;
        fds[n].events = POLLIN;
        who[n++] = -1;
        fds[n].fd = srv.kbd_fd;
        fds[n].events = POLLIN;
        who[n++] = -2;
        fds[n].fd = srv.ptr_fd;
        fds[n].events = POLLIN;
        who[n++] = -3;
        fds[n].fd = srv.flip_pending ? srv.fb_fd : -1;
        fds[n].events = POLLOUT;
        who[n++] = -4;
        for (int i = 0; i < MAX_CLIENTS; i++) {
            struct client *c = srv.clients[i];
            if (c != NULL) {
                fds[n].fd = c->fd;
                fds[n].events = POLLIN | (c->wire.out_len > 0 ? POLLOUT : 0);
                who[n++] = i;
            }
        }
        for (int i = 0; i < n; i++) {
            fds[i].revents = 0;
        }
        int ticks = close_deadlines();
        int timeout = ticks < 0 ? -1 : ticks * 100;
        if (srv.repaint && !srv.flip_pending) {
            timeout = 0;
        }
        poll(fds, (unsigned long)n, timeout);

        for (int i = 0; i < n; i++) {
            short r = fds[i].revents;
            if (r == 0) {
                continue;
            }
            if (who[i] == -1) {
                accept_client();
            } else if (who[i] == -2) {
                keyboard_input();
            } else if (who[i] == -3) {
                pointer_input();
            } else if (who[i] == -4) {
                flip_done();
            } else if (srv.clients[who[i]] != NULL && (r & (POLLIN | POLLHUP | POLLERR))) {
                if (client_input(srv.clients[who[i]]) < 0) {
                    srv.clients[who[i]]->dead = 1;
                }
            }
        }
        pointer_repick();
        close_deadlines();
        for (int i = 0; i < MAX_CLIENTS; i++) {
            struct client *c = srv.clients[i];
            if (c == NULL) {
                continue;
            }
            if (c->dead || client_flush(c) < 0) {
                disconnect(i);
            }
        }
        if (srv.repaint && !srv.flip_pending) {
            repaint();
        }
    }
}
