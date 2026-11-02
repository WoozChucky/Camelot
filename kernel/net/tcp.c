/* TCP (RFC 9293; docs/camelot-net.md, "TCP"): connections, and the
 * kernel-internal socket API that #85's socket system calls put behind file
 * descriptors. A segment is a header of 20 bytes or more and its data:
 *
 *   0  source port        2  destination port
 *   4  sequence number    8  acknowledgment number
 *  12  header length in words (high 4 bits)   13  flags (FIN SYN RST PSH ACK)
 *  14  window            16  checksum (pseudo-header and segment, as UDP's)
 *  18  urgent pointer    20  options: this stack sends and reads only the
 *                            MSS option (kind 2, length 4), on SYNs
 *
 * Every byte of a connection has a sequence number, and so do its SYN and
 * its FIN; an acknowledgment number is the next byte the sender expects, so
 * it acknowledges everything before it. Numbers wrap at 2^32: they are
 * compared only through their difference (seq_lt and friends).
 *
 * What this stack does, and what it leaves out (the doc says why):
 *
 * - The state machine of RFC 9293 3.3.2, with RFC 5961's defenses against
 *   blind resets and SYNs: a reset is believed only at exactly the next
 *   sequence number expected (one elsewhere in the window gets a "challenge"
 *   ACK), and a SYN on a synchronized connection gets one too.
 * - Fixed buffers: a page each way per connection. The send buffer keeps
 *   what was sent until it is acknowledged; the receive buffer keeps what
 *   came in order until it is read, and the window offered is its free
 *   room, so a peer can never send more than fits.
 * - Segments that arrive out of order are dropped (the next one expected is
 *   acknowledged again, and the peer resends): no reassembly queue. No SACK,
 *   no window scaling, no urgent data, no delayed ACKs (every segment with
 *   data is acknowledged at once), no slow start or congestion window: the
 *   4 KiB send buffer is under three segments, so what a congestion window
 *   would limit hardly ever exists.
 * - Retransmission (RFC 6298): the timeout follows the round trip measured
 *   in timer ticks on one segment at a time (none that was sent twice, Karn's
 *   rule), doubles at each timeout, and after too many the connection is
 *   given up. A timeout sends everything again from the oldest byte not
 *   acknowledged ("go back N"), which is what a receiver that drops what
 *   arrives out of order needs. A zero window is probed with one byte on
 *   the same timer.
 * - Initial sequence numbers as RFC 6528 says: a clock that ticks every 4
 *   microseconds plus a keyed hash of the connection's addresses and ports.
 *
 * Everything runs with interrupts off, like the rest of the stack: a
 * segment is handled within the card's interrupt (or within the loopback
 * of the call that sent it), and a process waiting for a connection, for
 * data or for room sleeps in net_sleep(), woken by net_wakeup() or the next
 * tick, and checks again. A loopback segment is handled inside the
 * ip_output() of the one that caused it, so output() may find its socket
 * changed, even closed, after each segment it sends, and checks. */
#include "kalloc.h"
#include "net.h"
#include "proc.h"
#include "string.h"
#include "timer.h"
#include "trap.h"

#define MIN(a, b) ((a) < (b) ? (a) : (b))
#define MAX(a, b) ((a) > (b) ? (a) : (b))

/* The header's flags. */
#define TH_FIN 0x01
#define TH_SYN 0x02
#define TH_RST 0x04
#define TH_PSH 0x08
#define TH_ACK 0x10

/* The MSS option: kind 2, length 4, the size. */
#define OPT_MSS 2
#define OPT_MSS_LEN 4

static struct tcp_sock tcp_socks[TCP_SOCKETS];
u32 tcp_in_use;
/* Creation order, for accept(). */
static u32 tcp_born;
/* Where the next ephemeral port is looked for. */
static u32 tcp_next_port = TCP_EPHEMERAL_FIRST;

/* ---- sequence numbers ---- */

static bool seq_lt(u32 a, u32 b) {
    return (i32)(a - b) < 0;
}

static bool seq_le(u32 a, u32 b) {
    return (i32)(a - b) <= 0;
}

static bool seq_gt(u32 a, u32 b) {
    return (i32)(a - b) > 0;
}

static bool seq_ge(u32 a, u32 b) {
    return (i32)(a - b) >= 0;
}

/* Initial sequence numbers (RFC 6528): M + F(addresses, ports, key), M a
 * clock ticking every 4 microseconds, F a keyed hash. The key is made from
 * the clock and the card's MAC address when the first connection needs
 * it. Grail without --clock runs the same way every time, so its numbers do
 * too, but they are never 0, never one more than the last, and differ for
 * every connection's addresses and ports. The hash mixes with MurmurHash3's
 * finalizer: well spread, not cryptographic. */
static u32 isn_key[4];
static bool isn_keyed;

static u32 mix32(u32 h) {
    h ^= h >> 16;
    h *= 0x85ebca6bu;
    h ^= h >> 13;
    h *= 0xc2b2ae35u;
    h ^= h >> 16;
    return h;
}

static u32 isn(u32 laddr, u32 lport, u32 raddr, u32 rport) {
    if (!isn_keyed) {
        u32 seed = net_now() ^ (ticks << 16);
        for (u32 i = 0; i < 4; i++) {
            seed = mix32(seed + 0x9e3779b9u * (i + 1) + ((u32)netif.mac[i + 2] << (8 * i)));
            isn_key[i] = seed;
        }
        isn_keyed = true;
    }
    u32 h = mix32(isn_key[0] ^ laddr);
    h = mix32(h ^ isn_key[1] ^ raddr);
    h = mix32(h ^ isn_key[2] ^ ((lport << 16) | rport));
    h ^= isn_key[3];
    u32 per = timebase_hz() / 250000; /* mtime ticks in 4 microseconds */
    if (per == 0) {
        per = 1;
    }
    return h + net_now() / per;
}

/* ---- buffers ---- */

