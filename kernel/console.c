/* The kernel console: the read and write system calls on the UART, with a
 * line editor in between. Typed bytes wait in the UART driver's ring buffer
 * until a process reads; console_read() then edits them into the line
 * buffer here, echoing as it goes, and sleeps while the line is incomplete.
 * Echoing at read time rather than in the interrupt handler means input
 * typed ahead is echoed when a program asks for it, so the echo lands after
 * the prompt and not in the middle of another program's output.
 *
 * Locking: cons_lock protects the line and the UART's receive ring (the
 * interrupt handler fills it, readers drain it); readers sleep on the line
 * with it. Output takes the UART's own lock, uart_tx_lock, which comes
 * after cons_lock in the lock order (the echo is printed under both). */
#include "console.h"
#include "proc.h"
#include "spinlock.h"
#include "syscall.h"
#include "uart.h"

#define CHAR_EOT 0x04 /* Ctrl-D: end of input on an empty line */
#define CHAR_BS 0x08  /* backspace, Ctrl-H: what Windows consoles send */
#define CHAR_LF 0x0a  /* line feed */
#define CHAR_CR 0x0d  /* carriage return: Enter in a raw terminal */
#define CHAR_DEL 0x7f /* what most Unix terminals send for backspace */

/* Bytes copied out of user memory at a time by console_write(). */
#define WRITE_CHUNK 64

static struct spinlock cons_lock;

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

void console_init(void) {
    initlock(&cons_lock, "console");
}

void console_intr(void) {
    acquire(&cons_lock);
    uart_intr();
    wakeup(line);
    release(&cons_lock);
}

/* Echoes `s` for the line editor. */
static void echo(const char *s) {
    acquire(&uart_tx_lock);
    uart_puts(s);
    release(&uart_tx_lock);
}

/* Takes typed bytes from the UART's buffer into the line until the line is
 * complete or there are none left. Returns whether it is complete. Called
 * with cons_lock held. */
static bool edit_line(void) {
    char c;
    char out[2];
    out[1] = 0;
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
            echo("\n");
        } else if (c == CHAR_EOT) {
            line_done = true;
        } else if (c == CHAR_BS || c == CHAR_DEL) {
            if (line_len > 0) {
                line_len--;
                echo("\b \b"); /* back, blank it out, back again */
            }
        } else if (c >= ' ' && c < CHAR_DEL && line_len < CONSOLE_LINE_MAX - 1) {
            line[line_len] = c;
            line_len++;
            out[0] = c;
            echo(out);
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
    acquire(&cons_lock);
    while (!edit_line()) {
        if (proc_killed()) {
            release(&cons_lock);
            return 0; /* never seen: the caller ends on its way out */
        }
        sleep(line, &cons_lock);
    }
    u32 count = line_len - line_taken;
    if (count > n) {
        count = n;
    }
    /* Checked above, and nothing unmaps it while the process sleeps.
     * copyout() never sleeps, so it may run under the lock. */
    if (copyout(pt, dst, line + line_taken, count) != 0) {
        release(&cons_lock);
        return -EFAULT;
    }
    line_taken += count;
    if (line_taken == line_len) {
        line_len = 0;
        line_taken = 0;
        line_done = false;
    }
    release(&cons_lock);
    return (int)count;
}

int console_write(pagetable_t pt, u32 src, u32 n) {
    if (!uvm_range_ok(pt, src, n, PTE_R)) {
        return -EFAULT;
    }
    char chunk[WRITE_CHUNK];
    for (u32 done = 0; done < n;) {
        u32 count = n - done < WRITE_CHUNK ? n - done : WRITE_CHUNK;
        /* Checked above, but another thread of the process may have
         * unmapped part of the buffer since (sbrk). */
        if (copyin(pt, chunk, src + done, count) != 0) {
            return done > 0 ? (int)done : -EFAULT;
        }
        acquire(&uart_tx_lock);
        for (u32 i = 0; i < count; i++) {
            uart_putc(chunk[i]);
        }
        release(&uart_tx_lock);
        done += count;
    }
    return (int)n;
}
