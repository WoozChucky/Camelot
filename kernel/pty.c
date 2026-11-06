/* Pseudo-terminals (pty.h): pairs of a master and a slave, a page each,
 * the slave a terminal of the line discipline (tty.c). */
#include "pty.h"
#include "file.h"
#include "kalloc.h"
#include "poll.h"
#include "proc.h"
#include "spinlock.h"
#include "string.h"
#include "syscall.h"
#include "tty.h"

/* The slave's output waiting for the master to read it. */
#define PTY_OUT 2048

/* Bytes copied from user memory at a time. */
#define CHUNK 64

struct pty {
    struct tty tty; /* first: the discipline's tty is the pair */
    u32 index;      /* N of /dev/pts/N */
    bool master;    /* the master is open */
    u32 slaves;     /* open files of the slave */
    bool locked;    /* TIOCSPTLCK: the slave doesn't open */
    bool slave_closed; /* a slave file was open, and none is now */
    /* The output: [out_tail, out_head), the counters running on. */
    u32 out_head;
    u32 out_tail;
    char out[PTY_OUT];
};

_Static_assert(sizeof(struct pty) <= PGSIZE, "a pair fits a page");

static struct spinlock pty_lock;
static struct pty *ptys[NPTY];

void pty_init(void) {
    initlock(&pty_lock, "pty");
}

static u32 out_room(const struct pty *p) {
    return PTY_OUT - (p->out_head - p->out_tail);
}

/* Adds what fits of the `n` bytes at `s` to the output. */
static void out_put(struct pty *p, const char *s, u32 n) {
    u32 room = out_room(p);
    if (n > room) {
        n = room;
    }
    for (u32 i = 0; i < n; i++) {
        p->out[p->out_head % PTY_OUT] = s[i];
        p->out_head++;
    }
    if (n > 0) {
        wakeup(&p->out_head);
    }
}

/* The discipline's echo: into the output, what fits. */
static void pty_echo(struct tty *t, const char *s, u32 n) {
    out_put((struct pty *)t, s, n);
}

static const struct tty_ops pty_ops = {.echo = pty_echo};

/* ---- the master ---- */

int ptmx_open(struct file *f) {
    struct pty *p = kalloc();
    if (p == NULL) {
        return -ENOMEM;
    }
    memset(p, 0, sizeof *p);
    tty_init(&p->tty, &pty_lock, &pty_ops, false);
    p->master = true;
    p->locked = true; /* until unlockpt(), as Linux's */
    acquire(&pty_lock);
    u32 i = 0;
    while (i < NPTY && ptys[i] != NULL) {
        i++;
    }
    if (i == NPTY) {
        release(&pty_lock);
        kfree(p);
        return -ENOSPC;
    }
    p->index = i;
    ptys[i] = p;
    release(&pty_lock);
    f->priv = p;
    return 0;
}

/* Frees `p` if neither end is open: under pty_lock, which the caller then
 * lets go of before kfree()ing what this returns. */
static struct pty *unused(struct pty *p) {
    if (p->master || p->slaves > 0) {
        return NULL;
    }
    ptys[p->index] = NULL;
    return p;
}

void ptmx_close(struct file *f) {
    struct pty *p = f->priv;
    acquire(&pty_lock);
    p->master = false;
    tty_hangup(&p->tty);
    wakeup(&p->out_tail); /* the slave's writers fail */
    struct pty *gone = unused(p);
    release(&pty_lock);
    if (gone != NULL) {
        kfree(gone);
    }
}

isize ptmx_read(struct file *f, pagetable_t pt, uintptr dst, usize n) {
    struct pty *p = f->priv;
    if (!uvm_range_ok(pt, dst, n, PTE_W)) {
        return -EFAULT;
    }
    if (n == 0) {
        return 0;
    }
    acquire(&pty_lock);
    while (p->out_head == p->out_tail) {
        if (p->slave_closed) {
            release(&pty_lock);
            return -EIO;
        }
        if (proc_killed()) {
            release(&pty_lock);
            return -EINTR;
        }
        sleep(&p->out_head, &pty_lock);
    }
    u32 count = p->out_head - p->out_tail;
    if (count > n) {
        count = (u32)n;
    }
    /* Two pieces where the ring wraps. Checked above; copyout() never
     * sleeps. */
    u32 at = p->out_tail % PTY_OUT;
    u32 first = count < PTY_OUT - at ? count : PTY_OUT - at;
    if (copyout(pt, dst, p->out + at, first) != 0 ||
        (count > first && copyout(pt, dst + first, p->out, count - first) != 0)) {
        release(&pty_lock);
        return -EFAULT;
    }
    p->out_tail += count;
    wakeup(&p->out_tail);
    release(&pty_lock);
    return (isize)count;
}