/* Copies `n` bytes from `src` (in `pt`'s user memory, or kernel memory if
 * `pt` is NULL) into the ring `ring` from index `at` on. Returns 0, or
 * -EFAULT. */
static int ring_in(u8 *ring, u32 at, pagetable_t pt, u32 src, u32 n) {
    while (n > 0) {
        u32 m = MIN(n, TCP_BUF - at);
        if (pt == NULL) {
            memcpy(ring + at, (const void *)(uintptr)src, m);
        } else if (copyin(pt, ring + at, src, m) < 0) {
            return -EFAULT;
        }
        src += m;
        n -= m;
        at = (at + m) % TCP_BUF;
    }
    return 0;
}

/* Copies `n` bytes of the ring from index `at` on to `dst`, the same
 * way. */
static int ring_out(const u8 *ring, u32 at, pagetable_t pt, u32 dst, u32 n) {
    while (n > 0) {
        u32 m = MIN(n, TCP_BUF - at);
        if (pt == NULL) {
            memcpy((void *)(uintptr)dst, ring + at, m);
        } else if (copyout(pt, dst, ring + at, m) < 0) {
            return -EFAULT;
        }
        dst += m;
        n -= m;
        at = (at + m) % TCP_BUF;
    }
    return 0;
}

/* Room left in the receive buffer: the window to offer. */
static u32 rcv_space(const struct tcp_sock *s) {
    return s->rbuf != NULL ? TCP_BUF - s->rlen : 0;
}

static bool get_buffers(struct tcp_sock *s) {
    s->sbuf = kalloc();
    s->rbuf = kalloc();
    if (s->sbuf == NULL || s->rbuf == NULL) {
        if (s->sbuf != NULL) {
            kfree(s->sbuf);
        }
        if (s->rbuf != NULL) {
            kfree(s->rbuf);
        }
        s->sbuf = NULL;
        s->rbuf = NULL;
        return false;
    }
    return true;
}

static void drop_buffers(struct tcp_sock *s) {
    if (s->sbuf != NULL) {
        kfree(s->sbuf);
        s->sbuf = NULL;
    }
    if (s->rbuf != NULL) {
        kfree(s->rbuf);
        s->rbuf = NULL;
    }
    s->slen = 0;
    s->rlen = 0;
}

/* ---- sockets ---- */

static void release(struct tcp_sock *s) {
    drop_buffers(s);
    s->used = false;
    s->state = TCP_CLOSED;
    tcp_in_use--;
}

/* A free slot, cleared. When every one is taken, the TIME-WAIT connection
 * nearest its end is given up for it: TIME-WAIT only guards against old
 * duplicates of a connection that is over, and a slot is worth more. */
static struct tcp_sock *alloc(void) {
    struct tcp_sock *s = NULL;
    for (u32 i = 0; i < TCP_SOCKETS && s == NULL; i++) {
        if (!tcp_socks[i].used) {
            s = &tcp_socks[i];
        }
    }
    if (s == NULL) {
        for (u32 i = 0; i < TCP_SOCKETS; i++) {
            struct tcp_sock *t = &tcp_socks[i];
            if (t->state == TCP_TIME_WAIT_STATE && !t->owned && (s == NULL || t->linger < s->linger)) {
                s = t;
            }
        }
        if (s == NULL) {
            return NULL;
        }
        release(s);
    }
    memset(s, 0, sizeof *s);
    s->used = true;
    s->born = ++tcp_born;
    s->rto = TCP_RTO_INITIAL;
    s->mss = TCP_DEFAULT_MSS;
    tcp_in_use++;
    return s;
}

/* Whether any socket has local port `port`. */
static bool port_used(u32 port) {
    for (u32 i = 0; i < TCP_SOCKETS; i++) {
        if (tcp_socks[i].used && tcp_socks[i].lport == port) {
            return true;
        }
    }
    return false;
}

/* The next free ephemeral port, wrapping round within the range; there are
 * far more ports than sockets, so one is free. */
static u32 ephemeral_port(void) {
    u32 port;
    do {
        port = tcp_next_port;
        tcp_next_port = tcp_next_port == 0xffff ? TCP_EPHEMERAL_FIRST : tcp_next_port + 1;
    } while (port_used(port));
    return port;
}

/* The socket a segment from `raddr`:`rport` to `laddr`:`lport` is for: its
 * connection, or else a listener on the port. */
static struct tcp_sock *lookup(u32 laddr, u32 lport, u32 raddr, u32 rport) {
    struct tcp_sock *l = NULL;
    for (u32 i = 0; i < TCP_SOCKETS; i++) {
        struct tcp_sock *s = &tcp_socks[i];
        if (!s->used || s->lport != lport) {
            continue;
        }
        if (s->state == TCP_LISTEN) {
            if (s->laddr == 0 || s->laddr == laddr) {
                l = s;
            }
        } else if (s->state != TCP_CLOSED && s->laddr == laddr && s->raddr == raddr && s->rport == rport) {
            return s;
        }
    }
    return l;
}

/* ---- sending segments ---- */

/* The checksum of the `len`-byte segment at `t` from `src` to `dst`: the
 * pseudo-header (addresses, protocol 6, length), then the segment. */
static u32 checksum(u32 src, u32 dst, const u8 *t, u32 len) {
    u32 sum = (src >> 16) + (src & 0xffff) + (dst >> 16) + (dst & 0xffff) + IP_PROTO_TCP + len;
    return inet_fold(inet_sum(t, len, sum));
}

/* Sends one segment from `src`:`sport` to `dst`:`dport`, with `len` bytes
 * of the ring `ring` from index `at` (none if `ring` is NULL), and the MSS
 * option if `mss`. Returns ip_output()'s result. */
