/* Polling UART driver: busy-waits on LSR.THRE before each byte. Interrupt-
 * driven input comes later, with the PLIC. */
#include "uart.h"
#include "mmio.h"

void uart_init(void) {
    mmio_write8(UART_BASE + UART_IER, 0); /* polling only */
    mmio_write8(UART_BASE + UART_LCR, UART_LCR_8N1);
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
