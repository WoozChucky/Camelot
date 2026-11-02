/* The DHCP client (RFC 2131, options from RFC 2132; docs/camelot-net.md,
 * "DHCP"), on the UDP socket API: what gives the interface its address on
 * `--net user` (whose gateway is the server, crates/gateway) or any network
 * with a server.
 *
 * Four messages: the client broadcasts DISCOVER, a server OFFERs an
 * address, the client broadcasts a REQUEST for it naming that server, and
 * the server ACKs (or NAKs, and the client starts over). Each is a BOOTP
 * packet: a fixed 236-byte part, the "magic cookie" 99.130.83.99, then
 * options as code, length, data:
 *
 *    0  op (1 request, 2 reply)   1  htype (1)   2  hlen (6)   3  hops
 *    4  xid, the client's transaction number, echoed by the server
 *    8  secs   10  flags (0x8000: broadcast the reply)
 *   12  ciaddr (client)   16  yiaddr ("your" address, offered)
 *   20  siaddr   24  giaddr (relay)   28  chaddr (the client's MAC)
 *   44  sname    108 file    236 magic cookie   240 options
 *
 * The client has no address while it asks, so it sends from 0.0.0.0 to
 * 255.255.255.255 and sets the broadcast flag, asking for broadcast
 * replies. Each try waits DHCP_TIMEOUT ticks for an answer; after
 * DHCP_TRIES the caller falls back to the static configuration. The lease
 * is not renewed: the gateway's never expire while it lives (spec section
 * 9.8), and a lease of a day outlasts any test. */
#include "kalloc.h"
#include "net.h"
#include "string.h"
#include "trap.h"

#define DHCP_SERVER_PORT 67
#define DHCP_CLIENT_PORT 68

#define BOOTREQUEST 1
#define BOOTREPLY 2
#define FIXED_LEN 236
#define MAGIC 0x63825363u
#define OPTIONS (FIXED_LEN + 4)
/* BOOTP's smallest message; some servers ignore shorter ones. */
#define MIN_MESSAGE 300
#define FLAG_BROADCAST 0x8000

/* Message types (option 53). */
#define DISCOVER 1
#define OFFER 2
#define REQUEST 3
#define ACK 5
#define NAK 6

/* Option codes. */
#define OPT_PAD 0
#define OPT_MASK 1
#define OPT_ROUTER 3
#define OPT_DNS 6
#define OPT_REQUESTED 50
#define OPT_TYPE 53
#define OPT_SERVER 54
#define OPT_PARAMS 55
#define OPT_END 255

/* What a server's answer says. */
struct answer {
    u32 type;
    u32 addr; /* yiaddr */
    u32 mask;
    u32 router;
    u32 dns;
    u32 server;
};

/* Writes a client message of `type` into `m`: `requested` and `server`
 * (when non-zero) as options. Returns its length. */
static u32 build(u8 *m, u32 type, u32 xid, u32 requested, u32 server) {
    memset(m, 0, MIN_MESSAGE);
    m[0] = BOOTREQUEST;
    m[1] = 1; /* Ethernet */
    m[2] = ETH_ALEN;
    put32(m + 4, xid);
    put16(m + 10, FLAG_BROADCAST);
    memcpy(m + 28, netif.mac, ETH_ALEN);
    put32(m + FIXED_LEN, MAGIC);
    u32 i = OPTIONS;
    m[i++] = OPT_TYPE;
    m[i++] = 1;
    m[i++] = (u8)type;
    if (requested != 0) {
        m[i++] = OPT_REQUESTED;
        m[i++] = 4;
        put32(m + i, requested);
        i += 4;
    }
    if (server != 0) {
        m[i++] = OPT_SERVER;
        m[i++] = 4;
        put32(m + i, server);
        i += 4;
    }
    /* The options the client wants in the answer. */
    m[i++] = OPT_PARAMS;
    m[i++] = 3;
    m[i++] = OPT_MASK;
    m[i++] = OPT_ROUTER;
    m[i++] = OPT_DNS;
    m[i++] = OPT_END;
    return i < MIN_MESSAGE ? MIN_MESSAGE : i;
}

