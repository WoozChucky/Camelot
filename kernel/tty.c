/* Terminals' line discipline (tty.h), after Linux's n_tty.c: the console's
 * and the pseudo-terminals' input, edited into lines or passed as it
 * comes, their echo, and their termios.
 *
 * Input is mapped as it arrives (tty_receive) and edited as it is read:
 * cook_line() takes the bytes waiting into the line, echoing them, until
 * the line ends; a read in non-canonical mode takes them as they are,
 * echoing them then. Only the interrupt characters act at once, since the
 * program they would stop may never read. */
#include "tty.h"
#include "poll.h"
#include "proc.h"
#include "spinlock.h"
#include "string.h"
#include "syscall.h"
#include "trap.h"

#define CHAR_BS 0x08  /* backspace, Ctrl-H: what Windows consoles send */
#define CHAR_DEL 0x7f /* VERASE's default, what most Unix terminals send */

/* Bytes a read in non-canonical mode copies out at a time. */
#define RAW_CHUNK 64

/* Linux's control characters for a new terminal (INIT_C_CC). */
static const u8 std_cc[NCCS] = {
    [VINTR] = 0x03,   /* Ctrl-C */
    [VQUIT] = 0x1c,   /* Ctrl-\ */
    [VERASE] = 0x7f,  /* DEL */
    [VKILL] = 0x15,   /* Ctrl-U */
    [VEOF] = 0x04,    /* Ctrl-D */
    [VMIN] = 1,
    [VSTART] = 0x11,  /* Ctrl-Q */
    [VSTOP] = 0x13,   /* Ctrl-S */
    [VSUSP] = 0x1a,   /* Ctrl-Z */
    [VREPRINT] = 0x12, /* Ctrl-R */
    [VDISCARD] = 0x0f, /* Ctrl-O */
    [VWERASE] = 0x17, /* Ctrl-W */
    [VLNEXT] = 0x16,  /* Ctrl-V */
};

void tty_init(struct tty *t, struct spinlock *lock, const struct tty_ops *ops, bool console) {
    memset(t, 0, sizeof *t);
    t->lock = lock;
    t->ops = ops;
    /* Linux's tty_std_termios. */
    t->termios.c_iflag = ICRNL | IXON;
    t->termios.c_oflag = OPOST | ONLCR;
    t->termios.c_cflag = B38400 | CS8 | CREAD | HUPCL;
    t->termios.c_lflag = ISIG | ICANON | ECHO | ECHOE | ECHOK | ECHOCTL | ECHOKE | IEXTEN;
    memcpy(t->termios.c_cc, std_cc, NCCS);
    if (console) {
        /* As the console has always been: its UART gets what is written
         * as it is, and a line is edited with erasing only. */
        t->termios.c_oflag = 0;
        t->termios.c_cc[VKILL] = 0;
        t->termios.c_cc[VWERASE] = 0;
    } else {
        /* A pty's slave has no line to hang up (Linux's pty driver). */
        t->termios.c_cflag &= ~(u32)HUPCL;
    }
}

bool tty_room(const struct tty *t) {
    return t->in_head - t->in_tail < TTY_IN;
}

bool tty_signal_char(const struct tty *t, char c) {
    const struct termios *tio = &t->termios;
    return (tio->c_lflag & ISIG) != 0 && c != 0 &&
           (c == (char)tio->c_cc[VINTR] || c == (char)tio->c_cc[VQUIT]) && t->fg != 0 &&
           proc_exists(t->fg);
}

u32 tty_opost(u32 oflag, char c, char out[2]) {
    if (c == '\n' && (oflag & OPOST) != 0 && (oflag & ONLCR) != 0) {
        out[0] = '\r';
        out[1] = '\n';
        return 2;
    }
    out[0] = c;
    return 1;
}

/* Echoes the `n` bytes at `s`, through the output processing. */
static void echo(struct tty *t, const char *s, u32 n) {
    char buf[2 * RAW_CHUNK];
    u32 len = 0;
    for (u32 i = 0; i < n; i++) {
        if (len + 2 > sizeof buf) {
            t->ops->echo(t, buf, len);
            len = 0;
        }
        len += tty_opost(t->termios.c_oflag, s[i], buf + len);
    }
    if (len > 0) {
        t->ops->echo(t, buf, len);
    }
}

static void echo_str(struct tty *t, const char *s) {
    echo(t, s, (u32)strlen(s));
}

/* Throws away the input: what waits and the line. Writers waiting for
 * room get it. */