/* Where the first Ctrl-C (tty_signal_char) is among the next TTY_IN bytes
 * of [src + from, src + n): its offset from `src`, or n if there is none. */
static usize next_signal(struct tty *t, pagetable_t pt, uintptr src, usize from, usize n) {
    char chunk[CHUNK];
    usize end = n - from > TTY_IN ? from + TTY_IN : n;
    while (from < end) {
        u32 count = end - from < CHUNK ? (u32)(end - from) : CHUNK;
        if (copyin(pt, chunk, src + from, count) != 0) {
            return n;
        }
        for (u32 i = 0; i < count; i++) {
            if (tty_signal_char(t, chunk[i])) {
                return from + i;
            }
        }
        from += count;
    }
    return n;
}

isize ptmx_write(struct file *f, pagetable_t pt, uintptr src, usize n) {
    struct pty *p = f->priv;
    if (!uvm_range_ok(pt, src, n, PTE_R)) {
        return -EFAULT;
    }
    char chunk[CHUNK];
    usize done = 0;
    isize err = 0;
    acquire(&pty_lock);
    while (done < n) {
        if (p->slave_closed) {
            err = -EIO;
            break;
        }
        u32 room = TTY_IN - (p->tty.in_head - p->tty.in_tail);
        if (room == 0) {
            /* Ctrl-C takes no room: one among the next bytes gets past
             * input nobody reads, those ahead of it, which have none,
             * dropped (it throws the input away anyway). */
            usize at = next_signal(&p->tty, pt, src, done, n);
            if (at < n) {
                char c;
                copyin(pt, &c, src + at, 1);
                tty_receive(&p->tty, c);
                done = at + 1;
                continue;
            }
            if (proc_killed()) {
                err = -EINTR;
                break;
            }
            sleep(p->tty.in, &pty_lock);
            continue;
        }
        u32 count = n - done < CHUNK ? (u32)(n - done) : CHUNK;
        if (count > room) {
            count = room;
        }
        /* Checked above; another thread may have unmapped it since. */
        if (copyin(pt, chunk, src + done, count) != 0) {
            err = -EFAULT;
            break;
        }
        for (u32 i = 0; i < count; i++) {
            tty_receive(&p->tty, chunk[i]); /* room for each, or acted on */
        }
        done += count;
    }
    release(&pty_lock);
    return done > 0 ? (isize)done : err;
}

long ptmx_ioctl(struct file *f, u32 request, uintptr arg) {
    struct pty *p = f->priv;
    pagetable_t pt = myproc()->pagetable;
    long r;
    acquire(&pty_lock);
    if (request == TIOCGPTN) {
        r = copyout(pt, arg, &p->index, sizeof p->index) != 0 ? -EFAULT : 0;
    } else if (request == TIOCSPTLCK) {
        int lock;
        r = copyin(pt, &lock, arg, sizeof lock) != 0 ? -EFAULT : 0;
        if (r == 0) {
            p->locked = lock != 0;
        }
    } else if (request == FIONREAD) {
        int count = (int)(p->out_head - p->out_tail);
        r = copyout(pt, arg, &count, sizeof count) != 0 ? -EFAULT : 0;
    } else if (request == TCFLSH) {
        /* The master's input is the slave's output, its output the
         * slave's input. */
        r = arg > TCIOFLUSH ? -EINVAL : 0;
        if (r == 0 && arg != TCOFLUSH) {
            p->out_tail = p->out_head;
            wakeup(&p->out_tail);
        }
        if (r == 0 && arg != TCIFLUSH) {
            r = tty_ioctl(&p->tty, TCFLSH, TCIFLUSH);
        }
    } else {
        r = tty_ioctl(&p->tty, request, arg);
    }
    release(&pty_lock);
    return r;
}

