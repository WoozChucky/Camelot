/* ifconfig [ADDR [MASK [GATEWAY]]]: the network interface.
 *
 * Without arguments, prints the interface (netinfo): its MAC address and
 * link, its address, netmask, gateway and DNS server and how it got them,
 * the frames and bytes through it, and every counter of the stack
 * (docs/camelot-net.md, "Counters"); then loopback. With an address, gives
 * the interface that static address (netconf), with MASK (255.255.255.0 if
 * left out) and GATEWAY (none if left out). Exits 1, having said why, if
 * there is no network card or the configuration is refused. */
#include "user.h"

/* Reads "a.b.c.d" into *addr, in host order. Returns 0, or -1. */
static int parse_addr(const char *s, uint *addr) {
    struct in_addr a;
    if (!inet_aton(s, &a)) {
        return -1;
    }
    *addr = ntohl(a.s_addr);
    return 0;
}

static void print_addr(uint a) {
    printf("%u.%u.%u.%u", a >> 24, (a >> 16) & 255, (a >> 8) & 255, a & 255);
}

/* What a network call's error means to the user. */
static const char *net_error(int err) {
    return err == ENODEV ? "no network card" : strerror(err);
}

/* `ifconfig ADDR [MASK [GATEWAY]]`. */
static int set(int argc, char **argv) {
    uint addr = 0;
    uint mask = 0xffffff00;
    uint gateway = 0;
    if (argc > 4) {
        fprintf(STDERR, "usage: ifconfig [ADDR [MASK [GATEWAY]]]\n");
        return 1;
    }
    for (int i = 1; i < argc; i++) {
        uint *dst = i == 1 ? &addr : i == 2 ? &mask : &gateway;
        if (parse_addr(argv[i], dst) < 0) {
            fprintf(STDERR, "ifconfig: %s: not an address\n", argv[i]);
            return 1;
        }
    }
    int r = netconf(addr, mask, gateway);
    if (r < 0) {
        fprintf(STDERR, "ifconfig: %s\n", net_error(-r));
        return 1;
    }
    return 0;
}

int main(int argc, char **argv) {
    if (argc > 1) {
        return set(argc, argv);
    }
    struct netinfo n;
    int r = netinfo(&n);
    if (r < 0) {
        fprintf(STDERR, "ifconfig: %s\n", net_error(-r));
        return 1;
    }
    printf("net0: mac %02x:%02x:%02x:%02x:%02x:%02x, link %s\n", n.mac[0], n.mac[1], n.mac[2], n.mac[3], n.mac[4],
           n.mac[5], (n.flags & NETINFO_LINK) ? "up" : "down");
    if (n.flags & NETINFO_UP) {
        printf("  inet ");
        print_addr(n.addr);
        printf(" netmask ");
        print_addr(n.mask);
        if (n.gateway != 0) {
            printf(" gateway ");
            print_addr(n.gateway);
        }
        if (n.dns != 0) {
            printf(" dns ");
            print_addr(n.dns);
        }
        printf(" (%s)\n", (n.flags & NETINFO_DHCP) ? "dhcp" : "static");
    } else {
        printf("  no address\n");
    }
    struct net_stats *s = &n.stats;
    printf("  rx %u frames, %u bytes; tx %u frames, %u bytes\n", s->rx_frames, s->rx_bytes, s->tx_frames,
           s->tx_bytes);
    printf("  rx dropped: card %u, filtered %u, ether %u, arp %u, ip %u, ip-checksum %u, fragment %u, protocol %u\n",
           s->rx_card_dropped, s->rx_filtered, s->rx_bad_ether, s->rx_bad_arp, s->rx_bad_ip, s->rx_bad_ip_sum,
           s->rx_fragment, s->rx_bad_proto);
    printf("  rx dropped: icmp %u, udp %u, udp-checksum %u, no-port %u, queue-full %u, tcp %u, tcp-checksum %u\n",
           s->rx_bad_icmp, s->rx_bad_udp, s->rx_bad_udp_sum, s->rx_no_port, s->rx_queue_full, s->rx_bad_tcp,
           s->rx_bad_tcp_sum);
    printf("  other: no-buffer %u, arp-failed %u, tx-error %u\n", s->no_buffer, s->arp_failed, s->tx_errors);
    printf("  tcp: no-socket %u, backlog %u, out-of-order %u, retransmits %u, resets %u\n", s->tcp_no_socket,
           s->tcp_backlog, s->tcp_out_of_order, s->tcp_retransmits, s->tcp_resets);
    printf("lo: inet 127.0.0.1 netmask 255.0.0.0\n");
    return 0;
}
