/* Network card driver (spec section 9.8; docs/camelot-net.md, "The card").
 *
 * The card moves frames through two rings of 8-byte descriptors in RAM,
 * each with two free-running counters: the driver hands descriptors to the
 * card by advancing TAIL, the card hands them back by advancing HEAD.
 *
 * Receive: every descriptor of the receive ring holds an empty buffer from
 * the pool, so the card always has NIC_RX_RING places to put frames. On the
 * RX interrupt the driver walks from its own counter (rx_next) up to
 * RX_HEAD: each frame's buffer goes up the stack (ether_input) and a fresh
 * one from the pool takes its place in the descriptor, which goes back to
 * the card. With the pool empty, the frame is dropped and its own buffer
 * goes back instead, so the ring never shrinks.
 *
 * Transmit: a frame goes into the next free descriptor and TX_TAIL moves
 * past it, which makes the card send it. Its buffer stays in tx_buf until
 * the card has handed the descriptor back (TX_HEAD moved past it), then
 * returns to the pool. When all NIC_TX_RING descriptors are in flight,
 * frames wait in a software queue, up to NIC_TXQ_MAX, and go out as the
 * card frees descriptors. Grail sends during the TAIL store, so the queue
 * only fills on a card that takes its time; the driver doesn't count on
 * that.
 *
 * Interrupts are the only way in: nic_intr() runs from the trap code for
 * the card's PLIC source and takes net_lock (net.h), which every caller of
 * nic_transmit() (processes, or the stack itself, inside the interrupt)
 * holds too, so the two never race. */
#include "nic.h"
#include "kalloc.h"
#include "kprintf.h"
#include "mmio.h"
#include "plic.h"
#include "sysinfo.h"
#include "vm.h"

/* The interface's card. */
struct nic {
    uintptr base; /* its page of registers */
    u32 irq;      /* its PLIC source */
    /* The rings, in one page: NIC_RX_RING receive descriptors, then
     * NIC_TX_RING transmit ones. Volatile: the card reads and writes them
     * between any two instructions. */
    volatile struct nic_desc *rx_ring;
    volatile struct nic_desc *tx_ring;
    /* The buffer each descriptor holds. */
    struct pbuf *rx_buf[NIC_RX_RING];
    struct pbuf *tx_buf[NIC_TX_RING];
    /* The counter of the next receive descriptor to look at. */
    u32 rx_next;
    /* Transmit counters: the oldest descriptor not yet reclaimed, and the
     * next one to fill (TX_TAIL). tx_tail - tx_head are in flight. */
    u32 tx_head;
    u32 tx_tail;
    /* Frames waiting for a transmit descriptor. */
    struct pbuf_queue txq;
};

static struct nic nic;
static u8 mac[ETH_ALEN];

static u32 reg(u32 off) {
    return mmio_read32(nic.base + off);
}

static void set_reg(u32 off, u32 val) {
    mmio_write32(nic.base + off, val);
}

/* Writes a ring's base, a physical address: one 64-bit register in version
 * 2 of the interface, a 32-bit one in version 1. */
static void set_base(u32 off, phys_addr_t base) {
#if __riscv_xlen == 64
    mmio_write64(nic.base + off, base);
#else
    mmio_write32(nic.base + off, base);
#endif
}

/* The MAC address of the card at `base`, bytes in wire order. */
static void read_mac(uintptr base, u8 *out) {
    u32 lo = mmio_read32(base + NET_MAC_LO);
    u32 hi = mmio_read32(base + NET_MAC_HI);
    out[0] = (u8)lo;
    out[1] = (u8)(lo >> 8);
    out[2] = (u8)(lo >> 16);
    out[3] = (u8)(lo >> 24);
    out[4] = (u8)hi;
    out[5] = (u8)(hi >> 8);
}

/* Gives receive descriptor `slot`, holding rx_buf[slot], to the card's
 * side: an empty buffer of PBUF_SIZE bytes. (TAIL is written by the
 * caller.) */
static void arm_rx(u32 slot) {
    volatile struct nic_desc *d = &nic.rx_ring[slot];
    d->addr = pa(nic.rx_buf[slot]->data);
    d->len = PBUF_SIZE;
    d->status = 0;
}

/* Sets up the card at `base`: rings, buffers, interrupts. */
static bool setup(uintptr base, u32 irq) {
    nic.base = base;
    nic.irq = irq;
    u8 *page = kzalloc();
    if (page == NULL || !pbuf_init()) {
        return false;
    }
    nic.rx_ring = (volatile struct nic_desc *)page;
    nic.tx_ring = (volatile struct nic_desc *)(page + NIC_RX_RING * sizeof(struct nic_desc));
    for (u32 i = 0; i < NIC_RX_RING; i++) {
        nic.rx_buf[i] = pbuf_alloc();
        arm_rx(i);
    }
    /* Interrupts off while the rings are set up; writing BASE or SIZE
     * starts a ring over with HEAD = TAIL = 0. */
    set_reg(NET_IRQ_ENABLE, 0);
    set_base(NET_TX_BASE, pa(nic.tx_ring));
    set_reg(NET_TX_SIZE, NIC_TX_RING);
    set_base(NET_RX_BASE, pa(nic.rx_ring));
    set_reg(NET_RX_SIZE, NIC_RX_RING);
    nic.rx_next = 0;
    nic.tx_head = 0;
    nic.tx_tail = 0;
    /* Every receive descriptor to the card. */
    set_reg(NET_RX_TAIL, NIC_RX_RING);
    /* Forget whatever ran before the kernel left behind, then turn the
     * interrupts on: the line is low until something happens. */
    set_reg(NET_ERROR, 0);
    set_reg(NET_IRQ_STATUS, NET_IRQ_RX | NET_IRQ_TX | NET_IRQ_ERROR);
    set_reg(NET_IRQ_ENABLE, NET_IRQ_RX | NET_IRQ_TX | NET_IRQ_ERROR);
    /* The PLIC source comes on in nic_start(). */
    return true;
}

