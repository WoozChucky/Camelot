/* The network interface (docs/camelot-net.md): its configuration, bringing
 * it up at boot, the counters, the timer, waiting for the network, and the
 * system calls the shell's ifconfig and ping use (netinfo, netconf, ping;
 * docs/camelot-syscalls.md). */
#include "net.h"
#include "clint.h"
#include "kprintf.h"
#include "mmio.h"
#include "nic.h"
#include "proc.h"
#include "string.h"
#include "timer.h"
#include "trap.h"
#include "vm.h"

struct netif netif;
struct net_stats net_stats;

/* The static configuration (docs/camelot-net.md, "Configuration"): DHCP
 * first, and if no server answers, 10.0.2.15/24 with gateway 10.0.2.2,
 * the address `--net user` would give the first card. Each is in .data
 * (each non-zero, so none goes to .bss, which entry.s clears), so a test patches
 * them in the image as it patches boot_fault. */
volatile u32 net_dhcp = 1;
volatile u32 net_static_addr = 0x0a00020f;    /* 10.0.2.15 */
volatile u32 net_static_mask = 0xffffff00;    /* 255.255.255.0 */
volatile u32 net_static_gateway = 0x0a000202; /* 10.0.2.2 */

/* How long ping waits for each reply. */
#define PING_TIMEOUT 10 /* 1 s */

/* Processes in net_sleep(). */
static u32 sleepers;

char *ip_str(u32 addr, char *buf) {
    ksnprintf(buf, 16, "%u.%u.%u.%u", addr >> 24, (addr >> 16) & 0xff, (addr >> 8) & 0xff, addr & 0xff);
    return buf;
}

/* The prefix length of a contiguous mask (255.255.255.0 is 24). */
static u32 prefix_len(u32 mask) {
    u32 n = 0;
    while (n < 32 && (mask & (0x80000000u >> n)) != 0) {
        n++;
    }
    return n;
}

void net_init(void) {
    if (!nic_init()) {
        return;
    }
    const u8 *mac = nic_mac();
    for (u32 i = 0; i < ETH_ALEN; i++) {
        netif.mac[i] = mac[i];
    }
    netif.present = true;
    /* kmain runs with interrupts on: the card's come once the interface
     * is all set up. */
    nic_start();
}

void net_configure(u32 addr, u32 mask, u32 gateway, u32 dns, u32 how) {
    netif.addr = addr;
    netif.mask = mask;
    netif.gateway = gateway;
    netif.dns = dns;
    netif.how = how;
    /* What the cache learned was for the old address. */
    arp_flush();
}

/* Prints the interface's configuration. */
static void print_config(void) {
    char a[16];
    char g[16];
    char d[16];
    kprintf("net: %s/%u", ip_str(netif.addr, a), prefix_len(netif.mask));
    if (netif.gateway != 0) {
        kprintf(", gateway %s", ip_str(netif.gateway, g));
    }
    if (netif.dns != 0) {
        kprintf(", dns %s", ip_str(netif.dns, d));
    }
    kprintf(" (%s)\n", netif.how == NETIF_DHCP ? "dhcp" : "static");
}

/* The DHCP client's kernel process, and what it found. */
static int dhcp_result;

static void dhcp_main(void) {
    dhcp_result = dhcp_configure();
}

int net_boot(void) {
    if (!netif.present) {
        return 0;
    }
    if (net_dhcp != 0) {
        const char *why;
        dhcp_result = -ETIMEDOUT;
        if (kproc_create("dhcp", dhcp_main, &why) == NULL) {
            kprintf("net: cannot start DHCP: %s\n", why);
        } else {
            scheduler();
        }
        if (dhcp_result != 0) {
            kprintf("net: no answer from DHCP\n");
        }
    }
    if (netif.how == NETIF_NONE) {
        net_configure(net_static_addr, net_static_mask, net_static_gateway, 0, NETIF_STATIC);
    }
    print_config();
    return 0;
}

void net_timer(void) {
    if (!netif.present) {
        return;
    }
    arp_timer();
    tcp_timer();
    /* Let the waiting processes check their timeouts. */
    net_wakeup();
}

void net_sleep(void) {
    sleepers++;
    sleep(&netif);
    sleepers--;
}

void net_wakeup(void) {
    if (sleepers != 0) {
        wakeup(&netif);
    }
}

u32 net_now(void) {
    return mmio_read32(CLINT_MTIME_LO);
}

u32 net_usec(u32 delta) {
    u32 hz = timebase_hz();
    if (hz >= 1000000) {
        return delta / (hz / 1000000);
    }
    return delta * (1000000 / hz);
}

/* ---- the system calls ---- */

static pagetable_t user_pt(void) {
    return myproc()->pagetable;
}

