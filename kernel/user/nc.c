/* nc: netcat, the network's cat.
 *
 *   nc [-u] [-v] HOST PORT     connects to HOST (an address or a name) on PORT
 *   nc -l [-u] [-v] PORT       listens on PORT for one connection (TCP), or
 *                              for the first datagram (UDP), and talks to it
 *
 * Then it copies its input to the other end and what the other end sends
 * to its output, both at once: TCP by default, UDP with -u (each read of
 * the input is a datagram, and each datagram that comes is written out).
 * -v says on stderr what it does ("listening on port 7000", "connection
 * from 10.0.2.2:49152").
 *
 * With TCP, the end of the input ends this side of the connection (a FIN,
 * through shutdown), and nc ends once the other side has ended its own and
 * all the input is sent; but input from the console, which only ends with
 * Ctrl-D, is not waited for: `echo hello | nc host 7` sends "hello" and
 * prints the echo, and `nc -l 7000 > file` writes what a client sends to
 * `file` and ends with it. With UDP nothing says that the other side is
 * done: nc ends once its input has ended and nothing more came for a
 * second.
 *
 * There is no fork and no poll, so the two directions are two processes:
 * nc spawns itself as `nc -s` with the socket as its output and nc's own
 * input; that half reads the input and sends it, then shuts the socket
 * down for sending. The first half receives, and once the other side is
 * done kills the sending half if it is still waiting for the console (or
 * at once if the connection failed), and collects it.
 *
 * Exits 0, or 1 having said why: the host can't be found, the connection
 * is refused or reset, the port is taken. */
#include "user.h"

#define BUF 1024

static char buf[BUF];
static int verbose;

static int usage(void) {
    fprintf(STDERR, "usage: nc [-u] [-v] HOST PORT\n       nc -l [-u] [-v] PORT\n");
    return 1;
}

static int fail(const char *what, int err) {
    if (what != NULL) {
        fprintf(STDERR, "nc: %s: %s\n", what, err == ENODEV ? "no network card" : strerror(err));
    } else {
        fprintf(STDERR, "nc: %s\n", err == ENODEV ? "no network card" : strerror(err));
    }
    return 1;
}

/* A port number from `s`: 1 to 65535, else -1. */
static int parse_port(const char *s) {
    char *end;
    long n = strtol(s, &end, 10);
    if (end == s || *end != 0 || n < 1 || n > 65535) {
        return -1;
    }
    return (int)n;
}

/* The sending half (`nc -s`): copies the input to the output, the socket,
 * then ends the sending side. Exits 0, or 1 if a send failed (the
 * receiving half reports why). */
static int send_half(void) {
    for (;;) {
        int n = read(STDIN, buf, BUF);
        if (n <= 0) {
            break;
        }
        int done = 0;
        while (done < n) {
            int w = write(STDOUT, buf + done, (uint)(n - done));
            if (w <= 0) {
                return 1;
            }
            done += w;
        }
    }
    shutdown(STDOUT, SHUT_WR);
    return 0;
}

/* Starts the sending half with `sock` as its output. Returns its pid, or
 * an error. */
static int start_sender(int sock) {
    int saved = dup(STDOUT);
    if (saved < 0) {
        return saved;
    }
    close(STDOUT);
    dup(sock); /* the lowest free descriptor: 1 */
    char *argv[3];
    argv[0] = "nc";
    argv[1] = "-s";
    argv[2] = NULL;
    int pid = spawn("/bin/nc", argv, environ);
    close(STDOUT);
    dup(saved);
    close(saved);
    return pid;
}

/* Whether process `pid` has ended (or is gone): procs() says. */
static int ended(int pid) {
    struct procinfo info[16];
    int n = procs(info, 16);
    for (int i = 0; i < n && i < 16; i++) {
        if ((int)info[i].pid == pid) {
            return info[i].state == PROC_ZOMBIE;
        }
    }
    return 1;
}

/* Copies what arrives on `sock` to the output while the sending half
 * `sender` runs; returns 0, or the error that ended the connection. */
