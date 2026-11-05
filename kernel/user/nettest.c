/* nettest: moves known bytes through TCP sockets, for the tests of the
 * kernel's TCP (crates/grail/tests/kernel_tcp.rs) and for trying the stack
 * by hand:
 *
 *   nettest connect ADDR PORT BYTES [TIMES]
 *       connects to ADDR:PORT, sends BYTES bytes of a fixed pattern (byte
 *       i is i ^ i >> 8 ^ i >> 16, so a byte out of place shows) in 512-byte
 *       sends, ends its side of the connection and reads until the other
 *       end ends its own; TIMES times (1 if left out). For each connection,
 *       one line:
 *       10.0.2.15:49152 > 10.0.2.2:8000: sent N, received N, hash H
 *       H being the FNV-1a hash of what it received, in hex.
 *   nettest serve PORT CONNECTIONS [BACKLOG]
 *       listens on PORT (with BACKLOG, 4 if left out), prints
 *       "listening on port PORT", then accepts CONNECTIONS connections one
 *       after the other, sending back what each sends until it ends its
 *       side, and prints a line for each:
 *       10.0.2.2:49152 > 10.0.2.15:7000: echoed N, hash H
 *
 * A connection that fails prints what it did so far, then
 * "nettest: REASON", and nettest exits with 1. */
#include "user.h"

/* Bytes per send() and recv(). */
#define CHUNK 512
#define FNV_BASIS 0x811c9dc5u

static uchar buf[CHUNK];

/* What one connection did. */
struct report {
    struct sockaddr_in here;
    struct sockaddr_in there;
    uint sent;
    uint received;
    uint hash;
};

/* A number from `s`, or -1 if it is not one (or is over 2^31 - 1). */
static long parse_num(const char *s) {
    char *end;
    long n = strtol(s, &end, 10);
    if (end == s || *end != 0 || n < 0 || n > INT_MAX || n == LONG_MAX) {
        return -1;
    }
    return n;
}

/* Prints "FROM > TO: " and then `rest`, as one write: a server and a
 * client of its own may run at once on one console, and their lines must
 * not mix. */
static void print_line(struct sockaddr_in *from, struct sockaddr_in *to, const char *rest) {
    char line[128];
    int n = snprintf(line, sizeof line, "%s:%d > ", inet_ntoa(from->sin_addr), ntohs(from->sin_port));
    n += snprintf(line + n, sizeof line - (uint)n, "%s:%d: %s", inet_ntoa(to->sin_addr), ntohs(to->sin_port), rest);
    write(STDOUT, line, (uint)n);
}

static int usage(void) {
    fprintf(STDERR, "usage: nettest connect ADDR PORT BYTES [TIMES]\n"
                    "       nettest serve PORT CONNECTIONS [BACKLOG]\n");
    return 1;
}

static int fail(int err) {
    fprintf(STDERR, "nettest: %s\n", err == -ENODEV ? "no network card" : strerror(-err));
    return 1;
}

/* FNV-1a over `n` bytes, continuing from `h`. */
static uint fnv1a(uint h, const uchar *p, int n) {
    for (int i = 0; i < n; i++) {
        h ^= p[i];
        h *= 0x01000193u;
    }
    return h;
}

/* Reads `fd` to its end into r's count and hash. Returns 0 or the error. */
static int read_to_end(int fd, struct report *r) {
    for (;;) {
        int m = recv(fd, buf, CHUNK);
        if (m <= 0) {
            return m;
        }
        r->hash = fnv1a(r->hash, buf, m);
        r->received += (uint)m;
    }
}

