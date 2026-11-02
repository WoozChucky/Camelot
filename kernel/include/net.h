/* Camelot's network stack (docs/camelot-net.md): one Ethernet interface
 * (the card driver, nic.h), ARP, IPv4, ICMP echo and UDP, configured by
 * DHCP or statically, plus the loopback address 127.0.0.1. The code is in
 * kernel/net/, one file per layer:
 *
 *   nic.c    the card: rings, interrupts, the transmit queue
 *   pbuf.c   the packet buffer pool
 *   ether.c  Ethernet II: filtering by address, dispatch by EtherType
 *   arp.c    the ARP cache: requests, replies, timeouts, a packet waiting
 *   ip.c     IPv4: header checks, checksums, routing, loopback
 *   icmp.c   echo requests and replies (ping)
 *   udp.c    UDP ports and checksums: the kernel-internal socket API
 *   dhcp.c   the DHCP client
 *   net.c    the interface's configuration, boot, the counters, the timer
 *            and the system calls
 *
 * Everything runs with interrupts off, as all kernel code does (proc.c), on
 * one hart: a frame is handled from start to end inside the card's
 * interrupt (an answer, such as an echo reply, is sent from there), and a
 * process that waits for the network sleeps on `netif` until a packet or a
 * timer tick wakes it. No locks.
 *
 * Addresses are u32 in host order (10.0.2.15 is 0x0a00020f) and ports u16;
 * packets hold them big-endian, and get16/get32/put16/put32 convert. */
#ifndef CAMELOT_NET_H
#define CAMELOT_NET_H

#include "syscall.h"
#include "types.h"

/* ---- packet buffers (pbuf.c) ---- */

/* A buffer holds one Ethernet frame. PBUF_SIZE is more than the largest
 * frame (1514 bytes) and two of them fill a page. The pool has NPBUF; the
 * card's receive ring always holds NIC_RX_RING of them (nic.h). When the
 * pool is empty a received frame is dropped (its buffer goes straight back
 * to the card) and sending fails with -ENOBUFS; both count in no_buffer. */
#define PBUF_SIZE 2048
#define NPBUF 64

struct pbuf {
    struct pbuf *next; /* in a queue: the transmit queue, ARP, a socket */
    u8 *data;          /* PBUF_SIZE bytes; its kernel address is also its physical one */
    u32 len;           /* bytes of the frame in data */
    /* A datagram waiting in a UDP socket: where its payload starts in
     * data, and where it came from. */
    u32 off;
    u32 addr;
    u16 port;
};

/* A FIFO of buffers, linked through `next`. */
struct pbuf_queue {
    struct pbuf *head;
    struct pbuf *tail;
    u32 len;
};

/* Takes the pool's pages from kalloc. Returns false if there are not
 * enough. */
bool pbuf_init(void);
/* A buffer from the pool (len 0), or NULL when it is empty (counted in
 * no_buffer). */
struct pbuf *pbuf_alloc(void);
void pbuf_free(struct pbuf *p);
/* Buffers left in the pool. */
u32 pbuf_available(void);

void pbuf_push(struct pbuf_queue *q, struct pbuf *p);
/* The first buffer of `q`, taken off it, or NULL. */
struct pbuf *pbuf_pop(struct pbuf_queue *q);
/* Frees every buffer in `q`. */
void pbuf_drain(struct pbuf_queue *q);

/* ---- bytes in network order ---- */

static inline u32 get16(const u8 *p) {
    return ((u32)p[0] << 8) | p[1];
}

static inline u32 get32(const u8 *p) {
    return ((u32)p[0] << 24) | ((u32)p[1] << 16) | ((u32)p[2] << 8) | p[3];
}

static inline void put16(u8 *p, u32 v) {
    p[0] = (u8)(v >> 8);
    p[1] = (u8)v;
}

