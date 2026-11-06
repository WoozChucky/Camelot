/* The kernel console: the read and write system calls on the UART and the
 * framebuffer console, through the terminals' line discipline (tty.c),
 * which edits and echoes lines as it does for the pseudo-terminals.
 *
 * Typed bytes come from two places: the UART's receive ring, which its
 * interrupt fills (console_intr), and the keyboard while no program reads
 * it (console_input). The discipline takes the UART's as it has room for
 * them (cons_pull), so a burst of input waits in the UART, whose interrupt
 * stays off while its ring is full, rather than being lost; the keyboard's
 * go straight in, and are dropped when the input is full, as its own FIFO
 * drops them. Echoing happens when a process reads, so input typed ahead is
 * echoed after the prompt that asks for it.
 *
 * Locking: cons_lock is the discipline's lock (the line, the input, the
 * termios) and protects the UART's receive ring (the interrupt handler
 * fills it, readers drain it); readers sleep with it. Output takes the
 * UART's own lock, uart_tx_lock, which comes after cons_lock in the lock
 * order (the echo is printed under both). */
#include "console.h"
#include "fbcon.h"
#include "file.h"
#include "poll.h"
#include "proc.h"
#include "spinlock.h"
#include "syscall.h"
#include "tty.h"
#include "uart.h"

/* Bytes copied out of user memory at a time by console_write(). */
#define WRITE_CHUNK 64

static struct spinlock cons_lock;
static struct tty cons;

/* Prints `n` bytes on the UART and the framebuffer console. */
static void cons_echo(struct tty *t, const char *s, u32 n) {
    (void)t;
    acquire(&uart_tx_lock);
    for (u32 i = 0; i < n; i++) {
        uart_putc(s[i]);
        fbcon_putc(s[i]);
    }
    release(&uart_tx_lock);
}

/* Moves the UART's received bytes into the discipline while it has room,
 * and Ctrl-C even when it has none (it takes none). */
static void cons_pull(struct tty *t) {
    char c;
    while (uart_rx_peek(&c) && (tty_room(t) || tty_signal_char(t, c))) {
        uart_rx_pop(&c);
        tty_receive(t, c);
    }
}

static const struct tty_ops cons_ops = {.echo = cons_echo, .pull = cons_pull};

void console_init(void) {
    initlock(&cons_lock, "console");
    tty_init(&cons, &cons_lock, &cons_ops, true);
}

void console_input(char c) {
    acquire(&cons_lock);
    /* What the UART's ring holds came first. */
    cons_pull(&cons);
    tty_receive(&cons, c); /* no room: dropped */
    release(&cons_lock);
}

void console_intr(void) {
    acquire(&cons_lock);
    uart_intr();
    /* Taken now, so that Ctrl-C acts while nobody reads. */
    cons_pull(&cons);
    release(&cons_lock);
}

u32 console_poll(struct file *f, struct poll_table *pt) {
    u32 mask = POLL_READY_OUT;
    if (!f->readable) {
        return mask;
    }
    acquire(&cons_lock);
    if (tty_poll_in(&cons, pt)) {
        mask |= POLL_READY_IN;
    }
    release(&cons_lock);
    return mask;
}

isize console_read(struct file *f, pagetable_t pt, uintptr dst, usize n) {
    (void)f;
    return tty_read(&cons, pt, dst, n);
}

isize console_write(struct file *f, pagetable_t pt, uintptr src, usize n) {
    (void)f;
    if (!uvm_range_ok(pt, src, n, PTE_R)) {
        return -EFAULT;
    }
    /* The output processing as it is now: the termios under the lock,
     * the printing under the UART's only. */
    acquire(&cons_lock);
    u32 oflag = cons.termios.c_oflag;
    release(&cons_lock);
    char chunk[WRITE_CHUNK];
    char out[2];
    for (usize done = 0; done < n;) {
        u32 count = n - done < WRITE_CHUNK ? (u32)(n - done) : WRITE_CHUNK;
        /* Checked above, but another thread of the process may have
         * unmapped part of the buffer since (sbrk). */
        if (copyin(pt, chunk, src + done, count) != 0) {
            return done > 0 ? (isize)done : -EFAULT;
        }
        acquire(&uart_tx_lock);
        for (u32 i = 0; i < count; i++) {
            u32 m = tty_opost(oflag, chunk[i], out);
            for (u32 j = 0; j < m; j++) {
                uart_putc(out[j]);
                fbcon_putc(out[j]);
            }
        }
        release(&uart_tx_lock);
        done += count;
    }
    return (isize)n;
}

long console_ioctl(struct file *f, u32 request, uintptr arg) {
    (void)f;
    acquire(&cons_lock);
    long r = tty_ioctl(&cons, request, arg);
    release(&cons_lock);
    return r;
}
