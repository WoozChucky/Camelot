/* ICMP echo (RFC 792; docs/camelot-net.md, "ICMP"): what ping is made of.
 * An echo message is
 *
 *   0  type (8 request, 0 reply)   1  code (0)   2  checksum (the whole message)
 *   4  identifier                  6  sequence number
 *   8  data, sent back unchanged
 *
 * This host answers every echo request to one of its own addresses (not
 * the broadcast ones: a broadcast ping would have everyone on the network
 * answer at once). Other ICMP types are taken and ignored.
 *
 * icmp_ping() is the sending side, the kernel interface behind the ping
 * system call: a process sends a request with its pid as the identifier
 * and waits in one of PING_WAITS slots until the reply with the same
 * identifier and sequence number arrives, or the timeout passes. */
#include "net.h"
#include "proc.h"
#include "string.h"
#include "trap.h"

#define ICMP_ECHO_REPLY 0
#define ICMP_ECHO_REQUEST 8

/* A process waiting for an echo reply. */
struct ping_wait {
    bool used;
    bool done; /* the reply came: `reply` holds it */
    u32 id;
    u32 seq;
    u32 sent; /* mtime's low word when the request went out */
    struct ping_reply reply;
};

/* One per thread slot is enough: a thread waits for one reply at a
 * time. */
#define PING_WAITS NTHREAD

static struct ping_wait waits[PING_WAITS];

/* Fills in the checksum of the `len`-byte ICMP message at `m`. */
static void set_checksum(u8 *m, u32 len) {
    put16(m + 2, 0);
    put16(m + 2, inet_fold(inet_sum(m, len, 0)));
}

/* Answers the echo request `req` (`len` bytes) from `src` to `dst`. */
static void echo_reply(u32 src, u32 dst, const u8 *req, u32 len) {
    struct pbuf *r = pbuf_alloc();
    if (r == NULL) {
        return;
    }
    u8 *m = r->data + IP_DATA;
    memcpy(m, req, len);
    m[0] = ICMP_ECHO_REPLY;
    set_checksum(m, len);
    /* From the address the request went to: 127.0.0.1 answers as
     * 127.0.0.1. */
    ip_output(r, dst, src, IP_PROTO_ICMP, len);
}

/* An echo reply: completes the ping waiting for it, if any. */
static void echo_done(u32 src, u32 ttl, const u8 *m, u32 len) {
    u32 now = net_now();
    u32 id = get16(m + 4);
    u32 seq = get16(m + 6);
    for (u32 i = 0; i < PING_WAITS; i++) {
        struct ping_wait *w = &waits[i];
        if (w->used && !w->done && w->id == id && w->seq == seq) {
            w->done = true;
            w->reply.from = src;
            w->reply.ttl = ttl;
            w->reply.len = len;
            w->reply.rtt_us = net_usec(now - w->sent);
            net_wakeup();
            return;
        }
    }
}

void icmp_input(struct pbuf *p, u32 src, u32 dst, u32 ttl, const u8 *m, u32 len) {
    if (len < ICMP_HLEN || inet_fold(inet_sum(m, len, 0)) != 0) {
        net_stats.rx_bad_icmp++;
    } else if (m[0] == ICMP_ECHO_REQUEST) {
        /* Not to a broadcast address, nor to a sender with no address of
         * its own to answer to. */
        if (!ip_is_broadcast(dst) && src != 0 && src != IP_BROADCAST) {
            echo_reply(src, dst, m, len);
        }
    } else if (m[0] == ICMP_ECHO_REPLY) {
        echo_done(src, ttl, m, len);
    }
    pbuf_free(p);
}

int icmp_ping(u32 dst, u32 id, u32 seq, u32 timeout, struct ping_reply *r) {
    struct ping_wait *w = NULL;
    for (u32 i = 0; i < PING_WAITS && w == NULL; i++) {
        if (!waits[i].used) {
            w = &waits[i];
        }
    }
    struct pbuf *p = w != NULL ? pbuf_alloc() : NULL;
    if (p == NULL) {
        return -ENOBUFS;
    }
    w->used = true;
    w->done = false;
    w->id = id & 0xffff;
    w->seq = seq & 0xffff;

    u8 *m = p->data + IP_DATA;
    m[0] = ICMP_ECHO_REQUEST;
    m[1] = 0;
    put16(m + 4, w->id);
    put16(m + 6, w->seq);
    /* The data: 0x08, 0x09, ... as BSD's ping fills it, so a capture shows
     * the reply's data is the request's. */
    for (u32 i = 0; i < PING_DATA; i++) {
        m[ICMP_HLEN + i] = (u8)(ICMP_HLEN + i);
    }
    set_checksum(m, ICMP_HLEN + PING_DATA);
    u32 start = ticks;
    w->sent = net_now();
    /* A request to this host comes back during the call. */
    int err = ip_output(p, 0, dst, IP_PROTO_ICMP, ICMP_HLEN + PING_DATA);
    while (err == 0 && !w->done) {
        /* A process killed while it waits gives up at once: the error is
         * never seen, the process ends on its way out of the call. */
        if (ticks - start >= timeout || proc_killed()) {
            err = -ETIMEDOUT;
            break;
        }
        net_sleep();
    }
    if (err == 0) {
        *r = w->reply;
    }
    w->used = false;
    return err;
}
