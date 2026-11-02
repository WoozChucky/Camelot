/* The network interface (docs/camelot-net.md): its configuration, bringing
 * it up at boot, the counters, the timer, waiting for the network, and the
 * system calls the ifconfig and ping programs use (netinfo, netconf, ping;
 * docs/camelot-syscalls.md). The socket calls are in socket.c. */
#include "net.h"
#include "clint.h"
#include "kprintf.h"
#include "mmio.h"
#include "nic.h"
#include "proc.h"
#include "timer.h"
#include "trap.h"
#include "vm.h"

struct netif netif;
struct net_stats net_stats;
struct spinlock net_lock;

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
    initlock(&net_lock, "net");
    acquire(&net_lock);
    if (!nic_init()) {
        release(&net_lock);
        return;
    }
    const u8 *mac = nic_mac();
    for (u32 i = 0; i < ETH_ALEN; i++) {
        netif.mac[i] = mac[i];
    }
    netif.present = true;
    /* kmain runs with interrupts on: the card's come once the interface
     * is all set up (and once the lock is let go). */
    nic_start();
    release(&net_lock);
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
    acquire(&net_lock);
    dhcp_result = dhcp_configure();
    release(&net_lock);
}

int net_boot(void) {
    if (!netif.present) {
        return 0;
    }
    /* The DHCP client runs in its own process, which takes net_lock
     * itself; the scheduler must not hold it. */
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
    acquire(&net_lock);
    if (netif.how == NETIF_NONE) {
        net_configure(net_static_addr, net_static_mask, net_static_gateway, 0, NETIF_STATIC);
    }
    print_config();
    release(&net_lock);
    return 0;
}

void net_timer(void) {
    if (!netif.present) {
        return;
    }
    acquire(&net_lock);
    arp_timer();
    tcp_timer();
    /* Let the waiting processes check their timeouts. */
    net_wakeup();
    release(&net_lock);
}

void net_sleep(void) {
    sleepers++;
    sleep(&netif, &net_lock);
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
    /* The identifier is the tid (a main thread's is its pid), so two
     * threads' pings never mix. */
    int err = icmp_ping(addr, mythread()->tid, seq, PING_TIMEOUT, &r);
    if (err != 0) {
        return err;
    }
    return copyout(user_pt(), ureply, &r, sizeof r) == 0 ? 0 : -EFAULT;
}