int net_sys_info(u32 uinfo) {
    if (!uvm_range_ok(user_pt(), uinfo, sizeof(struct netinfo), PTE_W)) {
        return -EFAULT;
    }
    if (!netif.present) {
        return -ENODEV;
    }
    struct netinfo info;
    for (u32 i = 0; i < ETH_ALEN; i++) {
        info.mac[i] = netif.mac[i];
    }
    info.flags = 0;
    if (nic_link_up()) {
        info.flags |= NETINFO_LINK;
    }
    if (netif.addr != 0) {
        info.flags |= NETINFO_UP;
    }
    if (netif.how == NETIF_DHCP) {
        info.flags |= NETINFO_DHCP;
    }
    info.addr = netif.addr;
    info.mask = netif.mask;
    info.gateway = netif.gateway;
    info.dns = netif.dns;
    net_stats.rx_card_dropped = nic_rx_dropped();
    info.stats = net_stats;
    return copyout(user_pt(), uinfo, &info, sizeof info) == 0 ? 0 : -EFAULT;
}

int net_sys_conf(u32 addr, u32 mask, u32 gateway) {
    if (!netif.present) {
        return -ENODEV;
    }
    /* A mask is ones then zeros, at least one of each... */
    u32 host = ~mask;
    if (mask == 0 || host == 0 || (host & (host + 1)) != 0) {
        return -EINVAL;
    }
    /* ... the address is an ordinary unicast one (not 0.0.0.0/8,
     * 127.0.0.0/8 or 224.0.0.0 and up), and not its subnet's network or
     * broadcast address... */
    u32 top = addr >> 24;
    if (top == 0 || top == 127 || top >= 224 || (addr & host) == 0 || (addr & host) == host) {
        return -EINVAL;
    }
    /* ... and the gateway, if any, another host on that subnet. */
    if (gateway != 0 && (((gateway ^ addr) & mask) != 0 || gateway == addr || (gateway & host) == 0 ||
                         (gateway & host) == host)) {
        return -EINVAL;
    }
    net_configure(addr, mask, gateway, netif.dns, NETIF_STATIC);
    return 0;
}

int net_sys_ping(u32 addr, u32 seq, u32 ureply) {
    if (!uvm_range_ok(user_pt(), ureply, sizeof(struct ping_reply), PTE_W)) {
        return -EFAULT;
    }
    if (!netif.present) {
        return -ENODEV;
    }
    if (addr == 0 || seq > 0xffff) {
        return -EINVAL;
    }
    struct ping_reply r;
    /* The identifier is the pid, so two processes' pings never mix. */
    int err = icmp_ping(addr, myproc()->pid, seq, PING_TIMEOUT, &r);
    if (err != 0) {
        return err;
    }
    return copyout(user_pt(), ureply, &r, sizeof r) == 0 ? 0 : -EFAULT;
}

/* ---- tcptest: TCP's test hook until #85 ----
 *
 * A temporary system call (docs/camelot-net.md, "Testing TCP until #85")
 * that drives the kernel's TCP API from a process, for kernel/user/tcptest.c
 * and the tests: a client that sends a pattern and reads to the end, and a
 * server that echoes what each connection it accepts sends. #85's socket
 * calls replace it, and its listeners, which a process holds by handle
 * between calls, with sockets behind file descriptors. */

/* Listeners handed out by TCPTEST_LISTEN: the handle is the index. */
#define TCPTEST_LISTENERS 4

static struct {
    struct tcp_sock *s; /* NULL: free */
    u32 pid;
} tcptest_listeners[TCPTEST_LISTENERS];

/* Bytes moved per tcp_send() or tcp_recv(), through the kernel stack. */
#define TCPTEST_CHUNK 512

/* The client's data: byte i of what it sends. Not periodic within 2^24
 * bytes, so a byte delivered at the wrong place shows. */
static u8 tcptest_pattern(u32 i) {
    return (u8)(i ^ (i >> 8) ^ (i >> 16));
}

/* FNV-1a over `n` bytes, continuing from `h`. */
static u32 fnv1a(u32 h, const u8 *p, u32 n) {
    for (u32 i = 0; i < n; i++) {
        h ^= p[i];
        h *= 0x01000193u;
    }
    return h;
}

#define FNV_BASIS 0x811c9dc5u

static void report_ends(struct tcp_sock *s, struct tcptest_report *r) {
    tcp_ends(s, &r->laddr, &r->lport, &r->raddr, &r->rport);
}

/* Connects to addr:port, sends `bytes` bytes of the pattern, ends its side
 * and reads until the server ends its own. */