static int xmit(u32 src, u32 sport, u32 dst, u32 dport, u32 seq, u32 ack, u32 flags, u32 wnd, bool mss,
                const u8 *ring, u32 at, u32 len) {
    struct pbuf *p = pbuf_alloc();
    if (p == NULL) {
        return -ENOBUFS;
    }
    u8 *t = p->data + IP_DATA;
    u32 hlen = mss ? TCP_HLEN + OPT_MSS_LEN : TCP_HLEN;
    put16(t, sport);
    put16(t + 2, dport);
    put32(t + 4, seq);
    put32(t + 8, ack);
    t[12] = (u8)((hlen / 4) << 4);
    t[13] = (u8)flags;
    put16(t + 14, wnd);
    put16(t + 16, 0);
    put16(t + 18, 0);
    if (mss) {
        t[20] = OPT_MSS;
        t[21] = OPT_MSS_LEN;
        put16(t + 22, TCP_MSS);
    }
    if (len > 0) {
        ring_out(ring, at, NULL, (u32)(uintptr)(t + hlen), len);
    }
    if (src == 0) {
        src = ip_source(dst);
    }
    put16(t + 16, checksum(src, dst, t, hlen + len));
    return ip_output(p, src, dst, IP_PROTO_TCP, hlen + len);
}

/* Sends a segment of connection `s`: `flags` (with ACK, the acknowledgment
 * of everything received), sequence number `seq`, and the `len` bytes of
 * the send buffer that start at `seq`. Every segment offers the window. */
static int send_seg(struct tcp_sock *s, u32 flags, u32 seq, u32 len) {
    u32 wnd = rcv_space(s);
    u32 ack = 0;
    if (flags & TH_ACK) {
        ack = s->rcv_nxt;
        s->ack_now = false;
        s->rcv_adv = s->rcv_nxt + wnd;
    }
    u32 at = len > 0 ? (s->shead + (seq - s->sseq)) % TCP_BUF : 0;
    return xmit(s->laddr, s->lport, s->raddr, s->rport, seq, ack, flags, wnd, (flags & TH_SYN) != 0, s->sbuf, at,
                len);
}

static void send_ack(struct tcp_sock *s) {
    send_seg(s, TH_ACK, s->snd_nxt, 0);
}

/* A reset for connection `s`, as an abort sends it (RFC 9293 3.10.4). */
static void send_rst(struct tcp_sock *s) {
    xmit(s->laddr, s->lport, s->raddr, s->rport, s->snd_nxt, 0, TH_RST, 0, false, NULL, 0, 0);
}

/* Starts the retransmission timer unless it runs (the persist timer gives
 * way to it: something is in flight now). */
static void arm(struct tcp_sock *s) {
    if (s->retx == 0 || s->persist) {
        s->persist = false;
        s->retx = s->rto;
    }
}

/* Whether `s` may send data and a FIN in its state. */
static bool sending_state(u32 state) {
    return state == TCP_ESTABLISHED || state == TCP_CLOSE_WAIT || state == TCP_FIN_WAIT_1 ||
           state == TCP_CLOSING || state == TCP_LAST_ACK;
}

/* Sends what can be sent: the SYN (or SYN-ACK) if it is due, else the data
 * the peer's window allows, in segments of at most its MSS, then the FIN if
 * the owner is done and everything before it is out. A short segment goes
 * out only if it is the last of the data, or at least half the largest
 * window the peer offered (sender-side silly window avoidance, RFC 9293
 * 3.8.6.2.1); `force` lets one through anyway, and one byte past a zero
 * window: the persist timer's probe. */
static void output(struct tcp_sock *s, bool force) {
    if (s->state == TCP_SYN_SENT || s->state == TCP_SYN_RECEIVED) {
        if (s->snd_nxt == s->iss) {
            s->snd_nxt = s->iss + 1;
            if (seq_lt(s->snd_max, s->snd_nxt)) {
                s->snd_max = s->snd_nxt;
                s->timing = true;
                s->rtt_seq = s->snd_nxt;
                s->rtt_start = ticks;
            }
            arm(s);
            send_seg(s, s->state == TCP_SYN_SENT ? TH_SYN : TH_SYN | TH_ACK, s->iss, 0);
        }
        return;
    }
    while (s->used && sending_state(s->state)) {
        u32 end = s->sseq + s->slen;
        u32 wnd = force ? MAX(s->snd_wnd, 1) : s->snd_wnd;
        u32 wnd_end = s->snd_una + wnd;
        u32 seq = s->snd_nxt;
        u32 len = 0;
        bool fin = false;
        if (seq_lt(seq, end) && seq_lt(seq, wnd_end)) {
            len = MIN(MIN(end - seq, wnd_end - seq), s->mss);
            if (len < s->mss && seq + len != end && len < s->max_wnd / 2 && !force) {
                break; /* wait for the window to open further */
            }
        }
        if (s->fin_queued && seq + len == s->fin_seq) {
            fin = true;
        }
        if (len == 0 && !fin) {
            break;
        }
        force = false;
        u32 flags = TH_ACK;
        if (len > 0 && seq + len == end) {
            flags |= TH_PSH;
        }
        if (fin) {
            flags |= TH_FIN;
        }
        s->snd_nxt = seq + len + (fin ? 1 : 0);
        if (seq_lt(s->snd_max, s->snd_nxt)) {
            /* New data: time it, unless a segment is timed already. */
            if (!s->timing && seq_ge(seq, s->snd_max)) {
                s->timing = true;
                s->rtt_seq = s->snd_nxt;
                s->rtt_start = ticks;
            }
            s->snd_max = s->snd_nxt;
        }
        arm(s);
        send_seg(s, flags, seq, len);
        /* A loopback peer has answered by now: go on from where that
         * left the connection. */
    }
    /* Data waits, nothing is in flight to bring an acknowledgment: the
     * persist timer will probe the window. */
    if (s->used && sending_state(s->state) && s->snd_nxt == s->snd_una && seq_lt(s->snd_nxt, s->sseq + s->slen) &&
        s->retx == 0) {
        s->persist = true;
        s->retx = s->rto;
    }
}

/* ---- closing ---- */