static inline void put32(u8 *p, u32 v) {
    p[0] = (u8)(v >> 24);
    p[1] = (u8)(v >> 16);
    p[2] = (u8)(v >> 8);
    p[3] = (u8)v;
}

/* The Internet checksum (RFC 1071): inet_sum() adds the big-endian 16-bit
 * words of `len` bytes to `sum` (an odd last byte is padded with a zero),
 * inet_fold() folds the carries back in and complements. A packet whose
 * checksum field is right sums, checksum included, to a fold of 0. */
u32 inet_sum(const u8 *p, u32 len, u32 sum);
u32 inet_fold(u32 sum);

/* ---- frame layout ----
 * What the stack sends always has the same shape: the Ethernet header, an
 * IPv4 header without options, then ICMP or UDP. So each layer writes its
 * header at a fixed offset of the buffer, from the top down. */
#define ETH_ALEN 6
#define ETH_HLEN 14
#define ETH_TYPE_IP 0x0800
#define ETH_TYPE_ARP 0x0806
#define IP_HLEN 20
#define IP_OFF ETH_HLEN             /* the IPv4 header */
#define IP_DATA (IP_OFF + IP_HLEN)  /* its payload: 34 */
#define UDP_HLEN 8
#define UDP_DATA (IP_DATA + UDP_HLEN) /* a UDP payload: 42 */
#define ICMP_HLEN 8

/* The largest IPv4 packet sent or taken, header included: Ethernet's MTU.
 * Nothing bigger is sent (no fragmentation), and fragments are dropped. */
#define NET_MTU 1500
#define UDP_MAX_DATA (NET_MTU - IP_HLEN - UDP_HLEN) /* 1472 */

#define IP_PROTO_ICMP 1
#define IP_PROTO_UDP 17

/* 255.255.255.255, 127.0.0.1, and the loopback network's mask. */
#define IP_BROADCAST 0xffffffffu
#define IP_LOOPBACK 0x7f000001u
#define IP_LOOPBACK_MASK 0xff000000u

/* ---- the interface (net.c) ---- */

/* How the interface got its address. */
#define NETIF_NONE 0   /* it has none yet */
#define NETIF_DHCP 1   /* from a DHCP server */
#define NETIF_STATIC 2 /* the static configuration, or `ifconfig ADDR` */

struct netif {
    bool present;    /* there is a card (nic_init found one) */
    u8 mac[ETH_ALEN];
    u32 addr;        /* 0 until configured */
    u32 mask;
    u32 gateway;     /* the default route; 0 for none */
    u32 dns;         /* what DHCP said, for #85's resolver; 0 for none */
    u32 how;         /* NETIF_ */
};

extern struct netif netif;
/* The counters (struct net_stats in syscall.h): what ifconfig shows. */
extern struct net_stats net_stats;

/* The static configuration (docs/camelot-net.md, "Configuration"): used
 * when net_dhcp is 0, and when DHCP gets no answer. Variables in .data,
 * so tests (and anyone) can patch them in kernel.elf. */
extern volatile u32 net_dhcp;
extern volatile u32 net_static_addr;
extern volatile u32 net_static_mask;
extern volatile u32 net_static_gateway;

/* Finds the cards and brings up the first one (nic_init), with no address
 * yet. Prints a line per card; nothing without one. After trap_init(). */
void net_init(void);

/* Gives the interface its address: DHCP in a kernel process (when
 * net_dhcp is set), falling back to the static configuration; prints the
 * result. Does nothing without a card. Returns 0 (the failure count, like
 * the other boot steps). */
int net_boot(void);

/* Sets the address, mask and default gateway (`how` NETIF_DHCP or
 * NETIF_STATIC) and forgets the ARP cache. */
void net_configure(u32 addr, u32 mask, u32 gateway, u32 dns, u32 how);

/* Called on every timer tick, from the interrupt: ARP's timeouts, and
 * waking the processes that wait for the network so they can check
 * theirs. */
void net_timer(void);