static int receive(int sock, int sender, int udp) {
    if (udp) {
        /* Datagrams never end: a second of quiet, once the input is over. */
        struct timeval second;
        second.tv_sec = 1;
        second.tv_usec = 0;
        setsockopt(sock, SOL_SOCKET, SO_RCVTIMEO, &second, sizeof second);
    }
    for (;;) {
        int n = recv(sock, buf, BUF);
        if (n == -EAGAIN) {
            if (ended(sender)) {
                return 0;
            }
            continue;
        }
        if (n == 0 && !udp) {
            return 0;
        }
        if (n < 0) {
            return n;
        }
        if (write(STDOUT, buf, (uint)n) != n) {
            return -EPIPE;
        }
    }
}

/* Talks to the other end of `sock` until both sides are done. */
static int talk(int sock, int udp) {
    int sender = start_sender(sock);
    if (sender < 0) {
        close(sock);
        return fail("spawn", -sender);
    }
    int err = receive(sock, sender, udp);
    /* The sending half may still wait for the console, or the connection
     * failed: then what is left of the input will never be sent. */
    struct stat st;
    int console = fstat(STDIN, &st) == 0 && st.type == T_DEVICE;
    if ((err < 0 || console) && !ended(sender)) {
        kill(sender);
    }
    wait(sender, NULL);
    close(sock);
    if (err < 0) {
        return fail(NULL, -err);
    }
    return 0;
}

static void say_peer(const char *what, struct sockaddr_in *a) {
    if (verbose) {
        fprintf(STDERR, "%s %s:%d\n", what, inet_ntoa(a->sin_addr), ntohs(a->sin_port));
    }
}

static int client(const char *host, int port, int udp) {
    struct in_addr addr;
    int err = resolve(host, &addr);
    if (err < 0) {
        fprintf(STDERR, "nc: %s: %s\n", host, err == -ENODEV ? "no network card" : resolve_error(-err));
        return 1;
    }
    int sock = socket(AF_INET, udp ? SOCK_DGRAM : SOCK_STREAM, 0);
    if (sock < 0) {
        return fail("socket", -sock);
    }
    struct sockaddr_in to;
    sockaddr_set(&to, ntohl(addr.s_addr), (uint)port);
    err = connect(sock, &to);
    if (err < 0) {
        close(sock);
        return fail(host, -err);
    }
    say_peer("connected to", &to);
    return talk(sock, udp);
}

static int server(int port, int udp) {
    int sock = socket(AF_INET, udp ? SOCK_DGRAM : SOCK_STREAM, 0);
    if (sock < 0) {
        return fail("socket", -sock);
    }
    struct sockaddr_in here;
    sockaddr_set(&here, INADDR_ANY, (uint)port);
    int err = bind(sock, &here);
    if (err == 0 && !udp) {
        err = listen(sock, 1);
    }
    if (err < 0) {
        close(sock);
        return fail("bind", -err);
    }
    if (verbose) {
        fprintf(STDERR, "listening on port %d\n", port);
    }
    struct sockaddr_in peer;
    if (udp) {
        /* The first datagram says who the other end is. */
        int n = recvfrom(sock, buf, BUF, &peer);
        if (n < 0) {
            close(sock);
            return fail(NULL, -n);
        }
        say_peer("datagram from", &peer);
        write(STDOUT, buf, (uint)n);
        err = connect(sock, &peer);
        if (err < 0) {
            close(sock);
            return fail(NULL, -err);
        }
        return talk(sock, udp);
    }
    int conn = accept(sock, &peer);
    close(sock);
    if (conn < 0) {
        return fail("accept", -conn);
    }
    say_peer("connection from", &peer);
    return talk(conn, udp);
}

int main(int argc, char **argv) {
    int listen_mode = 0;
    int udp = 0;
    int i = 1;
    for (; i < argc && argv[i][0] == '-' && argv[i][1] != 0; i++) {
        for (const char *f = argv[i] + 1; *f != 0; f++) {
            if (*f == 'l') {
                listen_mode = 1;
            } else if (*f == 'u') {
                udp = 1;
            } else if (*f == 'v') {
                verbose = 1;
            } else if (*f == 's' && argc == 2) {
                return send_half();
            } else {
                return usage();
            }
        }
    }
    if (listen_mode) {
        if (argc - i != 1) {
            return usage();
        }
        int port = parse_port(argv[i]);
        return port < 0 ? usage() : server(port, udp);
    }
    if (argc - i != 2) {
        return usage();
    }
    int port = parse_port(argv[i + 1]);
    return port < 0 ? usage() : client(argv[i], port, udp);
}