/* The connection is over: CLOSED, with `err` for its owner (a reset or a
 * timeout lose what was buffered either way). A socket nobody holds goes,
 * and a connection waiting for accept() leaves its listener's queue. */
static void closed(struct tcp_sock *s, int err) {
    s->state = TCP_CLOSED;
    s->err = err;
    s->retx = 0;
    s->linger = 0;
    s->persist = false;
    s->timing = false;
    if (err != 0) {
        s->rlen = 0;
        s->slen = 0;
    }
    net_wakeup();
    if (!s->owned) {
        if (s->listener != NULL) {
            s->listener->queued--;
            s->listener = NULL;
        }
        release(s);
    }
}

static void time_wait(struct tcp_sock *s) {
    s->state = TCP_TIME_WAIT_STATE;
    s->linger = TCP_TIME_WAIT;
    s->retx = 0;
    s->persist = false;
    /* Nothing more will be sent, or come; what was received stays for an
     * owner who has not read it. */
    if (!s->owned) {
        drop_buffers(s);
    }
}

static void queue_fin(struct tcp_sock *s) {
    s->fin_queued = true;
    s->fin_seq = s->sseq + s->slen;
}

/* ---- receiving segments ---- */

/* A segment, as tcp_input() read it. */
struct seg {
    u32 src;
    u32 dst;
    u32 sport;
    u32 dport;
    u32 seq;
    u32 ack;
    u32 flags;
    u32 wnd;
    u32 mss; /* the MSS option, or 0 */
    const u8 *data;
    u32 len;
};

/* Sequence space the segment takes: its data, its SYN and its FIN. */
static u32 seg_len(const struct seg *g) {
    return g->len + ((g->flags & TH_SYN) ? 1 : 0) + ((g->flags & TH_FIN) ? 1 : 0);
}

/* The answer to a segment for no socket (RFC 9293 3.10.7.1): a reset that
 * the sender takes as about its segment. Never one to a reset. */
static void refuse(const struct seg *g) {
    if (g->flags & TH_RST) {
        return;
    }
    if (g->flags & TH_ACK) {
        xmit(g->dst, g->dport, g->src, g->sport, g->ack, 0, TH_RST, 0, false, NULL, 0, 0);
    } else {
        xmit(g->dst, g->dport, g->src, g->sport, 0, g->seq + seg_len(g), TH_RST | TH_ACK, 0, false, NULL, 0, 0);
    }
}

/* The MSS the peer's SYN asked for, within what this host sends. */
static u32 peer_mss(const struct seg *g) {
    u32 mss = g->mss != 0 ? g->mss : TCP_DEFAULT_MSS;
    return MAX(MIN(mss, TCP_MSS), 64);
}

/* Takes the peer's window from a segment acknowledging `snd_una` or more,
 * unless an older segment than the last that set it (RFC 9293 3.10.7.4). */
static void update_window(struct tcp_sock *s, const struct seg *g) {
    if (seq_lt(s->snd_wl1, g->seq) || (s->snd_wl1 == g->seq && seq_le(s->snd_wl2, g->ack))) {
        u32 old = s->snd_wnd;
        s->snd_wnd = g->wnd;
        s->snd_wl1 = g->seq;
        s->snd_wl2 = g->ack;
        s->max_wnd = MAX(s->max_wnd, g->wnd);
        if (old == 0 && g->wnd != 0 && s->snd_nxt != s->snd_una && g->ack == s->snd_una) {
            /* The window opened while a probe's byte was out, not taken:
             * send again from snd_una, or that byte would leave a hole
             * only the retransmission timer fills. */
            s->snd_nxt = s->snd_una;
            s->retx = 0;
        }
        if (g->wnd != 0 && s->persist) {
            s->persist = false;
            s->retx = 0;
        }
    }
}

/* A round-trip sample of `r` ticks (RFC 6298 2.2 and 2.3, scaled as in its
 * appendix: srtt by 8, rttvar by 4, so 4 * RTTVAR is rttvar itself). */
static void rtt_sample(struct tcp_sock *s, u32 r) {
    i32 m = (i32)r;
    if (!s->rtt_valid) {
        s->srtt = m << 3;
        s->rttvar = m << 1;
        s->rtt_valid = true;
    } else {
        i32 delta = m - (s->srtt >> 3);
        s->srtt += delta;
        if (delta < 0) {
            delta = -delta;
        }
        s->rttvar += delta - (s->rttvar >> 2);
    }
    /* RTO = SRTT + max(G, 4 * RTTVAR), G the clock's one tick. */
    u32 rto = (u32)((s->srtt >> 3) + MAX(s->rttvar, 1));
    s->rto = MIN(MAX(rto, TCP_RTO_MIN), TCP_RTO_MAX);
}

/* An acceptable acknowledgment (snd_una < ack <= snd_max): drops what it
 * covers from the send buffer, takes a round-trip sample, and restarts or
 * stops the timer. */
static void acked(struct tcp_sock *s, u32 ack) {
    if (s->timing && seq_ge(ack, s->rtt_seq)) {
        s->timing = false;
        rtt_sample(s, ticks - s->rtt_start);
    }
    if (seq_lt(s->sseq, ack)) {
        u32 n = MIN(ack - s->sseq, s->slen);
        s->shead = (s->shead + n) % TCP_BUF;
        s->slen -= n;
        s->sseq += n;
    }
    s->snd_una = ack;
    if (seq_lt(s->snd_nxt, ack)) {
        s->snd_nxt = ack;
    }
    s->retries = 0;
    s->persist = false;
    /* Everything acknowledged stops the timer, anything less restarts it
     * (RFC 6298 5.2 and 5.3). */
    s->retx = s->snd_una == s->snd_max ? 0 : s->rto;
    net_wakeup();
}

/* Whether the FIN this end sent is acknowledged. */
static bool fin_acked(const struct tcp_sock *s) {
    return s->fin_queued && seq_gt(s->snd_una, s->fin_seq);
}

