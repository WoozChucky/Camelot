/* ARP (RFC 826; docs/camelot-net.md, "ARP"): which MAC address an IPv4
 * address on the local network has.
 *
 * An ARP packet for Ethernet and IPv4 is 28 bytes:
 *
 *   0  hardware type (1, Ethernet)    2  protocol type (0x0800, IPv4)
 *   4  hardware length (6)            5  protocol length (4)
 *   6  operation (1 request, 2 reply)
 *   8  sender MAC    14  sender IPv4 address
 *  18  target MAC    24  target IPv4 address
 *
 * A request is broadcast: "who has the target address? tell the sender";
 * the owner answers the sender alone. Both ends learn from it: whoever
 * receives a packet updates the sender's entry if it has one, and the
 * target of a request adds one, since it is about to talk back (RFC 826's
 * merge rule). So a host that is pinged learns the pinger's MAC from its
 * request and answers without asking.
 *
 * The cache has ARP_ENTRIES entries, each FREE, PENDING (a request is out,
 * and the one packet waiting for the answer sits in the entry), RESOLVED
 * (good for ARP_TIMEOUT ticks) or FAILED (ARP_TRIES requests went
 * unanswered; for ARP_HOLD ticks, sending there fails at once). arp_timer()
 * moves entries along every tick. */
#include "net.h"
#include "trap.h"

#define ARP_LEN 28
#define ARP_HTYPE_ETHER 1
#define ARP_REQUEST 1
#define ARP_REPLY 2

#define ARP_FREE 0
#define ARP_PENDING 1
#define ARP_RESOLVED 2
#define ARP_FAILED 3

struct arp_entry {
    u32 state;
    u32 addr;
    u8 mac[ETH_ALEN];
    /* The tick it was answered (RESOLVED), last asked (PENDING) or given up
     * (FAILED). */
    u32 time;
    u32 tries;            /* requests sent, while PENDING */
    struct pbuf *waiting; /* the frame to send once answered, while PENDING */
};

static struct arp_entry cache[ARP_ENTRIES];

static struct arp_entry *lookup(u32 addr) {
    for (u32 i = 0; i < ARP_ENTRIES; i++) {
        if (cache[i].state != ARP_FREE && cache[i].addr == addr) {
            return &cache[i];
        }
    }
    return NULL;
}

/* Drops the packet waiting in `e`, if any: its next hop is out of reach. */
static void drop_waiting(struct arp_entry *e) {
    if (e->waiting != NULL) {
        net_stats.arp_failed++;
        pbuf_free(e->waiting);
        e->waiting = NULL;
    }
}

/* A free entry for `addr`: a FREE one, or else the one used longest ago
 * (its waiting packet, if any, dropped). */
static struct arp_entry *take_entry(u32 addr) {
    struct arp_entry *best = &cache[0];
    for (u32 i = 0; i < ARP_ENTRIES; i++) {
        struct arp_entry *e = &cache[i];
        if (e->state == ARP_FREE) {
            best = e;
            break;
        }
        if (ticks - e->time > ticks - best->time) {
            best = e;
        }
    }
    drop_waiting(best);
    best->state = ARP_FREE;
    best->addr = addr;
    best->tries = 0;
    best->time = ticks;
    return best;
}

/* Sends an ARP packet: `op` to `dst_mac` (in the Ethernet header) about
 * target `target_mac`/`target_addr`, from this host. */
static void send_arp(u32 op, const u8 *dst_mac, const u8 *target_mac, u32 target_addr) {
    struct pbuf *p = pbuf_alloc();
    if (p == NULL) {
        return;
    }
    u8 *a = p->data + ETH_HLEN;
    put16(a, ARP_HTYPE_ETHER);
    put16(a + 2, ETH_TYPE_IP);
    a[4] = ETH_ALEN;
    a[5] = 4;
    put16(a + 6, op);
    for (u32 i = 0; i < ETH_ALEN; i++) {
        a[8 + i] = netif.mac[i];
        a[18 + i] = target_mac[i];
    }
    put32(a + 14, netif.addr);
    put32(a + 24, target_addr);
    p->len = ETH_HLEN + ARP_LEN;
    ether_output(p, dst_mac, ETH_TYPE_ARP);
}