/* A process waits for the network: it sleeps until a packet for it or the
 * next timer tick. Callers check their condition in a loop around it. */
void net_sleep(void);
/* Wakes every process in net_sleep(): something arrived. */
void net_wakeup(void);

/* mtime ticks converted to microseconds (for ping's round-trip time). */
u32 net_usec(u32 mtime_delta);
/* The low word of mtime. */
u32 net_now(void);

/* The system calls (docs/camelot-syscalls.md): netinfo, netconf, ping. */
int net_sys_info(u32 uinfo);
int net_sys_conf(u32 addr, u32 mask, u32 gateway);
int net_sys_ping(u32 addr, u32 seq, u32 ureply);

/* "a.b.c.d" of `addr` into `buf` (16 bytes). Returns buf. */
char *ip_str(u32 addr, char *buf);

/* ---- Ethernet (ether.c) ---- */

extern const u8 ether_broadcast[ETH_ALEN];

/* A received frame (p->len bytes): drops what is not addressed to the
 * interface's MAC or broadcast, passes ARP and IPv4 on. Takes `p`. */
void ether_input(struct pbuf *p);

/* Fills in the Ethernet header of the frame in `p` (to `dst`, from the
 * interface's MAC, `type`) and transmits it. Takes `p`. */
int ether_output(struct pbuf *p, const u8 *dst, u32 type);

/* ---- ARP (arp.c) ---- */

/* The cache: ARP_ENTRIES addresses. An entry answered is good for
 * ARP_TIMEOUT ticks; a request is repeated every ARP_RETRY ticks, ARP_TRIES
 * times in all, after which the address counts as unreachable for
 * ARP_HOLD ticks (sending to it fails at once with -EHOSTUNREACH). */
#define ARP_ENTRIES 16
#define ARP_TIMEOUT 600 /* 60 s */
#define ARP_RETRY 10    /* 1 s */
#define ARP_TRIES 3
#define ARP_HOLD 30 /* 3 s */

/* An ARP packet (`len` bytes at `arp`, inside p's frame). Takes `p`. */
void arp_input(struct pbuf *p, const u8 *arp, u32 len);

/* Sends the IPv4 frame in `p` to `next_hop` on the local network: at once
 * if its MAC is known, otherwise once ARP has found it (the frame waits in
 * the cache entry, one per address: a newer one replaces it). Takes `p`.
 * Returns 0, -EHOSTUNREACH (the address did not answer lately) or
 * -ENOBUFS. */
int arp_output(struct pbuf *p, u32 next_hop);

/* Ages the cache and repeats the requests that are due: every tick. */
void arp_timer(void);

/* Empties the cache (the interface's address changed). */
void arp_flush(void);

/* ---- IPv4 (ip.c) ---- */

/* The TTL of the packets the stack sends. */
#define IP_TTL 64

/* A received IPv4 packet: `len` bytes at `ip` inside p's frame, from the
 * card or, when `loopback`, sent by this host to itself. Checks the
 * header, drops what is not for this host, passes ICMP and UDP on. Takes
 * `p`. */
void ip_input(struct pbuf *p, const u8 *ip, u32 len, bool loopback);

/* Sends `len` bytes at p->data + IP_DATA (an ICMP message or a UDP
 * datagram) to `dst` as protocol `proto`, from `src` (0: the interface's
 * address, or 127.0.0.1 to loopback). Routing: 127.0.0.0/8 and the
 * interface's own address loop back, 255.255.255.255 and the subnet's
 * broadcast address are broadcast, the subnet goes straight to `dst`,
 * anything else to the default gateway. Takes `p`. Returns 0, or
 * -EMSGSIZE, -ENETUNREACH (no route), -EHOSTUNREACH or -ENOBUFS. */
int ip_output(struct pbuf *p, u32 src, u32 dst, u32 proto, u32 len);

/* Whether `addr` is one this host takes packets for: its own, loopback,
 * or a broadcast. */