/* A segment for a listener: a SYN makes a connection in SYN-RECEIVED that
 * waits for accept() once established; anything else gets a reset, or (a
 * reset) nothing. */
static void listen_input(struct tcp_sock *l, const struct seg *g) {
    if (g->flags & TH_RST) {
        return;
    }
    if (g->flags & TH_ACK) {
        refuse(g);
        return;
    }
    if (!(g->flags & TH_SYN)) {
        return;
    }
    struct tcp_sock *c = l->queued < l->backlog ? alloc() : NULL;
    if (c != NULL && !get_buffers(c)) {
        release(c);
        c = NULL;
    }
    if (c == NULL) {
        /* The peer will send its SYN again, and may find room then. */
        net_stats.tcp_backlog++;
        return;
    }
    c->laddr = g->dst;
    c->lport = l->lport;
    c->raddr = g->src;
    c->rport = (u16)g->sport;
    c->listener = l;
    l->queued++;
    c->irs = g->seq;
    c->rcv_nxt = g->seq + 1;
    c->mss = peer_mss(g);
    c->snd_wnd = g->wnd;
    c->max_wnd = g->wnd;
    c->snd_wl1 = g->seq;
    c->iss = isn(c->laddr, c->lport, c->raddr, c->rport);
    c->snd_una = c->iss;
    c->snd_nxt = c->iss;
    c->snd_max = c->iss;
    c->sseq = c->iss + 1;
    c->state = TCP_SYN_RECEIVED;
    output(c, false);
}

/* A segment for a connection in SYN-SENT (RFC 9293 3.10.7.3). */
static void syn_sent_input(struct tcp_sock *s, const struct seg *g) {
    bool ack_ok = false;
    if (g->flags & TH_ACK) {
        if (seq_le(g->ack, s->iss) || seq_gt(g->ack, s->snd_max)) {
            if (!(g->flags & TH_RST)) {
                xmit(s->laddr, s->lport, s->raddr, s->rport, g->ack, 0, TH_RST, 0, false, NULL, 0, 0);
            }
            return;
        }
        ack_ok = true;
    }
    if (g->flags & TH_RST) {
        if (ack_ok) {
            net_stats.tcp_resets++;
            closed(s, -ECONNREFUSED);
        }
        return;
    }
    if (!(g->flags & TH_SYN)) {
        return;
    }
    s->irs = g->seq;
    s->rcv_nxt = g->seq + 1;
    s->mss = peer_mss(g);
    if (ack_ok) {
        acked(s, g->ack);
        s->state = TCP_ESTABLISHED;
        s->snd_wnd = g->wnd;
        s->max_wnd = g->wnd;
        s->snd_wl1 = g->seq;
        s->snd_wl2 = g->ack;
        /* Data on a SYN-ACK is not taken (the peer sends it again). */
        send_ack(s);
    } else {
        /* Both ends sent a SYN at once: answer with a SYN-ACK. */
        s->state = TCP_SYN_RECEIVED;
        s->snd_wnd = g->wnd;
        s->max_wnd = g->wnd;
        s->snd_wl1 = g->seq;
        s->snd_nxt = s->iss;
        output(s, false);
    }
}

/* Whether sequence number `seq` is within the window: rcv_nxt to
 * rcv_nxt + wnd - 1. */
static bool in_window(const struct tcp_sock *s, u32 seq, u32 wnd) {
    return seq_le(s->rcv_nxt, seq) && seq_lt(seq, s->rcv_nxt + wnd);
}

/* Takes the data of an acceptable segment, in order only. Returns whether
 * all of it was taken (so a FIN on it is next). */
static bool take_data(struct tcp_sock *s, const struct seg *g) {
    if (seq_gt(g->seq, s->rcv_nxt)) {
        /* A gap before it: dropped, and the ACK says what is missing. */
        net_stats.tcp_out_of_order++;
        s->ack_now = true;
        return false;
    }
    u32 skip = s->rcv_nxt - g->seq; /* bytes it has that came already */
    u32 n = MIN(g->len - skip, rcv_space(s));
    ring_in(s->rbuf, (s->rhead + s->rlen) % TCP_BUF, NULL, (u32)(uintptr)(g->data + skip), n);
    s->rlen += n;
    s->rcv_nxt += n;
    s->ack_now = true;
    if (n > 0) {
        net_wakeup();
    }
    return skip + n == g->len;
}

/* A segment for a synchronized connection, SYN-RECEIVED onwards (RFC 9293
 * 3.10.7.4, with RFC 5961's checks): sequence number, RST, SYN, ACK, data,
 * FIN, in that order. */