/* Reads the server's message `m` (`len` bytes) into *a. False unless it is
 * a reply to transaction `xid` for this card, with a message type. */
static bool parse(const u8 *m, u32 len, u32 xid, struct answer *a) {
    if (len < OPTIONS || m[0] != BOOTREPLY || get32(m + 4) != xid || memcmp(m + 28, netif.mac, ETH_ALEN) != 0 ||
        get32(m + FIXED_LEN) != MAGIC) {
        return false;
    }
    memset(a, 0, sizeof *a);
    a->addr = get32(m + 16);
    u32 i = OPTIONS;
    while (i < len && m[i] != OPT_END) {
        if (m[i] == OPT_PAD) {
            i++;
            continue;
        }
        /* An option that runs past the message ends the reading. */
        if (i + 2 > len || i + 2 + m[i + 1] > len) {
            break;
        }
        u32 code = m[i];
        u32 n = m[i + 1];
        const u8 *data = m + i + 2;
        if (code == OPT_TYPE && n >= 1) {
            a->type = data[0];
        } else if (code == OPT_MASK && n >= 4) {
            a->mask = get32(data);
        } else if (code == OPT_ROUTER && n >= 4) {
            a->router = get32(data);
        } else if (code == OPT_DNS && n >= 4) {
            a->dns = get32(data);
        } else if (code == OPT_SERVER && n >= 4) {
            a->server = get32(data);
        }
        i += 2 + n;
    }
    return a->type != 0;
}

/* Waits for an answer to `xid` of type `want` (ACK also takes a NAK) until
 * tick `deadline`, ignoring anything else. */
static bool await(struct udp_sock *s, u8 *buf, u32 xid, u32 want, u32 deadline, struct answer *a) {
    for (;;) {
        u32 left = deadline - ticks;
        if ((i32)left <= 0) {
            return false;
        }
        int n = udp_recvfrom(s, buf, PBUF_SIZE, NULL, NULL, left);
        if (n < 0) {
            return false;
        }
        if (parse(buf, (u32)n, xid, a) && (a->type == want || (want == ACK && a->type == NAK))) {
            return true;
        }
    }
}

int dhcp_configure(void) {
    int err;
    struct udp_sock *s = udp_open(DHCP_CLIENT_PORT, &err);
    if (s == NULL) {
        return err;
    }
    /* A page: the answers in its first half, the messages sent in the
     * second. */
    u8 *buf = kalloc();
    if (buf == NULL) {
        udp_close(s);
        return -ENOMEM;
    }
    u8 *msg = buf + PBUF_SIZE;
    /* A transaction number unlikely to be another client's: the MAC and
     * the clock. */
    u32 xid = get32(netif.mac + 2) ^ net_now();
    err = -ETIMEDOUT;
    for (u32 try = 0; try < DHCP_TRIES && err != 0; try++) {
        xid++;
        struct answer offer;
        u32 len = build(msg, DISCOVER, xid, 0, 0);
        udp_sendto(s, IP_BROADCAST, DHCP_SERVER_PORT, msg, len);
        if (!await(s, buf, xid, OFFER, ticks + DHCP_TIMEOUT, &offer) || offer.addr == 0) {
            continue;
        }
        struct answer ack;
        len = build(msg, REQUEST, xid, offer.addr, offer.server);
        udp_sendto(s, IP_BROADCAST, DHCP_SERVER_PORT, msg, len);
        if (!await(s, buf, xid, ACK, ticks + DHCP_TIMEOUT, &ack) || ack.type != ACK || ack.addr == 0) {
            continue;
        }
        /* No mask option: the class C one, which every small network uses. */
        u32 mask = ack.mask != 0 ? ack.mask : 0xffffff00u;
        net_configure(ack.addr, mask, ack.router, ack.dns, NETIF_DHCP);
        err = 0;
    }
    kfree(buf);
    udp_close(s);
    return err;
}
