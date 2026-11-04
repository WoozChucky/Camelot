/* Driver for Grail's network cards (spec section 9.8; docs/camelot-net.md):
 * Ethernet frames through two rings of descriptors in RAM, receive on
 * interrupt, transmit from a queue. The kernel has one network interface:
 * the first card in the sysinfo device table. Others are listed at boot and
 * left alone. */
#ifndef CAMELOT_NIC_H
#define CAMELOT_NIC_H

#include "net.h"
#include "types.h"

/* The most cards a machine has (spec section 9.8). */
#define NIC_MAX_CARDS 4

/* Register offsets in a card's page. */
#define NET_MAC_LO 0x00     /* MAC bytes 0-3, byte 0 in bits 7:0 */
#define NET_MAC_HI 0x04     /* MAC bytes 4-5 in bits 15:0 */
#define NET_STATUS 0x08     /* bit 0: link up */
#define NET_ERROR 0x0c      /* the latest ring error; any write clears it */
#define NET_IRQ_ENABLE 0x10 /* NET_IRQ_ bits */
#define NET_IRQ_STATUS 0x14 /* NET_IRQ_ bits; writing 1 clears one */
#define NET_RX_DROPPED 0x18 /* frames lost on the way in */
#if __riscv_xlen == 64
/* Version 2 of the interface (RV64 machines): the ring bases are 64-bit. */
#define NET_TX_BASE 0x20    /* the transmit ring: physical address (64-bit), */
#define NET_TX_SIZE 0x28    /* size in descriptors (a power of two), */
#define NET_TX_HEAD 0x2c    /* the card's counter (read-only) */
#define NET_TX_TAIL 0x30    /* and the driver's: writing it sends */
#define NET_RX_BASE 0x38    /* the same for the receive ring */
#define NET_RX_SIZE 0x40
#define NET_RX_HEAD 0x44
#define NET_RX_TAIL 0x48
#else
/* Version 1 (RV32 machines): every register 32-bit. */
#define NET_TX_BASE 0x20
#define NET_TX_SIZE 0x24
#define NET_TX_HEAD 0x28
#define NET_TX_TAIL 0x2c
#define NET_RX_BASE 0x30
#define NET_RX_SIZE 0x34
#define NET_RX_HEAD 0x38
#define NET_RX_TAIL 0x3c
#endif

#define NET_STATUS_LINK 0x1
#define NET_IRQ_RX 0x1
#define NET_IRQ_TX 0x2
#define NET_IRQ_ERROR 0x4

/* A descriptor's status, written by the card when it hands it back. */
#define NET_DESC_OK 0

/* The rings this driver sets up, in descriptors (powers of two). Every
 * receive descriptor always holds a buffer from the pool (net.h), so
 * NIC_RX_RING buffers belong to the card for good; a transmit descriptor
 * holds its frame's buffer until the card has sent it. */
#define NIC_RX_RING 16
#define NIC_TX_RING 16

/* Frames waiting for a free transmit descriptor, at most. One more is
 * dropped and counted as a transmit error. */
#define NIC_TXQ_MAX 32

/* One descriptor, as the card reads and writes it: 16 bytes in version 2,
 * with a 64-bit buffer address, 8 in version 1. */
struct nic_desc {
#if __riscv_xlen == 64
    u64 addr; /* physical address of the buffer */
#else
    u32 addr;
#endif
    u16 len;    /* transmit: the frame's length; receive: the buffer's size, then the frame's */
    u16 status; /* NET_DESC_OK, or the card's error */
#if __riscv_xlen == 64
    u32 reserved; /* 0 */
#endif
};

/* Finds the cards in the sysinfo device table, prints a line for each and
 * sets up the first one: its rings, filled with buffers from the pool, and
 * its interrupts. Returns false (having said why) if there is no card, or
 * no memory for the rings and the pool. */
bool nic_init(void);

/* Enables the card's PLIC source: from now on nic_intr() runs. Call after
 * trap_init(), once the stack is ready for frames. */
void nic_start(void);

/* The interface's card: its MAC address (6 bytes), whether the link is up,
 * and the frames the card itself dropped (NET_RX_DROPPED). */
const u8 *nic_mac(void);
bool nic_link_up(void);
u32 nic_rx_dropped(void);

/* Hands the frame in `p` (p->len bytes from p->data) to the card, or queues
 * it until a transmit descriptor is free. Takes `p` in every case: it goes
 * back to the pool once the card has sent it, or at once if it can't be
 * sent (counted in tx_errors). Returns 0, or -ENOBUFS if it was dropped. */
int nic_transmit(struct pbuf *p);

/* The interrupt handler, called for each PLIC source the trap code claims:
 * if `irq` is the card's, acknowledges it, passes every received frame to
 * ether_input(), gives the buffers back to the card, frees the buffers of
 * sent frames and returns true. Returns false for any other source. */
bool nic_intr(u32 irq);

#endif