static void flush_input(struct tty *t) {
    t->in_tail = t->in_head;
    t->line_len = 0;
    t->line_taken = 0;
    t->line_done = false;
    t->after_cr = false;
    wakeup(t->in);
}

/* VINTR or VQUIT `c` with ISIG: kills the foreground process, as Linux's
 * SIGINT and SIGQUIT end a program that doesn't catch them. */
static void interrupt(struct tty *t, char c) {
    if (t->fg == 0 || !proc_exists(t->fg)) {
        return; /* no foreground (any more): dropped, as the console always did */
    }
    u32 lflag = t->termios.c_lflag;
    if ((lflag & NOFLSH) == 0) {
        flush_input(t);
    }
    if ((lflag & ECHO) != 0) {
        if ((lflag & ECHOCTL) != 0) {
            char ctl[2] = {'^', (char)(c ^ 0x40)};
            echo(t, ctl, 2);
        }
        echo_str(t, "\n");
    }
    proc_kill_pid(t->fg);
}

bool tty_receive(struct tty *t, char c) {
    const struct termios *tio = &t->termios;
    if ((tio->c_lflag & ISIG) != 0 && c != 0) {
        if (c == (char)tio->c_cc[VINTR] || c == (char)tio->c_cc[VQUIT]) {
            interrupt(t, c);
            return true;
        }
        if (c == (char)tio->c_cc[VSUSP]) {
            return true; /* Camelot doesn't stop processes */
        }
    }
    if (!tty_room(t)) {
        return false;
    }
    bool cr = false;
    if (c == '\r') {
        if ((tio->c_iflag & IGNCR) != 0) {
            t->after_cr = false;
            return true;
        }
        if ((tio->c_iflag & ICRNL) != 0) {
            c = '\n';
            cr = true;
        }
    } else if (c == '\n') {
        if (t->after_cr && (tio->c_lflag & ICANON) != 0) {
            /* CR LF: the CR ended the line already. */
            t->after_cr = false;
            return true;
        }
        if ((tio->c_iflag & INLCR) != 0) {
            c = '\r';
        }
    }
    t->after_cr = cr;
    t->in[t->in_head % TTY_IN] = c;
    t->in_head++;
    t->last_rx = ticks;
    wakeup(t);
    return true;
}

/* Takes back `count` characters of the line not yet read, echoing their
 * erasure: BS, space, BS each (ECHOE). */
static void erase(struct tty *t, u32 count) {
    bool visible = (t->termios.c_lflag & (ECHO | ECHOE)) == (ECHO | ECHOE);
    for (u32 i = 0; i < count && t->line_len > t->line_taken; i++) {
        t->line_len--;
        if (visible) {
            echo_str(t, "\b \b");
        }
    }
}

/* The characters VWERASE takes: the spaces before the cursor, then the
 * word before them. */
static u32 word_length(const struct tty *t) {
    u32 i = t->line_len;
    while (i > t->line_taken && t->line[i - 1] == ' ') {
        i--;
    }
    while (i > t->line_taken && t->line[i - 1] != ' ') {
        i--;
    }
    return t->line_len - i;
}

/* Canonical mode's editing of one byte into the line. */
static void edit(struct tty *t, char c) {
    const u8 *cc = t->termios.c_cc;
    u32 lflag = t->termios.c_lflag;
    bool echoing = (lflag & ECHO) != 0;
    u8 b = (u8)c;
    if (c == '\n' || (cc[VEOL] != 0 && b == cc[VEOL])) {
        /* There is always room: characters stop one byte short, and
         * nothing is added to a line once it has ended (line_done stays
         * set until it has all been read, whatever the mode). */
        if (t->line_len < TTY_LINE_MAX) {
            t->line[t->line_len] = c;
            t->line_len++;
        }
        t->line_done = true;
        if (echoing || (c == '\n' && (lflag & ECHONL) != 0)) {
            echo(t, &c, 1);
        }
    } else if (cc[VEOF] != 0 && b == cc[VEOF]) {
        t->line_done = true;
    } else if ((cc[VERASE] != 0 && b == cc[VERASE]) || b == CHAR_BS) {
        erase(t, 1);
    } else if (cc[VWERASE] != 0 && b == cc[VWERASE]) {
        erase(t, word_length(t));
    } else if (cc[VKILL] != 0 && b == cc[VKILL]) {
        if ((lflag & (ECHOKE | ECHOE)) != 0) {
            erase(t, t->line_len - t->line_taken);
        } else {
            t->line_len = t->line_taken;
            if (echoing && (lflag & ECHOK) != 0) {
                echo_str(t, "\n");
            }
        }
    } else if (b >= ' ' && b < CHAR_DEL && t->line_len < TTY_LINE_MAX - 1) {
        t->line[t->line_len] = c;
        t->line_len++;
        if (echoing) {
            echo(t, &c, 1);
        }
    }
}

