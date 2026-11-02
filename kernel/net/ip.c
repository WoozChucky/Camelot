/* IPv4 (RFC 791; docs/camelot-net.md, "IPv4"). The header, without
 * options 20 bytes:
 *
 *   0  version (4) and header length in words (IHL, 5 to 15)
 *   1  type of service            2  total length, header included
 *   4  identification             6  flags (DF 0x4000, MF 0x2000) and
 *                                    fragment offset (low 13 bits)
 *   8  time to live               9  protocol (1 ICMP, 6 TCP, 17 UDP)
 *  10  header checksum           12  source address   16  destination
 *
 * Input checks everything a packet says about itself before believing it:
 * a header that does not fit in the frame, a total length longer than the
 * frame or shorter than the header, a wrong checksum, or a fragment (MF set
 * or an offset: reassembly is not supported) and the packet is dropped and
 * counted. Bytes past the total length are the frame's padding, ignored.
 * So is a packet from the wire whose source no host can have (RFC 1122
 * 3.2.1.3): a broadcast, multicast or loopback address, or this host's own.
 * Answering one would reflect the answer to the whole segment (a broadcast
 * source) or to this host itself. The TTL matters only to routers; this
 * host forwards nothing.
 *
 * Output never fragments: it refuses packets over NET_MTU (-EMSGSIZE) and
 * sets DF in the rest. Routing is a host's: one interface with its subnet
 * and default gateway, plus loopback (127.0.0.0/8, and the interface's own
 * address), whose packets go through a queue back into ip_input() without
 * touching the card. */
#include "net.h"

static u32 next_id;

/* Loopback: packets this host sent to itself, waiting to be received. The
 * queue turns what would be recursion (an echo request to 127.0.0.1
 * answered from inside its own sending) into a loop. */
static struct pbuf_queue loop_queue;
static bool looping;

u32 inet_sum(const u8 *p, u32 len, u32 sum) {
    u32 i = 0;
    for (; i + 1 < len; i += 2) {
        sum += ((u32)p[i] << 8) | p[i + 1];
    }
    if (i < len) {
        sum += (u32)p[i] << 8;
    }
    return sum;
}

