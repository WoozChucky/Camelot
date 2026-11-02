/* Interrupt-driven UART driver. The receive interrupt handler is the only code
 * that reads RBR: it fills a ring buffer that the console drains, so nothing
 * in the kernel busy-waits for input. Transmit still polls LSR.THRE, which
 * Grail always reports as set. */
#include "uart.h"
#include "mmio.h"

/* The receive ring buffer. Single producer (uart_intr, in the trap handler)
 * and single consumer (uart_rx_pop), so each index has one writer and no lock
 * is needed. The indices count bytes forever and wrap at 2^32; since
 * UART_RX_SIZE divides 2^32, `index % UART_RX_SIZE` stays right across the
 * wrap, and `rx_head - rx_tail` is always the number of buffered bytes. */
static volatile u8 rx_buf[UART_RX_SIZE];
static volatile u32 rx_head; /* bytes ever received: written by uart_intr */
static volatile u32 rx_tail; /* bytes ever taken: written by uart_rx_pop */

/* Set by uart_intr when the buffer was full and it turned the receive
 * interrupt off; uart_rx_pop turns it back on once the buffer is half
 * empty. */
static volatile bool rx_stalled;

void uart_init(void) {
    mmio_write8(UART_BASE + UART_LCR, UART_LCR_8N1);
    mmio_write8(UART_BASE + UART_IER, UART_IER_ERBFI);
}

void uart_putc(char c) {
    while ((mmio_read8(UART_BASE + UART_LSR) & UART_LSR_THRE) == 0) {
    }
    mmio_write8(UART_BASE + UART_THR, (u8)c);
}

void uart_puts(const char *s) {
    while (*s) {
        uart_putc(*s);
        s++;
    }
}

void uart_intr(void) {
    /* Drain RBR until LSR.DR clears, which lowers the UART's interrupt line;
     * kernel_trap then completes the claim. */
    while (mmio_read8(UART_BASE + UART_LSR) & UART_LSR_DR) {
        if (rx_head - rx_tail == UART_RX_SIZE) {
            /* Full. Leaving the byte in RBR keeps the line high, so the
             * interrupt would fire again as soon as this one completes and the
             * console would never get to run: turn it off instead. */
            rx_stalled = true;
            mmio_write8(UART_BASE + UART_IER, 0);
            return;
        }
        rx_buf[rx_head % UART_RX_SIZE] = mmio_read8(UART_BASE + UART_RBR);
        rx_head = rx_head + 1;
    }
}

bool uart_rx_pop(char *c) {
    if (rx_tail == rx_head) {
        return false;
    }
    *c = (char)rx_buf[rx_tail % UART_RX_SIZE];
    rx_tail = rx_tail + 1;
    if (rx_stalled && rx_head - rx_tail <= UART_RX_SIZE / 2) {
        /* Half empty again: let the bytes still waiting in the UART in. (Not
         * at the first free byte, or a long paste would take one interrupt
         * per byte.) */
        rx_stalled = false;
        mmio_write8(UART_BASE + UART_IER, UART_IER_ERBFI);
    }
    return true;
}
