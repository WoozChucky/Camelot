/* ping HOST [N]: sends N ICMP echo requests (4 if left out) to HOST, a
 * second apart, each waiting a second for its reply, and prints a line for
 * each and a summary. HOST is an address (a.b.c.d) or a name, which the
 * resolver looks up by DNS. Uses the kernel's ICMP interface, the `ping`
 * system call (docs/camelot-syscalls.md): a program gets no raw sockets.
 *
 *   PING 10.0.2.2: 56 data bytes
 *   64 bytes from 10.0.2.2: seq=1 ttl=64 time=1.211 ms
 *   seq=2: timed out
 *   2 sent, 1 received
 *
 * Exits 0 if a reply came, else 1; also 1, having said why, if HOST can't
 * be found or nothing can be sent (no network card, no route). */
#include "user.h"

static void print_addr(uint a) {
    printf("%u.%u.%u.%u", a >> 24, (a >> 16) & 255, (a >> 8) & 255, a & 255);
}

/* What a network call's error means to the user. */
static const char *net_error(int err) {
    return err == ENODEV ? "no network card" : strerror(err);
}

int main(int argc, char **argv) {
    if (argc < 2 || argc > 3) {
        fprintf(STDERR, "usage: ping HOST [N]\n");
        return 1;
    }
    struct in_addr a;
    int err = resolve(argv[1], &a);
    if (err < 0) {
        fprintf(STDERR, "ping: %s: %s\n", argv[1], err == -ENODEV ? "no network card" : resolve_error(-err));
        return 1;
    }
    uint addr = ntohl(a.s_addr);
    int count = argc > 2 ? atoi(argv[2]) : 4;
    if (count < 1) {
        count = 1;
    }
    if (inet_addr(argv[1]) == INADDR_NONE) {
        printf("PING %s (%s): 56 data bytes\n", argv[1], inet_ntoa(a));
    } else {
        printf("PING %s: 56 data bytes\n", argv[1]);
    }
    int received = 0;
    for (int seq = 1; seq <= count; seq++) {
        struct ping_reply r;
        /* The sequence number is 16 bits on the wire: it wraps. */
        int e = ping(addr, (uint)seq & 0xffff, &r);
        if (e == 0) {
            received++;
            printf("%u bytes from ", r.len);
            print_addr(r.from);
            printf(": seq=%d ttl=%u time=%u.%03u ms\n", seq, r.ttl, r.rtt_us / 1000, r.rtt_us % 1000);
        } else if (e == -ENODEV || e == -ENETUNREACH || e == -EINVAL) {
            /* Nothing will change by trying again. */
            fprintf(STDERR, "ping: %s\n", net_error(-e));
            return 1;
        } else {
            printf("seq=%d: %s\n", seq, strerror(-e));
        }
        /* A second between requests: after a reply or an error that came
         * at once; a timeout has already waited it. */
        if (seq < count && e != -ETIMEDOUT) {
            sleep(10);
        }
    }
    printf("%d sent, %d received\n", count, received);
    return received > 0 ? 0 : 1;
}