static int tcptest_connect(u32 addr, u32 port, u32 bytes, struct tcptest_report *r) {
    int err;
    struct tcp_sock *s = tcp_socket(&err);
    if (s == NULL) {
        return err;
    }
    err = tcp_connect(s, addr, port);
    report_ends(s, r);
    u8 buf[TCPTEST_CHUNK];
    while (err == 0 && r->sent < bytes) {
        u32 n = bytes - r->sent < TCPTEST_CHUNK ? bytes - r->sent : TCPTEST_CHUNK;
        for (u32 i = 0; i < n; i++) {
            buf[i] = tcptest_pattern(r->sent + i);
        }
        int m = tcp_send(s, NULL, (u32)(uintptr)buf, n);
        if (m < 0) {
            err = m;
        } else {
            r->sent += (u32)m;
            if ((u32)m < n) {
                err = -EINTR;
            }
        }
    }
    if (err == 0) {
        err = tcp_shutdown(s);
    }
    while (err == 0) {
        int m = tcp_recv(s, NULL, (u32)(uintptr)buf, TCPTEST_CHUNK);
        if (m <= 0) {
            err = m;
            break;
        }
        r->hash = fnv1a(r->hash, buf, (u32)m);
        r->received += (u32)m;
    }
    tcp_close(s);
    return err;
}

/* Accepts a connection on listener `l` and sends back what it sends,
 * until it ends its side; then closes it. */
static int tcptest_accept(struct tcp_sock *l, struct tcptest_report *r) {
    int err;
    struct tcp_sock *s = tcp_accept(l, &err);
    if (s == NULL) {
        return err;
    }
    report_ends(s, r);
    u8 buf[TCPTEST_CHUNK];
    err = 0;
    for (;;) {
        int m = tcp_recv(s, NULL, (u32)(uintptr)buf, TCPTEST_CHUNK);
        if (m <= 0) {
            err = m;
            break;
        }
        r->hash = fnv1a(r->hash, buf, (u32)m);
        r->received += (u32)m;
        int w = tcp_send(s, NULL, (u32)(uintptr)buf, (u32)m);
        if (w < 0) {
            err = w;
            break;
        }
        r->sent += (u32)w;
        if (w < m) {
            err = -EINTR;
            break;
        }
    }
    tcp_close(s);
    return err;
}

/* The listener behind `handle`, if the caller holds it. */
static struct tcp_sock *tcptest_listener(u32 handle) {
    if (handle >= TCPTEST_LISTENERS || tcptest_listeners[handle].s == NULL ||
        tcptest_listeners[handle].pid != myproc()->pid) {
        return NULL;
    }
    return tcptest_listeners[handle].s;
}

int net_sys_tcptest(u32 op, u32 a, u32 b, u32 c, u32 ureport) {
    struct tcptest_report r;
    memset(&r, 0, sizeof r);
    r.hash = FNV_BASIS;
    bool reports = op == TCPTEST_CONNECT || op == TCPTEST_ACCEPT;
    if (reports && !uvm_range_ok(user_pt(), ureport, sizeof r, PTE_W)) {
        return -EFAULT;
    }
    if (!netif.present) {
        return -ENODEV;
    }
    int ret;
    if (op == TCPTEST_CONNECT) {
        ret = tcptest_connect(a, b, c, &r);
    } else if (op == TCPTEST_LISTEN) {
        u32 h = 0;
        while (h < TCPTEST_LISTENERS && tcptest_listeners[h].s != NULL) {
            h++;
        }
        if (h == TCPTEST_LISTENERS) {
            return -ENOBUFS;
        }
        struct tcp_sock *s = tcp_socket(&ret);
        if (s == NULL) {
            return ret;
        }
        ret = a == 0 ? -EINVAL : tcp_bind(s, a);
        if (ret == 0) {
            ret = tcp_listen(s, b);
        }
        if (ret < 0) {
            tcp_close(s);
            return ret;
        }
        tcptest_listeners[h].s = s;
        tcptest_listeners[h].pid = myproc()->pid;
        return (int)h;
    } else if (op == TCPTEST_ACCEPT) {
        struct tcp_sock *l = tcptest_listener(a);
        ret = l == NULL ? -EINVAL : tcptest_accept(l, &r);
    } else if (op == TCPTEST_CLOSE) {
        struct tcp_sock *l = tcptest_listener(a);
        if (l == NULL) {
            return -EINVAL;
        }
        tcp_close(l);
        tcptest_listeners[a].s = NULL;
        return 0;
    } else {
        return -EINVAL;
    }
    if (copyout(user_pt(), ureport, &r, sizeof r) < 0) {
        return -EFAULT;
    }
    return ret;
}

void net_proc_exit(u32 pid) {
    for (u32 h = 0; h < TCPTEST_LISTENERS; h++) {
        if (tcptest_listeners[h].s != NULL && tcptest_listeners[h].pid == pid) {
            tcp_close(tcptest_listeners[h].s);
            tcptest_listeners[h].s = NULL;
        }
    }
}
