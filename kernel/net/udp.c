/* UDP (RFC 768; docs/camelot-net.md, "UDP"), and the kernel-internal
 * socket API that DHCP uses now and the socket system calls (#85) will put
 * behind file descriptors. A datagram is an 8-byte header and its data:
 *
 *   0  source port   2  destination port   4  length (header included)
 *   6  checksum, over a pseudo-header (source and destination addresses,
 *      protocol 17, the length) and the whole datagram; 0 means none
 *
 * Received datagrams are checked (a length that doesn't fit the packet, a
 * wrong checksum: dropped and counted) and queued, in their buffers, on the
 * socket bound to their destination port, which holds UDP_QUEUE_MAX at most;
 * a process reading the socket sleeps until one comes. Sent ones always
 * carry a checksum. A socket is bound to a port, not to an address: it
 * gets what comes to that port on any of the host's addresses. */
#include "net.h"
#include "proc.h"
#include "string.h"
#include "trap.h"

static struct udp_sock socks[UDP_SOCKETS];
/* Where udp_open(0) looks for a free port next. */
static u32 next_ephemeral = UDP_EPHEMERAL_FIRST;

static struct udp_sock *find(u32 port) {
    for (u32 i = 0; i < UDP_SOCKETS; i++) {
        if (socks[i].used && socks[i].port == port) {
            return &socks[i];
        }
    }
    return NULL;
}

struct udp_sock *udp_open(u32 port, int *err) {
    if (port > 0xffff) {
        *err = -EINVAL;
        return NULL;
    }
    if (port == 0) {
        /* The next free port, wrapping round within the ephemeral range;
         * there are far more ports than sockets, so one is free. */
        do {
            port = next_ephemeral;
            next_ephemeral = next_ephemeral == 0xffff ? UDP_EPHEMERAL_FIRST : next_ephemeral + 1;
        } while (find(port) != NULL);
    } else if (find(port) != NULL) {
        *err = -EADDRINUSE;
        return NULL;
    }
    for (u32 i = 0; i < UDP_SOCKETS; i++) {
        struct udp_sock *s = &socks[i];
        if (!s->used) {
            s->used = true;
            s->port = (u16)port;
            s->queue.head = NULL;
            s->queue.tail = NULL;
            s->queue.len = 0;
            return s;
        }
    }
    *err = -ENOBUFS;
    return NULL;
}

void udp_close(struct udp_sock *s) {
    pbuf_drain(&s->queue);
    s->used = false;
}

/* The checksum of the `len`-byte datagram at `u`, from `src` to `dst`:
 * the pseudo-header, then the datagram itself. */
static u32 checksum(u32 src, u32 dst, const u8 *u, u32 len) {
    u32 sum = (src >> 16) + (src & 0xffff) + (dst >> 16) + (dst & 0xffff) + IP_PROTO_UDP + len;
    return inet_fold(inet_sum(u, len, sum));
}

/* Sends the `len` bytes at p->data + UDP_DATA from port `sport` to
 * `dst`:`dport`, from `src` (0: as ip_output() chooses). Takes `p`. */
static int output(struct pbuf *p, u32 src, u32 sport, u32 dst, u32 dport, u32 len) {
    u8 *u = p->data + IP_DATA;
    u32 ulen = UDP_HLEN + len;
    if (src == 0) {
        src = ip_source(dst);
    }
    put16(u, sport);
    put16(u + 2, dport);
    put16(u + 4, ulen);
    put16(u + 6, 0);
    u32 sum = checksum(src, dst, u, ulen);
    /* A sum of 0 is sent as 0xffff, its other spelling: 0 means "none". */
    put16(u + 6, sum == 0 ? 0xffff : sum);
    return ip_output(p, src, dst, IP_PROTO_UDP, ulen);
}

int udp_sendto(struct udp_sock *s, u32 dst, u32 port, const void *buf, u32 len) {
    if (len > UDP_MAX_DATA) {
        return -EMSGSIZE;
    }
    struct pbuf *p = pbuf_alloc();
    if (p == NULL) {
        return -ENOBUFS;
    }
    memcpy(p->data + UDP_DATA, buf, len);
    int err = output(p, 0, s->port, dst, port, len);
    return err < 0 ? err : (int)len;
}

int udp_recvfrom(struct udp_sock *s, void *buf, u32 size, u32 *from, u32 *port, u32 timeout) {
    u32 start = ticks;
    for (;;) {
        struct pbuf *p = pbuf_pop(&s->queue);
        if (p != NULL) {
            u32 len = p->len - p->off;
            memcpy(buf, p->data + p->off, len < size ? len : size);
            if (from != NULL) {
                *from = p->addr;
            }
            if (port != NULL) {
                *port = p->port;
            }
            pbuf_free(p);
            return (int)len;
        }
        /* A killed process gives up at once (it ends on its way out of the
         * call, never seeing the error); a kernel process is never killed. */
        if ((timeout != 0 && ticks - start >= timeout) || proc_killed()) {
            return -ETIMEDOUT;
        }
        net_sleep();
    }
}

/* The echo service: sends the datagram (`len` bytes of data at `data`)
 * back to `src`:`sport`, from the address it came to. */
static void echo(u32 src, u32 sport, u32 dst, const u8 *data, u32 len) {
    struct pbuf *r = pbuf_alloc();
    if (r == NULL) {
        return;
    }
    memcpy(r->data + UDP_DATA, data, len);
    output(r, dst, UDP_ECHO_PORT, src, sport, len);
}

void udp_input(struct pbuf *p, u32 src, u32 dst, const u8 *u, u32 len) {
    u32 ulen = len >= UDP_HLEN ? get16(u + 4) : 0;
    if (len < UDP_HLEN || ulen < UDP_HLEN || ulen > len) {
        net_stats.rx_bad_udp++;
        pbuf_free(p);
        return;
    }
    if (get16(u + 6) != 0 && checksum(src, dst, u, ulen) != 0) {
        net_stats.rx_bad_udp_sum++;
        pbuf_free(p);
        return;
    }
    u32 sport = get16(u);
    u32 dport = get16(u + 2);
    struct udp_sock *s = find(dport);
    if (s != NULL) {
        if (s->queue.len >= UDP_QUEUE_MAX) {
            net_stats.rx_queue_full++;
            pbuf_free(p);
            return;
        }
        /* The buffer itself waits in the socket: its data runs from off to
         * len. */
        p->off = (u32)((uintptr)u - (uintptr)p->data) + UDP_HLEN;
        p->len = p->off + (ulen - UDP_HLEN);
        p->addr = src;
        p->port = (u16)sport;
        pbuf_push(&s->queue, p);
        net_wakeup();
        return;
    }
    if (dport == UDP_ECHO_PORT && sport >= 1024 && !ip_is_broadcast(dst) && src != 0) {
        echo(src, sport, dst, u + UDP_HLEN, ulen - UDP_HLEN);
    } else {
        net_stats.rx_no_port++;
    }
    pbuf_free(p);
}
