/* shd [-p PORT] [-n MAX] [-v]: the shell server. Each TCP connection to
 * PORT (23, telnet's, by default) gets a shell of its own, /bin/sh with the
 * connection as its input, output and error, so several people (or several
 * windows) each have an independent shell on one machine: a busy one never
 * holds up another, nor the console's.
 *
 * Start it in the background from the console, `shd &`; it runs until it
 * is killed. With -v it says on its output that it listens, and a line for
 * each session that starts, hangs up or ends, and each connection it turns
 * away:
 *
 *   shd: listening on port 23
 *   shd: [7] 10.0.2.2:49152 connected
 *   shd: [7] 10.0.2.2:49152 hung up
 *   shd: [7] 10.0.2.2:49152 ended, exit -1
 *   shd: 10.0.2.2:49153 turned away: 4 sessions already
 *
 * How it goes:
 *
 * - The connection becomes the shell's descriptors 0, 1 and 2 as the shell
 *   itself redirects a program's: shd puts it there (close, then dup), spawns
 *   /bin/sh and puts its own back from copies it keeps above 2. It keeps a
 *   descriptor for the connection too, and goes back to `accept`.
 * - A socket has no line discipline: the client edits and echoes lines
 *   (`herald --tcp`, telnet), and the shell reads the connection a byte at
 *   a time (sh.c). The end of the input ends the shell, as Ctrl-D does on
 *   the console.
 * - At most MAX sessions (4 by default, 8 at most) run at once: one more is
 *   told so and closed. Every process the sessions start takes one of the
 *   kernel's 8 process slots too.
 * - shd waits for connections half a second at a time (SO_RCVTIMEO) and in
 *   between looks after its sessions (`procs`): it collects each shell that
 *   ended and ends its connection (shutdown, so that a background program
 *   still holding it does not keep the client waiting), and hangs up the
 *   sessions whose client went away. A client that closes its side of the
 *   connection hangs up, as a telnet client does: once the shell has read
 *   everything the client sent (`fstat`: no byte waiting, and nlink 0, the
 *   other end gone), shd kills the shell and every process it started,
 *   which would otherwise run on, `sleep 5000` say, with nobody to see it.
 *   A script that sends commands and then closes its side should end with
 *   `exit`, so its last command is not cut short.
 *
 * Security: there are no users yet, so the shell runs as whoever started
 * shd, which is root, with no password asked. shd is for a machine whose
 * port is reached only from its own host (Grail's or Avalon's user-mode
 * gateway forwarding a port of 127.0.0.1), never one open to a network. */
#include "user.h"

#define DEFAULT_PORT 23
#define DEFAULT_MAX 4
/* Sessions at most: shd's descriptors are 0 to 2, their copies, the
 * listener and one per session, plus one for a connection being turned
 * away, 16 in all. */
#define MAX_SESSIONS 8
/* How long accept() waits before shd looks after its sessions. */
#define TEND_USEC 500000
/* Processes procs() describes: more than the kernel has. */
#define MAX_PROCS 32

struct session {
    int pid; /* the shell; 0: a free entry */
    int fd;  /* shd's descriptor for the connection */
    int hung_up;
    struct sockaddr_in peer;
};

static struct session sessions[MAX_SESSIONS];
static struct procinfo procs_now[MAX_PROCS];
static int verbose;
/* Copies of shd's own descriptors 0, 1 and 2, kept while a connection is
 * put there for a shell. */
static int own[3];

static int usage(void) {
    fprintf(STDERR, "usage: shd [-p PORT] [-n MAX] [-v]\n");
    return 1;
}

/* Prints "A.B.C.D:PORT" for `a`. */
static void print_peer(const struct sockaddr_in *a) {
    printf("%s:%d", inet_ntoa(a->sin_addr), ntohs(a->sin_port));
}

static void say(const struct session *s, const char *what) {
    if (verbose) {
        printf("shd: [%d] ", s->pid);
        print_peer(&s->peer);
        printf(" %s\n", what);
    }
}

/* Sends `text` to the connection `fd`, then closes it. */
static void turn_away(int fd, const char *text) {
    send(fd, text, (uint)strlen(text));
    close(fd);
}

/* Makes descriptor `fd` (0, 1 or 2) a copy of `from`. */
static void put_at(int fd, int from) {
    close(fd);
    /* The lowest free descriptor: `fd`, just closed. */
    dup(from);
}

/* Starts a shell on the connection `fd`. Returns its pid, or an error. */
static int spawn_shell(int fd) {
    char *argv[2];
    argv[0] = "sh";
    argv[1] = NULL;
    for (int i = 0; i < 3; i++) {
        put_at(i, fd);
    }
    int pid = spawn("/bin/sh", argv);
    for (int i = 0; i < 3; i++) {
        put_at(i, own[i]);
    }
    return pid;
}

/* The state of process `pid` in the last snapshot, or 0 if it isn't there. */
static int state_of(int pid, int n) {
    for (int i = 0; i < n; i++) {
        if ((int)procs_now[i].pid == pid) {
            return (int)procs_now[i].state;
        }
    }
    return 0;
}

/* Kills the shell `pid` and every process it started, and theirs, from the
 * snapshot procs_now[0..n). Pids only grow, and the snapshot is in pid
 * order, so a parent always comes before its children: one pass finds
 * them all. */
