/* The kernel console: blocking reads and a line editor over the UART's
 * receive buffer. */
#include "console.h"
#include "riscv.h"
#include "uart.h"

#define CHAR_BS 0x08  /* backspace, Ctrl-H: what Windows consoles send */
#define CHAR_LF 0x0a  /* line feed */
#define CHAR_CR 0x0d  /* carriage return: Enter in a raw terminal */
#define CHAR_DEL 0x7f /* what most Unix terminals send for backspace */

/* The last line ended with a CR, so a LF right after it belongs to the same
 * line end (CR LF). */
static bool after_cr;

char console_getc(void) {
    char c;
    for (;;) {
        /* Interrupts off from the check to wfi, as in kmain's wait_ticks: a
         * byte that arrived in between would otherwise sit in the buffer
         * while wfi sleeps until the next interrupt. wfi still wakes on the
         * pending one, which is taken when intr_on() runs. */
        intr_off();
        bool got = uart_rx_pop(&c);
        if (!got) {
            wfi();
        }
        intr_on();
        if (got) {
            return c;
        }
    }
}

int console_readline(char *buf, int size) {
    int len = 0;
    for (;;) {
        char c = console_getc();
        if (c == CHAR_LF && after_cr) {
            after_cr = false;
            continue;
        }
        after_cr = c == CHAR_CR;
        if (c == CHAR_CR || c == CHAR_LF) {
            uart_putc('\n');
            break;
        }
        if (c == CHAR_BS || c == CHAR_DEL) {
            if (len > 0) {
                len--;
                uart_puts("\b \b"); /* back, blank it out, back again */
            }
        } else if (c >= ' ' && c < CHAR_DEL && len < size - 1) {
            buf[len] = c;
            len++;
            uart_putc(c);
        }
    }
    buf[len] = 0;
    return len;
}