bool nic_init(void) {
    u32 cards = 0;
    u32 n = sysinfo_device_count();
    for (u32 i = 0; i < n && cards < NIC_MAX_CARDS; i++) {
        struct sysinfo_device dev;
        sysinfo_device(i, &dev);
        if (dev.type != DEV_NET) {
            continue;
        }
        u8 m[ETH_ALEN];
        uintptr regs = ioremap(dev.base, dev.size);
        read_mac(regs, m);
        bool link = (mmio_read32(regs + NET_STATUS) & NET_STATUS_LINK) != 0;
        kprintf("net %u: mac %02x:%02x:%02x:%02x:%02x:%02x, irq %u, link %s", cards, m[0], m[1], m[2], m[3],
                m[4], m[5], dev.irq, link ? "up" : "down");
        if (cards > 0) {
            /* One interface (docs/camelot-net.md). */
            kprintf(", not used\n");
        } else if (dev.irq == 0 || dev.irq > 31) {
            /* This driver needs the interrupt; Grail gives every card one. */
            kprintf(", no interrupt: not used\n");
        } else if (!setup(regs, dev.irq)) {
            kprintf(", no memory for its buffers: not used\n");
        } else {
            kprintf("\n");
            for (u32 j = 0; j < ETH_ALEN; j++) {
                mac[j] = m[j];
            }
        }
        cards++;
    }
    return nic.base != 0 && nic.rx_ring != NULL;
}

void nic_start(void) {
    /* An interrupt the card raised meanwhile is taken as soon as this
     * returns. */
    plic_enable(nic.irq);
}

const u8 *nic_mac(void) {
    return mac;
}

bool nic_link_up(void) {
    return (reg(NET_STATUS) & NET_STATUS_LINK) != 0;
}

u32 nic_rx_dropped(void) {
    return reg(NET_RX_DROPPED);
}

/* Puts `p` in the next transmit descriptor and tells the card. There must
 * be a free one. */
static void send_now(struct pbuf *p) {
    u32 slot = nic.tx_tail % NIC_TX_RING;
    volatile struct nic_desc *d = &nic.tx_ring[slot];
    d->addr = pa(p->data);
    d->len = (u16)p->len;
    d->status = 0;
    nic.tx_buf[slot] = p;
    nic.tx_tail++;
    net_stats.tx_frames++;
    net_stats.tx_bytes += p->len;
    set_reg(NET_TX_TAIL, nic.tx_tail);
}

/* Frees the buffers of the descriptors the card has handed back (up to
 * TX_HEAD), then sends queued frames into the descriptors that freed. */
static void reclaim_tx(void) {
    u32 head = reg(NET_TX_HEAD);
    while (nic.tx_head != head) {
        u32 slot = nic.tx_head % NIC_TX_RING;
        if (nic.tx_ring[slot].status != NET_DESC_OK) {
            net_stats.tx_errors++;
        }
        pbuf_free(nic.tx_buf[slot]);
        nic.tx_buf[slot] = NULL;
        nic.tx_head++;
    }
    while (nic.txq.head != NULL && nic.tx_tail - nic.tx_head < NIC_TX_RING) {
        send_now(pbuf_pop(&nic.txq));
    }
}

int nic_transmit(struct pbuf *p) {
    reclaim_tx();
    if (nic.txq.head == NULL && nic.tx_tail - nic.tx_head < NIC_TX_RING) {
        send_now(p);
        return 0;
    }
    if (nic.txq.len >= NIC_TXQ_MAX) {
        net_stats.tx_errors++;
        pbuf_free(p);
        return -ENOBUFS;
    }
    pbuf_push(&nic.txq, p);
    return 0;
}

/* Takes every frame the card has put in the receive ring. */
static void receive(void) {
    u32 head = reg(NET_RX_HEAD);
    while (nic.rx_next != head) {
        u32 slot = nic.rx_next % NIC_RX_RING;
        volatile struct nic_desc *d = &nic.rx_ring[slot];
        /* A descriptor with an error status holds no frame; the card has
         * counted it in NET_RX_DROPPED. */
        if (d->status == NET_DESC_OK) {
            struct pbuf *fresh = pbuf_alloc();
            if (fresh != NULL) {
                struct pbuf *p = nic.rx_buf[slot];
                p->len = d->len;
                nic.rx_buf[slot] = fresh;
                net_stats.rx_frames++;
                net_stats.rx_bytes += p->len;
                ether_input(p);
            }
        }
        arm_rx(slot);
        nic.rx_next++;
    }
    /* Every descriptor up to rx_next back to the card. */
    set_reg(NET_RX_TAIL, nic.rx_next + NIC_RX_RING);
}

bool nic_intr(u32 irq) {
    if (nic.rx_ring == NULL || irq != nic.irq) {
        return false;
    }
    acquire(&net_lock);
    /* Acknowledge first: a frame that lands while the ring is being read
     * sets RX again, and so interrupts again, rather than being missed. */
    u32 status = reg(NET_IRQ_STATUS);
    set_reg(NET_IRQ_STATUS, status);
    if (status & NET_IRQ_ERROR) {
        /* The rings are set up once and never wrong, so this would be a
         * bug: say so, clear it, carry on. */
        kprintf("net: card error %u\n", reg(NET_ERROR));
        set_reg(NET_ERROR, 0);
    }
    receive();
    reclaim_tx();
    release(&net_lock);
    return true;
}