static void kill_tree(int pid, int n) {
    int tree[MAX_PROCS];
    int count = 0;
    tree[count] = pid;
    count++;
    for (int i = 0; i < n; i++) {
        int in = 0;
        for (int j = 0; j < count; j++) {
            if ((int)procs_now[i].ppid == tree[j]) {
                in = 1;
            }
        }
        if (in && count < MAX_PROCS) {
            tree[count] = (int)procs_now[i].pid;
            count++;
        }
    }
    /* The children first: a shell killed first would orphan them. */
    for (int j = count - 1; j >= 0; j--) {
        kill(tree[j]);
    }
}

/* Looks after the sessions: collects the shells that ended, and hangs up
 * those whose client went away. */
static void tend(void) {
    int n = procs(procs_now, MAX_PROCS);
    if (n > MAX_PROCS) {
        n = MAX_PROCS;
    }
    for (int i = 0; i < MAX_SESSIONS; i++) {
        struct session *s = &sessions[i];
        if (s->pid == 0) {
            continue;
        }
        int state = state_of(s->pid, n);
        if (state == PROC_ZOMBIE || state == 0) {
            int status = 0;
            wait(s->pid, &status);
            /* Ended for everyone, background programs still holding it
             * included: the client sees the end. */
            shutdown(s->fd, SHUT_RDWR);
            close(s->fd);
            if (verbose) {
                char what[32];
                snprintf(what, sizeof what, "ended, exit %d", status);
                say(s, what);
            }
            s->pid = 0;
            continue;
        }
        struct stat st;
        if (fstat(s->fd, &st) == 0 && st.nlink == 0 && st.size == 0) {
            /* The client is gone and the shell has read all it sent:
             * nobody is left to see what runs. Again at every turn until
             * the shell has ended, for what it started meanwhile. */
            if (!s->hung_up) {
                say(s, "hung up");
                s->hung_up = 1;
            }
            kill_tree(s->pid, n);
        }
    }
}

/* A connection came, on descriptor `fd`, from `peer`. */
static void start_session(int fd, const struct sockaddr_in *peer, int max) {
    /* The sessions that ended give their place first. */
    tend();
    int active = 0;
    struct session *free_slot = NULL;
    for (int i = 0; i < MAX_SESSIONS; i++) {
        if (sessions[i].pid != 0) {
            active++;
        } else if (free_slot == NULL) {
            free_slot = &sessions[i];
        }
    }
    if (active >= max) {
        char text[64];
        snprintf(text, sizeof text, "shd: too many sessions (%d), try again later\n", max);
        turn_away(fd, text);
        if (verbose) {
            printf("shd: ");
            print_peer(peer);
            printf(" turned away: %d sessions already\n", active);
        }
        return;
    }
    int pid = spawn_shell(fd);
    if (pid < 0) {
        char text[80];
        snprintf(text, sizeof text, "shd: cannot start a shell: %s\n", strerror(-pid));
        turn_away(fd, text);
        fprintf(STDERR, "shd: cannot start a shell: %s\n", strerror(-pid));
        return;
    }
    free_slot->pid = pid;
    free_slot->fd = fd;
    free_slot->hung_up = 0;
    memcpy(&free_slot->peer, peer, sizeof *peer);
    say(free_slot, "connected");
}

int main(int argc, char **argv) {
    int port = DEFAULT_PORT;
    int max = DEFAULT_MAX;
    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "-v") == 0) {
            verbose = 1;
            continue;
        }
        if (i + 1 == argc) {
            return usage();
        }
        char *end;
        long v = strtol(argv[i + 1], &end, 10);
        if (end == argv[i + 1] || *end != 0) {
            return usage();
        }
        if (strcmp(argv[i], "-p") == 0 && v >= 1 && v <= 65535) {
            port = (int)v;
        } else if (strcmp(argv[i], "-n") == 0 && v >= 1 && v <= MAX_SESSIONS) {
            max = (int)v;
        } else {
            return usage();
        }
        i++;
    }
    int sock = socket(AF_INET, SOCK_STREAM, 0);
    if (sock < 0) {
        fprintf(STDERR, "shd: socket: %s\n", sock == -ENODEV ? "no network card" : strerror(-sock));
        return 1;
    }
    struct sockaddr_in here;
    sockaddr_set(&here, INADDR_ANY, (uint)port);
    int err = bind(sock, &here);
    if (err == 0) {
        err = listen(sock, 4);
    }
    struct timeval wait_time;
    wait_time.tv_sec = 0;
    wait_time.tv_usec = TEND_USEC;
    if (err == 0) {
        err = setsockopt(sock, SOL_SOCKET, SO_RCVTIMEO, &wait_time, sizeof wait_time);
    }
    if (err < 0) {
        fprintf(STDERR, "shd: port %d: %s\n", port, strerror(-err));
        return 1;
    }
    for (int i = 0; i < 3; i++) {
        own[i] = dup(i);
    }
    if (verbose) {
        printf("shd: listening on port %d\n", port);
    }
    for (;;) {
        struct sockaddr_in peer;
        int fd = accept(sock, &peer);
        if (fd >= 0) {
            start_session(fd, &peer, max);
        } else if (fd == -ENFILE) {
            /* The kernel's open files are all taken: the connection waits. */
            sleep(5);
        } else if (fd != -EAGAIN) {
            fprintf(STDERR, "shd: accept: %s\n", strerror(-fd));
            return 1;
        }
        tend();
    }
}