static void sync_input(struct tcp_sock *s, struct seg *g) {
    /* Our SYN-ACK was lost: the peer sent its SYN again. */
    if (s->state == TCP_SYN_RECEIVED && (g->flags & TH_SYN) && !(g->flags & TH_ACK) && g->seq == s->irs) {
        s->snd_nxt = s->iss;
        output(s, false);
        return;
    }

    /* 1. Is it within the window? A segment that is not gets an ACK saying
     * where we are (unless it is a reset), the answer to a duplicate. With
     * the window closed, RFC 9293 asks for "special allowance" for valid
     * ACKs: a segment at rcv_nxt (a probe), or a little past it (a peer that
     * probed with new data counts those bytes as sent, so its next segments
     * start past what this end took), still has its ACK and window looked
     * at, but its data is not taken.
     *
     * An empty segment may also sit right at the window's edge (rcv_nxt +
     * wnd, as Linux allows; RFC 9293 stops one short): a peer that filled
     * the window sends its ACKs from there, and when a segment of its was
     * lost, they would all be dropped, with the ACKs of what this end sent,
     * until the peer resent the lost one. */
    u32 wnd = rcv_space(s);
    u32 len = seg_len(g);
    bool ok;
    if (len == 0) {
        ok = seq_le(s->rcv_nxt, g->seq) && seq_le(g->seq, s->rcv_nxt + wnd);
    } else {
        ok = wnd != 0 && (in_window(s, g->seq, wnd) || in_window(s, g->seq + len - 1, wnd));
    }
    if (!ok) {
        if (wnd == 0 && seq_le(s->rcv_nxt, g->seq) && seq_le(g->seq, s->rcv_adv + s->mss) &&
            !(g->flags & (TH_RST | TH_SYN))) {
            g->len = 0;
            g->flags &= ~(u32)TH_FIN;
            /* A probe gets an ACK saying the window is still closed. */
            s->ack_now = s->ack_now || len > 0;
        } else {
            if (!(g->flags & TH_RST)) {
                if (s->state == TCP_TIME_WAIT_STATE && (g->flags & TH_FIN)) {
                    /* The peer's FIN again: our last ACK was lost. */
                    s->linger = TCP_TIME_WAIT;
                }
                send_ack(s);
            }
            return;
        }
    }

    /* 2. A reset, believed only at exactly rcv_nxt; anywhere else in the
     * window it may be forged, and gets a challenge ACK (RFC 5961 3.2): a
     * real peer answers that with a reset at the right number. */
    if (g->flags & TH_RST) {
        if (g->seq != s->rcv_nxt) {
            send_ack(s);
            return;
        }
        int err = 0;
        if (s->state == TCP_SYN_RECEIVED) {
            err = s->listener != NULL ? 0 : -ECONNREFUSED;
        } else if (s->state == TCP_ESTABLISHED || s->state == TCP_FIN_WAIT_1 || s->state == TCP_FIN_WAIT_2 ||
                   s->state == TCP_CLOSE_WAIT) {
            err = -ECONNRESET;
        }
        net_stats.tcp_resets++;
        closed(s, err);
        return;
    }

    /* 3. A SYN on a synchronized connection: a challenge ACK (RFC 5961
     * 4.2). If the peer restarted, it answers with a reset. */
    if (g->flags & TH_SYN) {
        send_ack(s);
        return;
    }

    /* 4. The acknowledgment. */
    if (!(g->flags & TH_ACK)) {
        return;
    }
    if (s->state == TCP_SYN_RECEIVED) {
        if (!(seq_lt(s->snd_una, g->ack) && seq_le(g->ack, s->snd_max))) {
            xmit(s->laddr, s->lport, s->raddr, s->rport, g->ack, 0, TH_RST, 0, false, NULL, 0, 0);
            return;
        }
        s->state = TCP_ESTABLISHED;
        s->snd_wnd = g->wnd;
        s->max_wnd = g->wnd;
        s->snd_wl1 = g->seq;
        s->snd_wl2 = g->ack;
        net_wakeup(); /* accept(), or connect() after a simultaneous open */
    }
    if (seq_gt(g->ack, s->snd_max)) {
        /* It acknowledges something never sent. */
        send_ack(s);
        return;
    }
    if (seq_le(s->snd_una, g->ack)) {
        update_window(s, g);
        if (g->ack == s->snd_una && g->wnd == 0) {
            /* The peer is there, its buffer full: not a lost segment. */
            s->retries = 0;
        }
    }
    if (seq_lt(s->snd_una, g->ack)) {
        acked(s, g->ack);
    }
    if (fin_acked(s)) {
        if (s->state == TCP_FIN_WAIT_1) {
            s->state = TCP_FIN_WAIT_2;
            if (!s->owned) {
                s->linger = TCP_FIN_TIMEOUT;
            }
        } else if (s->state == TCP_CLOSING) {
            time_wait(s);
        } else if (s->state == TCP_LAST_ACK) {
            closed(s, 0);
            return;
        }
    }

    /* 5. The data, while the peer may send any. */
    bool all_taken = true;
    if (g->len > 0) {
        if (s->state == TCP_ESTABLISHED || s->state == TCP_FIN_WAIT_1 || s->state == TCP_FIN_WAIT_2) {
            if (!s->owned && s->listener == NULL) {
                /* Its owner closed it: nobody will ever read this. */
                send_rst(s);
                closed(s, 0);
                return;
            }
            all_taken = take_data(s, g);
        } else {
            s->ack_now = true; /* after its FIN: ignored */
        }
    }

    /* 6. The FIN, once everything before it is in. */
    if ((g->flags & TH_FIN) && all_taken && g->seq + g->len == s->rcv_nxt) {
        s->rcv_nxt++;
        s->fin_rcvd = true;
        s->ack_now = true;
        net_wakeup();
        if (s->state == TCP_ESTABLISHED) {
            s->state = TCP_CLOSE_WAIT;
        } else if (s->state == TCP_FIN_WAIT_1) {
            if (fin_acked(s)) {
                time_wait(s);
            } else {
                s->state = TCP_CLOSING;
            }
        } else if (s->state == TCP_FIN_WAIT_2) {
            time_wait(s);
        }
    }

    /* What the acknowledgment or the window let go, then the ACK owed if
     * nothing sent carried it. */
    output(s, false);
    if (s->used && s->state != TCP_CLOSED && s->ack_now) {
        send_ack(s);
    }
}