static void send_request(struct arp_entry *e) {
    static const u8 unknown[ETH_ALEN] = {0, 0, 0, 0, 0, 0};
    e->tries++;
    e->time = ticks;
    send_arp(ARP_REQUEST, ether_broadcast, unknown, e->addr);
}

/* `e` now knows `mac`: it is RESOLVED, and the packet waiting in it goes. */
static void resolved(struct arp_entry *e, const u8 *mac) {
    for (u32 i = 0; i < ETH_ALEN; i++) {
        e->mac[i] = mac[i];
    }
    e->state = ARP_RESOLVED;
    e->time = ticks;
    struct pbuf *p = e->waiting;
    e->waiting = NULL;
    if (p != NULL) {
        ether_output(p, e->mac, ETH_TYPE_IP);
    }
}

void arp_input(struct pbuf *p, const u8 *a, u32 len) {
    /* Only Ethernet and IPv4, and a whole packet (the frame may be longer:
     * padding). */
    if (len < ARP_LEN || get16(a) != ARP_HTYPE_ETHER || get16(a + 2) != ETH_TYPE_IP || a[4] != ETH_ALEN ||
        a[5] != 4) {
        net_stats.rx_bad_arp++;
        pbuf_free(p);
        return;
    }
    u32 op = get16(a + 6);
    const u8 *sender_mac = a + 8;
    u32 sender = get32(a + 14);
    u32 target = get32(a + 24);
    if (op != ARP_REQUEST && op != ARP_REPLY) {
        net_stats.rx_bad_arp++;
        pbuf_free(p);
        return;
    }
    /* Learn nothing from a probe (sender 0.0.0.0), a group MAC address, or
     * a host claiming ours; without an address, take part in nothing. */
    bool for_us = netif.addr != 0 && target == netif.addr;
    if (netif.addr == 0 || sender == 0 || sender == netif.addr || (sender_mac[0] & 1) != 0) {
        net_stats.rx_filtered++;
        pbuf_free(p);
        return;
    }
    struct arp_entry *e = lookup(sender);
    if (e != NULL) {
        resolved(e, sender_mac);
    } else if (for_us) {
        resolved(take_entry(sender), sender_mac);
    }
    if (for_us && op == ARP_REQUEST) {
        send_arp(ARP_REPLY, sender_mac, sender_mac, sender);
    } else if (!for_us && e == NULL) {
        net_stats.rx_filtered++;
    }
    pbuf_free(p);
}

int arp_output(struct pbuf *p, u32 next_hop) {
    struct arp_entry *e = lookup(next_hop);
    if (e != NULL && e->state == ARP_RESOLVED) {
        return ether_output(p, e->mac, ETH_TYPE_IP);
    }
    if (e != NULL && e->state == ARP_FAILED) {
        net_stats.arp_failed++;
        pbuf_free(p);
        return -EHOSTUNREACH;
    }
    if (e == NULL) {
        e = take_entry(next_hop);
        e->state = ARP_PENDING;
        e->waiting = p;
        send_request(e);
        return 0;
    }
    /* PENDING: the newest packet waits, the one before is dropped. */
    drop_waiting(e);
    e->waiting = p;
    return 0;
}

void arp_timer(void) {
    for (u32 i = 0; i < ARP_ENTRIES; i++) {
        struct arp_entry *e = &cache[i];
        u32 age = ticks - e->time;
        if (e->state == ARP_RESOLVED && age >= ARP_TIMEOUT) {
            e->state = ARP_FREE;
        } else if (e->state == ARP_PENDING && age >= ARP_RETRY) {
            if (e->tries < ARP_TRIES) {
                send_request(e);
            } else {
                drop_waiting(e);
                e->state = ARP_FAILED;
                e->time = ticks;
            }
        } else if (e->state == ARP_FAILED && age >= ARP_HOLD) {
            e->state = ARP_FREE;
        }
    }
}

void arp_flush(void) {
    for (u32 i = 0; i < ARP_ENTRIES; i++) {
        drop_waiting(&cache[i]);
        cache[i].state = ARP_FREE;
    }
}
