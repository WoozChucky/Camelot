/* The kernel console: the read and write system calls on the UART, with a
 * line editor in between. Typed bytes wait in the UART driver's ring buffer
 * until a process reads; console_read() then edits them into the line
 * buffer here, echoing as it goes, and sleeps while the line is incomplete.
 * Echoing at read time rather than in the interrupt handler means input
 * typed ahead is echoed when a program asks for it, so the echo lands after
 * the prompt and not in the middle of another program's output. */
#include "console.h"
#include "proc.h"
#include "syscall.h"
#include "uart.h"

#define CHAR_EOT 0x04 /* Ctrl-D: end of input on an empty line */
#define CHAR_BS 0x08  /* backspace, Ctrl-H: what Windows consoles send */
#define CHAR_LF 0x0a  /* line feed */
#define CHAR_CR 0x0d  /* carriage return: Enter in a raw terminal */
#define CHAR_DEL 0x7f /* what most Unix terminals send for backspace */

/* Bytes copied out of user memory at a time by console_write(). */
#define WRITE_CHUNK 64

/* The line being typed or read: `line_len` bytes, of which `line_taken` have
 * been returned by reads. Once `line_done`, nothing more is added until every
 * byte has been read. The buffer's address is also what readers sleep on. */
static char line[CONSOLE_LINE_MAX];
static u32 line_len;
static u32 line_taken;
static bool line_done;

/* The last line ended with a CR, so a LF right after it belongs to the same
 * line end (CR LF). */
static bool after_cr;

void console_intr(void) {
    uart_intr();
    proc_wakeup(line);
}

/* Takes typed bytes from the UART's buffer into the line until the line is
 * complete or there are none left. Returns whether it is complete. */
static bool edit_line(void) {
    char c;
    while (!line_done && uart_rx_pop(&c)) {
        if (c == CHAR_LF && after_cr) {
            after_cr = false;
            continue;
        }
        after_cr = c == CHAR_CR;
        if (c == CHAR_CR || c == CHAR_LF) {
            /* There is always room: characters stop one byte short. */
            line[line_len] = '\n';
            line_len++;
            line_done = true;
            uart_putc('\n');
        } else if (c == CHAR_EOT) {
            line_done = true;
        } else if (c == CHAR_BS || c == CHAR_DEL) {
            if (line_len > 0) {
                line_len--;
                uart_puts("\b \b"); /* back, blank it out, back again */
            }
        } else if (c >= ' ' && c < CHAR_DEL && line_len < CONSOLE_LINE_MAX - 1) {
            line[line_len] = c;
            line_len++;
            uart_putc(c);
        }
    }
    return line_done;
}

int console_read(pagetable_t pt, u32 dst, u32 n) {
    if (!uvm_range_ok(pt, dst, n, PTE_W)) {
        return -EFAULT;
    }
    if (n == 0) {
        return 0;
    }
    while (!edit_line()) {
        proc_sleep(line);
    }
    u32 count = line_len - line_taken;
    if (count > n) {
        count = n;
    }
    /* Checked above, and nothing unmaps it while the process sleeps. */
    if (copyout(pt, dst, line + line_taken, count) != 0) {
        return -EFAULT;
    }
    line_taken += count;
    if (line_taken == line_len) {
        line_len = 0;
        line_taken = 0;
        line_done = false;
    }
    return (int)count;
}

int console_write(pagetable_t pt, u32 src, u32 n) {
    if (!uvm_range_ok(pt, src, n, PTE_R)) {
        return -EFAULT;
    }
    char chunk[WRITE_CHUNK];
    for (u32 done = 0; done < n;) {
        u32 count = n - done < WRITE_CHUNK ? n - done : WRITE_CHUNK;
        copyin(pt, chunk, src + done, count);
        for (u32 i = 0; i < count; i++) {
            uart_putc(chunk[i]);
        }
        done += count;
    }
    return (int)n;
}
