/* tcptest: drives the kernel's TCP through the temporary tcptest system
 * call, until #85's sockets and network programs replace both
 * (docs/camelot-net.md, "Testing TCP until #85"):
 *
 *   tcptest connect ADDR PORT BYTES [TIMES]
 *       connects to ADDR:PORT, sends BYTES bytes of a fixed pattern, ends
 *       its side of the connection and reads until the other end ends its
 *       own; TIMES times (1 if left out). For each connection, one line:
 *       10.0.2.15:49152 > 10.0.2.2:8000: sent N, received N, hash H
 *       H being FNV-1a of what it received, in hex.
 *   tcptest serve PORT CONNECTIONS [BACKLOG]
 *       listens on PORT (with BACKLOG, 4 if left out), prints
 *       "listening on port PORT", then accepts CONNECTIONS connections one
 *       after the other, sending back what each sends until it ends its
 *       side, and prints a line for each:
 *       10.0.2.2:49152 > 10.0.2.15:7000: echoed N, hash H
 *
 * A connection that fails prints what it did so far, then
 * "tcptest: REASON", and tcptest exits with 1. */
#include "user.h"

/* Reads "a.b.c.d" into *addr. Returns 0, or -1 if `s` is not an address. */
static int parse_addr(const char *s, uint *addr) {
    uint a = 0;
    for (int part = 0; part < 4; part++) {
        uint n = 0;
        int digits = 0;
        while (*s >= '0' && *s <= '9' && digits < 4) {
            n = n * 10 + (uint)(*s - '0');
            s++;
            digits++;
        }
        if (digits == 0 || digits > 3 || n > 255) {
            return -1;
        }
        a = (a << 8) | n;
        if (part < 3) {
            if (*s != '.') {
                return -1;
            }
            s++;
        }
    }
    if (*s != 0) {
        return -1;
    }
    *addr = a;
    return 0;
}

/* A number from `s`, or -1 if it is not one (or is over 2^31 - 1). */
static long parse_num(const char *s) {
    char *end;
    long n = strtol(s, &end, 10);
    if (end == s || *end != 0 || n < 0 || n == LONG_MAX) {
        return -1;
    }
    return n;
}

static void print_end(uint addr, uint port) {
    printf("%u.%u.%u.%u:%u", addr >> 24, (addr >> 16) & 255, (addr >> 8) & 255, addr & 255, port);
}

static int usage(void) {
    fprintf(STDERR, "usage: tcptest connect ADDR PORT BYTES [TIMES]\n"
                    "       tcptest serve PORT CONNECTIONS [BACKLOG]\n");
    return 1;
}

static int fail(int err) {
    fprintf(STDERR, "tcptest: %s\n", err == -ENODEV ? "no network card" : strerror(-err));
    return 1;
}

static int connect_cmd(int argc, char **argv) {
    uint addr;
    long port = argc > 3 ? parse_num(argv[3]) : -1;
    long bytes = argc > 4 ? parse_num(argv[4]) : -1;
    long times = argc > 5 ? parse_num(argv[5]) : 1;
    if (argc < 5 || argc > 6 || parse_addr(argv[2], &addr) < 0 || port < 0 || bytes < 0 || times < 1) {
        return usage();
    }
    for (long i = 0; i < times; i++) {
        struct tcptest_report r;
        memset(&r, 0, sizeof r);
        int err = tcptest(TCPTEST_CONNECT, addr, (uint)port, (uint)bytes, &r);
        if (r.lport != 0) {
            print_end(r.laddr, r.lport);
            printf(" > ");
            print_end(r.raddr, r.rport);
            printf(": sent %u, received %u, hash %08x\n", r.sent, r.received, r.hash);
        }
        if (err < 0) {
            return fail(err);
        }
    }
    return 0;
}

static int serve_cmd(int argc, char **argv) {
    long port = argc > 2 ? parse_num(argv[2]) : -1;
    long count = argc > 3 ? parse_num(argv[3]) : -1;
    long backlog = argc > 4 ? parse_num(argv[4]) : 4;
    if (argc < 4 || argc > 5 || port < 0 || count < 0 || backlog < 0) {
        return usage();
    }
    int h = tcptest(TCPTEST_LISTEN, (uint)port, (uint)backlog, 0, NULL);
    if (h < 0) {
        return fail(h);
    }
    printf("listening on port %d\n", (int)port);
    for (long i = 0; i < count; i++) {
        struct tcptest_report r;
        memset(&r, 0, sizeof r);
        int err = tcptest(TCPTEST_ACCEPT, (uint)h, 0, 0, &r);
        if (r.lport != 0) {
            print_end(r.raddr, r.rport);
            printf(" > ");
            print_end(r.laddr, r.lport);
            printf(": echoed %u, hash %08x\n", r.sent, r.hash);
        }
        if (err < 0) {
            tcptest(TCPTEST_CLOSE, (uint)h, 0, 0, NULL);
            return fail(err);
        }
    }
    tcptest(TCPTEST_CLOSE, (uint)h, 0, 0, NULL);
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