bool ip_is_local(u32 addr);

/* Whether `addr` is 255.255.255.255 or the subnet's broadcast address. */
bool ip_is_broadcast(u32 addr);

/* The source address of a packet to `dst`: 127.0.0.1 to loopback, the
 * interface's address (0 while it has none) otherwise. */
u32 ip_source(u32 dst);

/* ---- ICMP (icmp.c) ---- */

/* An ICMP message (`len` bytes at `icmp`) for this host from `src`, sent
 * to `dst` with time to live `ttl`. Answers echo requests to this host's
 * own addresses and completes the ping waiting for an echo reply. Takes
 * `p`. */
void icmp_input(struct pbuf *p, u32 src, u32 dst, u32 ttl, const u8 *icmp, u32 len);

/* The echo request data a ping sends: 56 bytes, 64 with the ICMP header. */
#define PING_DATA 56

/* Sends an echo request to `dst` with identifier `id` and sequence number
 * `seq`, and waits up to `timeout` ticks for its reply. Fills in `r` and
 * returns 0, or -ETIMEDOUT (also when the process is killed while it
 * waits), or ip_output()'s error. Must be called by a process. */
int icmp_ping(u32 dst, u32 id, u32 seq, u32 timeout, struct ping_reply *r);

/* ---- UDP (udp.c): the kernel-internal socket API ---- */

/* Sockets, ports given out by udp_open(0), and the datagrams a socket
 * holds until it is read: beyond that, arrivals are dropped and counted in
 * rx_queue_full. */
#define UDP_SOCKETS 8
#define UDP_EPHEMERAL_FIRST 49152
#define UDP_QUEUE_MAX 4

/* The echo service (RFC 862): a datagram to this port with no socket on
 * it comes back to its sender, unless it came from a port below 1024
 * (so two echo services can't bounce one between them forever). */
#define UDP_ECHO_PORT 7

struct udp_sock {
    bool used;
    u16 port;
    struct pbuf_queue queue; /* received datagrams, oldest first */
};

/* A socket bound to `port` (0: the next free port from
 * UDP_EPHEMERAL_FIRST up), or NULL with *err set to -EADDRINUSE or
 * -ENOBUFS (every socket is taken). */
struct udp_sock *udp_open(u32 port, int *err);

/* Unbinds `s`, dropping what it has not read. */
void udp_close(struct udp_sock *s);

/* Sends `len` bytes from `buf` (kernel memory) to `dst`:`port`, from s's
 * port. Returns `len`, or -EMSGSIZE (over UDP_MAX_DATA), or ip_output()'s
 * error. */
int udp_sendto(struct udp_sock *s, u32 dst, u32 port, const void *buf, u32 len);

/* Takes the oldest datagram that came to `s`: copies up to `size` bytes
 * of it to `buf` (the rest is lost, as with any datagram socket), stores
 * its sender in *from and *port (unless NULL) and returns its length.
 * Sleeps while there is none, for at most `timeout` ticks (0: no limit),
 * then -ETIMEDOUT (at once if the process is killed). Must be called by a
 * process. */
int udp_recvfrom(struct udp_sock *s, void *buf, u32 size, u32 *from, u32 *port, u32 timeout);

/* A UDP datagram (`len` bytes at `udp`) for this host from `src` to
 * `dst`. Takes `p`. */
void udp_input(struct pbuf *p, u32 src, u32 dst, const u8 *udp, u32 len);

/* ---- DHCP (dhcp.c) ---- */

/* Asks for an address, DHCP_TRIES times at most, waiting DHCP_TIMEOUT
 * ticks for each answer; configures the interface with the lease. Returns
 * 0, or -ETIMEDOUT if no server answered. Must be called by a process. */
#define DHCP_TRIES 4
#define DHCP_TIMEOUT 10 /* 1 s */
int dhcp_configure(void);

#endif