u32 ptmx_poll(struct file *f, struct poll_table *pt) {
    struct pty *p = f->priv;
    acquire(&pty_lock);
    poll_wait(pt, &p->out_head);
    poll_wait(pt, p->tty.in);
    u32 mask = 0;
    if (p->out_head != p->out_tail) {
        mask |= POLL_READY_IN;
    }
    if (p->slave_closed) {
        mask |= POLLHUP;
    } else if (tty_room(&p->tty)) {
        mask |= POLL_READY_OUT;
    }
    release(&pty_lock);
    return mask;
}

/* ---- the slave ---- */

int pts_open(struct file *f) {
    if (f->minor >= NPTY) {
        return -ENXIO;
    }
    acquire(&pty_lock);
    struct pty *p = ptys[f->minor];
    int r = 0;
    if (p == NULL) {
        r = -ENXIO;
    } else if (!p->master || p->locked) {
        r = -EIO;
    } else {
        p->slaves++;
        p->slave_closed = false;
        f->priv = p;
    }
    release(&pty_lock);
    return r;
}

void pts_close(struct file *f) {
    struct pty *p = f->priv;
    acquire(&pty_lock);
    p->slaves--;
    if (p->slaves == 0) {
        p->slave_closed = true;
        wakeup(&p->out_head); /* the master's readers: -EIO */
        wakeup(p->tty.in);    /* and its writers */
    }
    struct pty *gone = unused(p);
    release(&pty_lock);
    if (gone != NULL) {
        kfree(gone);
    }
}

isize pts_read(struct file *f, pagetable_t pt, uintptr dst, usize n) {
    struct pty *p = f->priv;
    return tty_read(&p->tty, pt, dst, n);
}

isize pts_write(struct file *f, pagetable_t pt, uintptr src, usize n) {
    struct pty *p = f->priv;
    if (!uvm_range_ok(pt, src, n, PTE_R)) {
        return -EFAULT;
    }
    char chunk[CHUNK];
    usize done = 0;
    isize err = 0;
    acquire(&pty_lock);
    while (done < n && err == 0) {
        u32 count = n - done < CHUNK ? (u32)(n - done) : CHUNK;
        if (copyin(pt, chunk, src + done, count) != 0) {
            err = -EFAULT;
            break;
        }
        for (u32 i = 0; i < count; i++) {
            char out[2];
            u32 m = tty_opost(p->tty.termios.c_oflag, chunk[i], out);
            while (p->master && out_room(p) < m) {
                if (proc_killed()) {
                    err = -EINTR;
                    break;
                }
                sleep(&p->out_tail, &pty_lock);
            }
            if (err == 0 && !p->master) {
                err = -EIO; /* hung up */
            }
            if (err != 0) {
                break;
            }
            out_put(p, out, m);
            done++;
        }
    }
    release(&pty_lock);
    return done > 0 ? (isize)done : err;
}

long pts_ioctl(struct file *f, u32 request, uintptr arg) {
    struct pty *p = f->priv;
    if (request == TIOCGPTN || request == TIOCSPTLCK) {
        return -ENOTTY; /* the master's */
    }
    acquire(&pty_lock);
    if (request == TCFLSH && arg <= TCIOFLUSH && arg != TCIFLUSH) {
        p->out_tail = p->out_head; /* the output not read yet */
        wakeup(&p->out_tail);
    }
    if (request == TCSETSW || request == TCSETSF) {
        /* Once the master has read what the slave wrote (TCSADRAIN). */
        while (p->master && p->out_head != p->out_tail) {
            if (proc_killed()) {
                release(&pty_lock);
                return -EINTR;
            }
            sleep(&p->out_tail, &pty_lock);
        }
    }
    long r = tty_ioctl(&p->tty, request, arg);
    release(&pty_lock);
    return r;
}

u32 pts_poll(struct file *f, struct poll_table *pt) {
    struct pty *p = f->priv;
    acquire(&pty_lock);
    /* Only a file open for reading edits and echoes, as a read would. */
    u32 mask = f->readable && tty_poll_in(&p->tty, pt) ? POLL_READY_IN : 0;
    poll_wait(pt, &p->out_tail);
    if (!p->master) {
        mask |= POLLHUP;
    } else if (out_room(p) >= 2) {
        mask |= POLL_READY_OUT;
    }
    release(&pty_lock);
    return mask;
}