/* One client connection: the pattern up, everything back. */
static int connect_once(uint addr, uint port, uint bytes, struct report *r) {
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) {
        return fd;
    }
    struct sockaddr_in to;
    sockaddr_set(&to, addr, port);
    int err = connect(fd, &to);
    getsockname(fd, &r->here);
    getpeername(fd, &r->there);
    while (err == 0 && r->sent < bytes) {
        uint n = bytes - r->sent < CHUNK ? bytes - r->sent : CHUNK;
        for (uint i = 0; i < n; i++) {
            uint k = r->sent + i;
            buf[i] = (uchar)(k ^ (k >> 8) ^ (k >> 16));
        }
        int m = send(fd, buf, n);
        if (m < 0) {
            err = m;
        } else {
            r->sent += (uint)m;
        }
    }
    if (err == 0) {
        err = shutdown(fd, SHUT_WR);
    }
    if (err == 0) {
        err = read_to_end(fd, r);
    }
    close(fd);
    return err;
}

static int connect_cmd(int argc, char **argv) {
    struct in_addr addr;
    long port = argc > 3 ? parse_num(argv[3]) : -1;
    long bytes = argc > 4 ? parse_num(argv[4]) : -1;
    long times = argc > 5 ? parse_num(argv[5]) : 1;
    if (argc < 5 || argc > 6 || !inet_aton(argv[2], &addr) || port < 0 || bytes < 0 || times < 1) {
        return usage();
    }
    for (long i = 0; i < times; i++) {
        struct report r;
        memset(&r, 0, sizeof r);
        r.hash = FNV_BASIS;
        int err = connect_once(ntohl(addr.s_addr), (uint)port, (uint)bytes, &r);
        if (r.here.sin_port != 0) {
            char rest[64];
            snprintf(rest, sizeof rest, "sent %u, received %u, hash %08x\n", r.sent, r.received, r.hash);
            print_line(&r.here, &r.there, rest);
        }
        if (err < 0) {
            return fail(err);
        }
    }
    return 0;
}

/* Accepts a connection on `listener` and sends back what it sends, until
 * it ends its side; then closes it. */
static int echo_once(int listener, struct report *r) {
    int fd = accept(listener, &r->there);
    if (fd < 0) {
        return fd;
    }
    getsockname(fd, &r->here);
    int err = 0;
    for (;;) {
        int m = recv(fd, buf, CHUNK);
        if (m <= 0) {
            err = m;
            break;
        }
        r->hash = fnv1a(r->hash, buf, m);
        r->received += (uint)m;
        int w = send(fd, buf, (uint)m);
        if (w < 0) {
            err = w;
            break;
        }
        r->sent += (uint)w;
    }
    close(fd);
    return err;
}

static int serve_cmd(int argc, char **argv) {
    long port = argc > 2 ? parse_num(argv[2]) : -1;
    long count = argc > 3 ? parse_num(argv[3]) : -1;
    long backlog = argc > 4 ? parse_num(argv[4]) : 4;
    if (argc < 4 || argc > 5 || port < 1 || count < 0 || backlog < 0) {
        return usage();
    }
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) {
        return fail(fd);
    }
    struct sockaddr_in here;
    sockaddr_set(&here, INADDR_ANY, (uint)port);
    int err = bind(fd, &here);
    if (err == 0) {
        err = listen(fd, (int)backlog);
    }
    if (err < 0) {
        close(fd);
        return fail(err);
    }
    printf("listening on port %d\n", (int)port);
    for (long i = 0; i < count; i++) {
        struct report r;
        memset(&r, 0, sizeof r);
        r.hash = FNV_BASIS;
        err = echo_once(fd, &r);
        if (r.here.sin_port != 0) {
            char rest[64];
            snprintf(rest, sizeof rest, "echoed %u, hash %08x\n", r.sent, r.hash);
            print_line(&r.there, &r.here, rest);
        }
        if (err < 0) {
            close(fd);
            return fail(err);
        }
    }
    close(fd);
    return 0;
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "connect") == 0) {
        return connect_cmd(argc, argv);
    }
    if (argc > 1 && strcmp(argv[1], "serve") == 0) {
        return serve_cmd(argc, argv);
    }
    return usage();
}