/* Whether input waits: in the ring, or brought in from elsewhere now that
 * there is room. */
static bool input_waiting(struct tty *t) {
    if (t->in_tail == t->in_head && t->ops->pull != NULL) {
        t->ops->pull(t);
    }
    return t->in_tail != t->in_head;
}

/* Canonical mode: edits the input waiting into the line until it ends or
 * none is left. Whether it has ended. */
static bool cook_line(struct tty *t) {
    bool took = false;
    /* As the ring empties, input waiting elsewhere comes in: a line longer
     * than the ring holds goes on in the UART's. */
    while (!t->line_done && input_waiting(t)) {
        char c = t->in[t->in_tail % TTY_IN];
        t->in_tail++;
        took = true;
        edit(t, c);
    }
    if (took) {
        wakeup(t->in);
    }
    return t->line_done;
}

/* Non-canonical mode: the bytes a read could take. */
static u32 raw_avail(const struct tty *t) {
    return (t->line_len - t->line_taken) + (t->in_head - t->in_tail);
}

static void reset_line(struct tty *t) {
    t->line_len = 0;
    t->line_taken = 0;
    t->line_done = false;
}

/* Sleeps on the readers' channel, letting go of the lock; `timed`: on the
 * tick counter too (a poll table, poll.h), for VTIME. The caller looks
 * again either way. */
static void tty_sleep(struct tty *t, bool timed) {
    if (!timed) {
        sleep(t, t->lock);
        return;
    }
    struct poll_table table;
    poll_begin(&table);
    poll_wait(&table, t);
    poll_wait(&table, (const void *)&ticks);
    release(t->lock);
    poll_sleep(&table);
    poll_end();
    acquire(t->lock);
}

/* Non-canonical mode: whether a read may take what there is now (VMIN and
 * VTIME, as Linux's n_tty_read), and if not whether its wait is timed. */
static bool raw_ready(const struct tty *t, usize n, u32 start, bool *timed) {
    u32 vmin = t->termios.c_cc[VMIN];
    u32 vtime = t->termios.c_cc[VTIME];
    u32 avail = raw_avail(t);
    u32 want = vmin < n ? vmin : (u32)n;
    *timed = false;
    if (vmin == 0) {
        if (vtime == 0 || avail > 0 || ticks - start >= vtime) {
            return true;
        }
        *timed = true; /* VTIME from the read's start */
        return false;
    }
    if (avail >= want) {
        return true;
    }
    if (vtime > 0 && avail > 0) {
        /* VTIME between bytes, once the first has come. */
        if (ticks - t->last_rx >= vtime) {
            return true;
        }
        *timed = true;
    }
    return false;
}

/* Non-canonical mode: takes up to `n` bytes to user address `dst`, what a
 * line held first, then the input, echoing it. Their count, or -EFAULT. */
static isize raw_take(struct tty *t, pagetable_t pt, uintptr dst, usize n) {
    bool echoing = (t->termios.c_lflag & ECHO) != 0;
    char chunk[RAW_CHUNK];
    usize done = 0;
    bool took = false;
    bool fault = false;
    while (done < n) {
        u32 len = 0;
        while (done + len < n && len < RAW_CHUNK) {
            char c;
            if (t->line_taken < t->line_len) {
                c = t->line[t->line_taken];
                t->line_taken++;
                if (t->line_taken == t->line_len) {
                    reset_line(t);
                }
            } else if (input_waiting(t)) {
                c = t->in[t->in_tail % TTY_IN];
                t->in_tail++;
                took = true;
                if (echoing) {
                    echo(t, &c, 1);
                }
            } else {
                break;
            }
            chunk[len] = c;
            len++;
        }
        if (len == 0) {
            break;
        }
        /* Checked by the caller; another thread may have unmapped it since. */
        if (copyout(pt, dst + done, chunk, len) != 0) {
            fault = true;
            break;
        }
        done += len;
    }
    if (took) {
        wakeup(t->in);
    }
    return fault && done == 0 ? -EFAULT : (isize)done;
}

