/* Polling driver for the 16550-compatible UART (spec section 7). */
#ifndef CAMELOT_UART_H
#define CAMELOT_UART_H

#define UART_BASE 0x10000000

/* Register offsets. */
#define UART_THR 0 /* transmit holding (write) */
#define UART_RBR 0 /* receive buffer (read) */
#define UART_IER 1 /* interrupt enable */
#define UART_LCR 3 /* line control */
#define UART_LSR 5 /* line status */

#define UART_LCR_8N1 0x03
#define UART_LSR_DR 0x01   /* data ready */
#define UART_LSR_THRE 0x20 /* transmit holding register empty */

void uart_init(void);
void uart_putc(char c);
void uart_puts(const char *s);

#endif