u32 inet_fold(u32 sum) {
    while ((sum >> 16) != 0) {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    return ~sum & 0xffff;
}

static bool is_loopback(u32 addr) {
    return (addr & IP_LOOPBACK_MASK) == (IP_LOOPBACK & IP_LOOPBACK_MASK);
}

/* The interface's subnet broadcast address, or 0 without an address. */
static u32 subnet_broadcast(void) {
    return netif.addr != 0 ? netif.addr | ~netif.mask : 0;
}

bool ip_is_broadcast(u32 addr) {
    return addr == IP_BROADCAST || (netif.addr != 0 && addr == subnet_broadcast());
}

bool ip_is_local(u32 addr) {
    return (netif.addr != 0 && addr == netif.addr) || is_loopback(addr) || ip_is_broadcast(addr);
}

u32 ip_source(u32 dst) {
    return is_loopback(dst) ? IP_LOOPBACK : netif.addr;
}

/* Whether `src`, the source of a packet from the card, is one a host can
 * have: not a broadcast address (255.255.255.255 or the subnet's), not
 * multicast or reserved (224.0.0.0 and up), not loopback, not this host's
 * own. 0.0.0.0 is allowed: a host with no address yet sends from it, and
 * the protocols above never answer it. */
static bool valid_source(u32 src) {
    return (src >> 28) < 0xe && !is_loopback(src) && !ip_is_broadcast(src) &&
           (netif.addr == 0 || src != netif.addr);
}

/* Whether a packet to `dst` from the card is for this host. While the
 * interface has no address, UDP to any address is taken: DHCP's offer may
 * come to the address being offered. Loopback addresses never come from
 * the wire. */
static bool for_this_host(u32 dst, u32 proto) {
    if (is_loopback(dst)) {
        return false;
    }
    if (netif.addr == 0) {
        return proto == IP_PROTO_UDP;
    }
    return ip_is_local(dst);
}

void ip_input(struct pbuf *p, const u8 *ip, u32 len, bool loopback) {
    u32 hlen = (u32)(ip[0] & 0x0f) * 4;
    u32 total = len >= IP_HLEN ? get16(ip + 2) : 0;
    if (len < IP_HLEN || (ip[0] >> 4) != 4 || hlen < IP_HLEN || hlen > len || total < hlen || total > len) {
        net_stats.rx_bad_ip++;
        pbuf_free(p);
        return;
    }
    if (inet_fold(inet_sum(ip, hlen, 0)) != 0) {
        net_stats.rx_bad_ip_sum++;
        pbuf_free(p);
        return;
    }
    if ((get16(ip + 6) & 0x3fff) != 0) {
        net_stats.rx_fragment++;
        pbuf_free(p);
        return;
    }
    u32 proto = ip[9];
    u32 src = get32(ip + 12);
    u32 dst = get32(ip + 16);
    if (!loopback && !valid_source(src)) {
        net_stats.rx_bad_ip++;
        pbuf_free(p);
        return;
    }
    if (!loopback && !for_this_host(dst, proto)) {
        net_stats.rx_filtered++;
        pbuf_free(p);
        return;
    }
    if (proto == IP_PROTO_ICMP) {
        icmp_input(p, src, dst, ip[8], ip + hlen, total - hlen);
    } else if (proto == IP_PROTO_UDP) {
        udp_input(p, src, dst, ip + hlen, total - hlen);
    } else if (proto == IP_PROTO_TCP) {
        tcp_input(p, src, dst, ip + hlen, total - hlen);
    } else {
        net_stats.rx_bad_proto++;
        pbuf_free(p);
    }
}

/* Sends `p` back up to ip_input(), after whatever is being received. */
static void loop_back(struct pbuf *p) {
    pbuf_push(&loop_queue, p);
    if (looping) {
        return;
    }
    looping = true;
    for (;;) {
        struct pbuf *q = pbuf_pop(&loop_queue);
        if (q == NULL) {
            break;
        }
        ip_input(q, q->data + IP_OFF, q->len - IP_OFF, true);
    }
    looping = false;
}

int ip_output(struct pbuf *p, u32 src, u32 dst, u32 proto, u32 len) {
    if (len > NET_MTU - IP_HLEN) {
        pbuf_free(p);
        return -EMSGSIZE;
    }
    bool loop = is_loopback(dst) || (netif.addr != 0 && dst == netif.addr);
    bool broadcast = ip_is_broadcast(dst);
    u32 next_hop = dst;
    if (src == 0) {
        src = ip_source(dst);
    }
    if (!loop && !broadcast) {
        /* Without an address only broadcasts go out (DHCP's); multicast
         * would need group membership, which the stack doesn't do. */
        if (!netif.present || netif.addr == 0 || (dst >> 28) == 0xe) {
            pbuf_free(p);
            return -ENETUNREACH;
        }
        if (((dst ^ netif.addr) & netif.mask) != 0) {
            next_hop = netif.gateway;
            if (next_hop == 0) {
                pbuf_free(p);
                return -ENETUNREACH;
            }
        }
    } else if (broadcast && !netif.present) {
        pbuf_free(p);
        return -ENETUNREACH;
    }

    u8 *ip = p->data + IP_OFF;
    ip[0] = 0x45; /* version 4, five words */
    ip[1] = 0;
    put16(ip + 2, IP_HLEN + len);
    put16(ip + 4, next_id);
    next_id++;
    put16(ip + 6, 0x4000); /* DF: this host never fragments */
    ip[8] = IP_TTL;
    ip[9] = (u8)proto;
    put16(ip + 10, 0);
    put32(ip + 12, src);
    put32(ip + 16, dst);
    put16(ip + 10, inet_fold(inet_sum(ip, IP_HLEN, 0)));
    p->len = IP_DATA + len;

    if (loop) {
        loop_back(p);
        return 0;
    }
    if (broadcast) {
        return ether_output(p, ether_broadcast, ETH_TYPE_IP);
    }
    return arp_output(p, next_hop);
}
