/* Driver for the 16550-compatible UART (spec section 7). Receive is
 * interrupt-driven: the UART holds PLIC source PLIC_IRQ_UART high while a
 * byte is waiting, and uart_intr() moves the bytes into a ring buffer that
 * uart_rx_pop() reads. Transmit polls LSR.THRE, which is always 1 in Grail. */
#ifndef CAMELOT_UART_H
#define CAMELOT_UART_H

#ifndef __ASSEMBLER__
#include "spinlock.h"
#include "types.h"
#endif

#define UART_BASE 0x10000000

/* Register offsets. */
#define UART_THR 0 /* transmit holding (write) */
#define UART_RBR 0 /* receive buffer (read) */
#define UART_IER 1 /* interrupt enable */
#define UART_LCR 3 /* line control */
#define UART_LSR 5 /* line status */

#define UART_IER_ERBFI 0x01 /* interrupt while received data is available */
#define UART_LCR_8N1 0x03
#define UART_LSR_DR 0x01   /* data ready */
#define UART_LSR_THRE 0x20 /* transmit holding register empty */

/* Size of the receive ring buffer in bytes, a power of two. */
#define UART_RX_SIZE 128

#ifndef __ASSEMBLER__
/* Where the registers are reached: UART_BASE (with paging off), the
 * fixmap's page (from setup_vm(), vm.h), then the kernel's ioremap() of it
 * (uart_map()). */
extern uintptr uart_regs;

/* 8N1 with the receive interrupt on, through uart_regs as it is: the first
 * thing kmain does, as early as the boot can print. The interrupt reaches the
 * kernel once the PLIC and sie.SEIE are set up (plic_init, trap_init); bytes
 * typed before that wait in the UART. */
void uart_init(void);

/* Points uart_regs at the UART's ioremap() mapping, once kvminit() has made
 * ioremap() work (as Linux's 8250 driver takes over from earlycon). */
void uart_map(void);

/* Held while writing to the UART, by the callers of uart_putc() and
 * uart_puts(): kprintf() for a whole message, console_write() for each
 * chunk, the console's echo for each edit. So what one hart prints comes out
 * whole, not mixed with another's. A leaf of the lock order. */
extern struct spinlock uart_tx_lock;

/* Write a byte, or a string, polling LSR.THRE. The caller holds
 * uart_tx_lock. */
void uart_putc(char c);
void uart_puts(const char *s);

/* Writes a byte at the UART's physical address, with no lock: for the
 * messages of the early boot code (setup_vm()), which runs with paging off,
 * before the fixmap is there. */
void uart_putc_phys(char c);

/* The receive interrupt handler, called by kernel_trap for PLIC_IRQ_UART:
 * moves bytes from RBR into the ring buffer until LSR.DR is clear. If the
 * buffer fills up first, it turns the receive interrupt off and leaves the
 * rest in the UART until uart_rx_pop() has emptied half of the buffer. */
void uart_intr(void);

/* Takes the oldest received byte out of the ring buffer into `*c`. Returns
 * false if the buffer is empty. Never touches RBR. */
bool uart_rx_pop(char *c);

/* The same, leaving the byte in the buffer. */
bool uart_rx_peek(char *c);

#endif

#endif
