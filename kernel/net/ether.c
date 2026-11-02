/* Ethernet II (docs/camelot-net.md, "Ethernet"): a 14-byte header, the
 * destination and source MAC addresses and the EtherType, then the
 * payload. No FCS (the card neither sends nor checks one) and no padding to
 * 60 bytes, which Grail's cards don't need.
 *
 * The card takes every frame its backend delivers (a hub repeats every
 * frame to every port), so the first job here is the filter a real card
 * does in hardware: only frames to the interface's own MAC address or to
 * broadcast are for this host. */
#include "net.h"
#include "nic.h"

const u8 ether_broadcast[ETH_ALEN] = {0xff, 0xff, 0xff, 0xff, 0xff, 0xff};

static bool mac_eq(const u8 *a, const u8 *b) {
    for (u32 i = 0; i < ETH_ALEN; i++) {
        if (a[i] != b[i]) {
            return false;
        }
    }
    return true;
}

void ether_input(struct pbuf *p) {
    const u8 *f = p->data;
    /* The card never delivers a frame shorter than the header; checked
     * anyway, as everything a frame says is. */
    if (p->len < ETH_HLEN) {
        net_stats.rx_bad_ether++;
        pbuf_free(p);
        return;
    }
    if (!mac_eq(f, netif.mac) && !mac_eq(f, ether_broadcast)) {
        net_stats.rx_filtered++;
        pbuf_free(p);
        return;
    }
    u32 type = get16(f + 12);
    if (type == ETH_TYPE_IP) {
        ip_input(p, f + ETH_HLEN, p->len - ETH_HLEN, false);
    } else if (type == ETH_TYPE_ARP) {
        arp_input(p, f + ETH_HLEN, p->len - ETH_HLEN);
    } else {
        net_stats.rx_bad_ether++;
        pbuf_free(p);
    }
}

int ether_output(struct pbuf *p, const u8 *dst, u32 type) {
    u8 *f = p->data;
    for (u32 i = 0; i < ETH_ALEN; i++) {
        f[i] = dst[i];
        f[ETH_ALEN + i] = netif.mac[i];
    }
    put16(f + 12, type);
    return nic_transmit(p);
}