void tcp_input(struct pbuf *p, u32 src, u32 dst, const u8 *t, u32 len) {
    u32 hlen = len >= TCP_HLEN ? (u32)(t[12] >> 4) * 4 : 0;
    if (len < TCP_HLEN || hlen < TCP_HLEN || hlen > len) {
        net_stats.rx_bad_tcp++;
        pbuf_free(p);
        return;
    }
    if (checksum(src, dst, t, len) != 0) {
        net_stats.rx_bad_tcp_sum++;
        pbuf_free(p);
        return;
    }
    /* TCP is between two hosts: never to a broadcast address, never from a
     * host without an address. */
    if (ip_is_broadcast(dst) || src == 0) {
        net_stats.rx_filtered++;
        pbuf_free(p);
        return;
    }
    struct seg g;
    g.src = src;
    g.dst = dst;
    g.sport = get16(t);
    g.dport = get16(t + 2);
    g.seq = get32(t + 4);
    g.ack = get32(t + 8);
    g.flags = t[13];
    g.wnd = get16(t + 14);
    g.data = t + hlen;
    g.len = len - hlen;
    g.mss = 0;
    /* The options: the MSS one is read; NOP and the end are understood; any
     * other is stepped over by its length. A malformed list ends the
     * reading, not the segment. */
    for (u32 i = TCP_HLEN; i < hlen;) {
        u32 kind = t[i];
        if (kind == 0) {
            break;
        }
        if (kind == 1) {
            i++;
            continue;
        }
        if (i + 1 >= hlen || t[i + 1] < 2 || i + t[i + 1] > hlen) {
            break;
        }
        if (kind == OPT_MSS && t[i + 1] == OPT_MSS_LEN) {
            g.mss = get16(t + i + 2);
        }
        i += t[i + 1];
    }

    struct tcp_sock *s = lookup(dst, g.dport, src, g.sport);
    if (s == NULL) {
        net_stats.tcp_no_socket++;
        refuse(&g);
    } else if (s->state == TCP_LISTEN) {
        listen_input(s, &g);
    } else if (s->state == TCP_SYN_SENT) {
        syn_sent_input(s, &g);
    } else {
        sync_input(s, &g);
    }
    pbuf_free(p);
}

/* ---- timers ---- */

/* The retransmission timer went off: send again everything from the
 * oldest byte not acknowledged (or the SYN), doubling the timeout; give up
 * after too many tries. The persist timer probes the window instead. */
static void timeout(struct tcp_sock *s) {
    if (s->persist) {
        s->persist = false;
        s->rto = MIN(s->rto * 2, TCP_RTO_MAX);
        output(s, true);
        return;
    }
    bool syn = s->state == TCP_SYN_SENT || s->state == TCP_SYN_RECEIVED;
    if (s->retries >= (syn ? TCP_SYN_RETRIES : TCP_RETRIES)) {
        /* A connection that was established hears why. */
        if (!syn) {
            send_rst(s);
        }
        closed(s, s->state == TCP_SYN_RECEIVED && s->listener != NULL ? 0 : -ETIMEDOUT);
        return;
    }
    s->retries++;
    net_stats.tcp_retransmits++;
    s->rto = MIN(s->rto * 2, TCP_RTO_MAX);
    s->timing = false; /* Karn: no sample from a segment sent twice */
    s->snd_nxt = syn ? s->iss : s->snd_una;
    output(s, false);
}

void tcp_timer(void) {
    for (u32 i = 0; i < TCP_SOCKETS; i++) {
        struct tcp_sock *s = &tcp_socks[i];
        if (!s->used) {
            continue;
        }
        if (s->linger != 0) {
            s->linger--;
            if (s->linger == 0) {
                /* TIME-WAIT is over, or the peer never closed its side of
                 * a connection nobody holds. */
                closed(s, 0);
                continue;
            }
        }
        if (s->retx != 0) {
            s->retx--;
            if (s->retx == 0) {
                timeout(s);
            }
        }
    }
}

/* ---- the API ---- */

struct tcp_sock *tcp_socket(int *err) {
    struct tcp_sock *s = alloc();
    if (s == NULL) {
        *err = -ENOBUFS;
        return NULL;
    }
    s->owned = true;
    return s;
}

int tcp_bind(struct tcp_sock *s, u32 port) {
    if (s->lport != 0 || s->state != TCP_CLOSED || s->raddr != 0 || port > 0xffff) {
        return -EINVAL;
    }
    if (port == 0) {
        s->lport = (u16)ephemeral_port();
        return 0;
    }
    /* A port is taken by a listener, or by a socket bound to it and not
     * connected yet. Connections on it (accepted ones share their
     * listener's port) are told apart by their other end, and don't stop a
     * new listener. */
    for (u32 i = 0; i < TCP_SOCKETS; i++) {
        struct tcp_sock *t = &tcp_socks[i];
        if (t->used && t != s && t->lport == port &&
            (t->state == TCP_LISTEN || (t->state == TCP_CLOSED && t->owned && t->raddr == 0))) {
            return -EADDRINUSE;
        }
    }
    s->lport = (u16)port;
    return 0;
}

int tcp_listen(struct tcp_sock *s, u32 backlog) {
    if (s->state != TCP_CLOSED || s->lport == 0 || s->raddr != 0) {
        return -EINVAL;
    }
    s->backlog = MAX(MIN(backlog, TCP_BACKLOG_MAX), 1);
    s->state = TCP_LISTEN;
    return 0;
}

int tcp_connect(struct tcp_sock *s, u32 addr, u32 port) {
    if (s->state == TCP_LISTEN) {
        return -EINVAL;
    }
    if (s->state != TCP_CLOSED || s->raddr != 0) {
        return -EISCONN;
    }
    if (port == 0 || port > 0xffff || addr == 0 || ip_is_broadcast(addr) || (addr >> 28) >= 0xe) {
        return -EINVAL;
    }
    u32 src = ip_source(addr);
    if (src == 0) {
        return -ENETUNREACH;
    }
    if (!get_buffers(s)) {
        return -ENOBUFS;
    }
    if (s->lport == 0) {
        s->lport = (u16)ephemeral_port();
    }
    s->laddr = src;
    s->raddr = addr;
    s->rport = (u16)port;
    s->iss = isn(s->laddr, s->lport, s->raddr, s->rport);
    s->snd_una = s->iss;
    s->snd_nxt = s->iss;
    s->snd_max = s->iss;
    s->sseq = s->iss + 1;
    s->state = TCP_SYN_SENT;
    /* The SYN, by hand rather than through output(), to see a route
     * missing at once. */
    s->snd_nxt = s->iss + 1;
    s->snd_max = s->snd_nxt;
    s->timing = true;
    s->rtt_seq = s->snd_nxt;
    s->rtt_start = ticks;
    arm(s);
    int err = send_seg(s, TH_SYN, s->iss, 0);
    if (err == -ENETUNREACH && s->state == TCP_SYN_SENT) {
        closed(s, err);
        return err;
    }
    while (s->state == TCP_SYN_SENT || s->state == TCP_SYN_RECEIVED) {
        if (proc_killed()) {
            return -EINTR;
        }
        net_sleep();
    }
    if (s->state == TCP_CLOSED) {
        return s->err != 0 ? s->err : -ECONNRESET;
    }
    return 0;
}