isize tty_read(struct tty *t, pagetable_t pt, uintptr dst, usize n) {
    if (!uvm_range_ok(pt, dst, n, PTE_W)) {
        return -EFAULT;
    }
    if (n == 0) {
        return 0;
    }
    acquire(t->lock);
    u32 start = ticks;
    for (;;) {
        if (t->ops->pull != NULL) {
            t->ops->pull(t);
        }
        bool timed = false;
        bool ready;
        if ((t->termios.c_lflag & ICANON) != 0) {
            ready = cook_line(t);
        } else {
            ready = raw_ready(t, n, start, &timed);
        }
        if (ready || t->hung) {
            break;
        }
        if (proc_killed()) {
            release(t->lock);
            return -EINTR;
        }
        tty_sleep(t, timed);
    }
    isize r;
    if ((t->termios.c_lflag & ICANON) == 0) {
        r = raw_take(t, pt, dst, n);
    } else if (!t->line_done && t->line_len == t->line_taken) {
        r = 0; /* hung up: the end */
    } else {
        /* A whole line, or what a hung-up terminal had of one. */
        u32 count = t->line_len - t->line_taken;
        if (count > n) {
            count = (u32)n;
        }
        /* Checked above; copyout() never sleeps, so it may run under the
         * lock. */
        if (copyout(pt, dst, t->line + t->line_taken, count) != 0) {
            release(t->lock);
            return -EFAULT;
        }
        t->line_taken += count;
        if (t->line_taken == t->line_len) {
            reset_line(t);
        }
        r = (isize)count;
    }
    release(t->lock);
    return r;
}

bool tty_poll_in(struct tty *t, struct poll_table *pt) {
    poll_wait(pt, t);
    if (t->ops->pull != NULL) {
        t->ops->pull(t);
    }
    if (t->hung) {
        return true;
    }
    if ((t->termios.c_lflag & ICANON) != 0) {
        return cook_line(t);
    }
    /* Linux's input_available_p(): VMIN bytes, unless VTIME times the read
     * or VMIN is 0. */
    u32 vmin = t->termios.c_cc[VMIN];
    u32 amt = t->termios.c_cc[VTIME] == 0 && vmin > 0 ? vmin : 1;
    return raw_avail(t) >= amt;
}

long tty_ioctl(struct tty *t, u32 request, uintptr arg) {
    pagetable_t pt = myproc()->pagetable;
    switch (request) {
    case TCGETS:
        return copyout(pt, arg, &t->termios, sizeof t->termios) != 0 ? -EFAULT : 0;
    case TCSETS:
    case TCSETSW:
    case TCSETSF: {
        struct termios n;
        if (copyin(pt, &n, arg, sizeof n) != 0) {
            return -EFAULT;
        }
        if (request == TCSETSF) {
            flush_input(t);
        }
        /* A line not read yet is read first in either mode: raw reads take
         * its bytes, and a canonical one, ended, gets nothing added. */
        t->termios = n;
        wakeup(t); /* a read may take what there is under the new mode */
        return 0;
    }
    case TIOCGWINSZ:
        return copyout(pt, arg, &t->winsize, sizeof t->winsize) != 0 ? -EFAULT : 0;
    case TIOCSWINSZ:
        return copyin(pt, &t->winsize, arg, sizeof t->winsize) != 0 ? -EFAULT : 0;
    case TIOCGPGRP:
        return copyout(pt, arg, &t->fg, sizeof t->fg) != 0 ? -EFAULT : 0;
    case TIOCSPGRP: {
        int pid;
        if (copyin(pt, &pid, arg, sizeof pid) != 0) {
            return -EFAULT;
        }
        if (pid < 0) {
            return -EINVAL;
        }
        if (pid != 0 && !proc_exists(pid)) {
            return -ESRCH;
        }
        t->fg = pid;
        return 0;
    }
    case FIONREAD: {
        if (t->ops->pull != NULL) {
            t->ops->pull(t);
        }
        int count = 0;
        if ((t->termios.c_lflag & ICANON) == 0) {
            count = (int)raw_avail(t);
        } else if (cook_line(t)) {
            count = (int)(t->line_len - t->line_taken);
        }
        return copyout(pt, arg, &count, sizeof count) != 0 ? -EFAULT : 0;
    }
    case TCFLSH:
        if (arg > TCIOFLUSH) {
            return -EINVAL;
        }
        if (arg != TCOFLUSH) {
            flush_input(t);
        }
        return 0;
    default:
        return -ENOTTY;
    }
}

void tty_hangup(struct tty *t) {
    t->hung = true;
    wakeup(t);
    wakeup(t->in);
}