struct tcp_sock *tcp_accept(struct tcp_sock *l, int *err) {
    if (l->state != TCP_LISTEN) {
        *err = -EINVAL;
        return NULL;
    }
    for (;;) {
        struct tcp_sock *best = NULL;
        for (u32 i = 0; i < TCP_SOCKETS; i++) {
            struct tcp_sock *c = &tcp_socks[i];
            if (c->used && c->listener == l && c->state != TCP_SYN_RECEIVED && (best == NULL || c->born < best->born)) {
                best = c;
            }
        }
        if (best != NULL) {
            best->listener = NULL;
            best->owned = true;
            l->queued--;
            return best;
        }
        if (proc_killed()) {
            *err = -EINTR;
            return NULL;
        }
        net_sleep();
    }
}

/* -ENOTCONN, or the error that ended the connection. */
static int not_connected(const struct tcp_sock *s) {
    return s->state == TCP_CLOSED && s->err != 0 ? s->err : -ENOTCONN;
}

int tcp_send(struct tcp_sock *s, pagetable_t pt, u32 src, u32 n) {
    u32 done = 0;
    while (done < n) {
        if (s->fin_queued) {
            return done > 0 ? (int)done : -EPIPE;
        }
        if (s->state != TCP_ESTABLISHED && s->state != TCP_CLOSE_WAIT) {
            return done > 0 ? (int)done : not_connected(s);
        }
        u32 room = TCP_BUF - s->slen;
        if (room == 0) {
            if (proc_killed()) {
                return done > 0 ? (int)done : -EINTR;
            }
            net_sleep();
            continue;
        }
        u32 m = MIN(room, n - done);
        if (ring_in(s->sbuf, (s->shead + s->slen) % TCP_BUF, pt, src + done, m) < 0) {
            return done > 0 ? (int)done : -EFAULT;
        }
        s->slen += m;
        done += m;
        output(s, false);
    }
    return (int)done;
}

/* After the owner read: tells the peer the window opened, if it did by a
 * segment's worth or half the buffer (receiver-side silly window
 * avoidance, RFC 9293 3.8.6.2.2). */
static void window_update(struct tcp_sock *s) {
    if (s->state != TCP_ESTABLISHED && s->state != TCP_FIN_WAIT_1 && s->state != TCP_FIN_WAIT_2) {
        return;
    }
    u32 edge = s->rcv_nxt + rcv_space(s);
    if (seq_ge(edge, s->rcv_adv + MIN(TCP_BUF / 2, s->mss))) {
        send_ack(s);
    }
}

int tcp_recv(struct tcp_sock *s, pagetable_t pt, u32 dst, u32 n) {
    for (;;) {
        if (s->rlen > 0) {
            u32 m = MIN(n, s->rlen);
            if (ring_out(s->rbuf, s->rhead, pt, dst, m) < 0) {
                return -EFAULT;
            }
            s->rhead = (s->rhead + m) % TCP_BUF;
            s->rlen -= m;
            window_update(s);
            return (int)m;
        }
        if (s->fin_rcvd) {
            return 0;
        }
        if (s->state == TCP_CLOSED || s->state == TCP_LISTEN || s->state == TCP_SYN_SENT) {
            return not_connected(s);
        }
        if (proc_killed()) {
            return -EINTR;
        }
        net_sleep();
    }
}

int tcp_shutdown(struct tcp_sock *s) {
    if (s->state == TCP_ESTABLISHED) {
        queue_fin(s);
        s->state = TCP_FIN_WAIT_1;
    } else if (s->state == TCP_CLOSE_WAIT) {
        queue_fin(s);
        s->state = TCP_LAST_ACK;
    } else if (s->fin_queued && s->state != TCP_CLOSED) {
        return 0; /* done already */
    } else {
        return not_connected(s);
    }
    output(s, false);
    return 0;
}

void tcp_close(struct tcp_sock *s) {
    if (s->state == TCP_LISTEN) {
        /* The connections nobody accepted: reset. */
        for (u32 i = 0; i < TCP_SOCKETS; i++) {
            struct tcp_sock *c = &tcp_socks[i];
            if (c->used && c->listener == s) {
                send_rst(c);
                c->listener = NULL;
                release(c);
            }
        }
        release(s);
        return;
    }
    if (s->state == TCP_CLOSED || s->state == TCP_SYN_SENT) {
        release(s);
        return;
    }
    bool open = s->state == TCP_SYN_RECEIVED || s->state == TCP_ESTABLISHED || s->state == TCP_CLOSE_WAIT ||
                s->state == TCP_FIN_WAIT_1 || s->state == TCP_FIN_WAIT_2;
    if (open && (s->rlen > 0 || s->state == TCP_SYN_RECEIVED)) {
        /* Data that will never be read: the peer must not think it was. */
        send_rst(s);
        release(s);
        return;
    }
    s->owned = false;
    if (s->state == TCP_ESTABLISHED || s->state == TCP_CLOSE_WAIT) {
        tcp_shutdown(s);
    } else if (s->state == TCP_FIN_WAIT_2) {
        s->linger = TCP_FIN_TIMEOUT;
    } else if (s->state == TCP_TIME_WAIT_STATE) {
        drop_buffers(s);
    }
}

void tcp_ends(const struct tcp_sock *s, u32 *laddr, u32 *lport, u32 *raddr, u32 *rport) {
    *laddr = s->laddr;
    *lport = s->lport;
    *raddr = s->raddr;
    *rport = s->rport;
}
