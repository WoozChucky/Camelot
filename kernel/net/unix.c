/* Local sockets (unix.h): AF_UNIX stream sockets, after Linux's
 * net/unix/af_unix.c.
 *
 * A socket is a page of its own (struct unix_sock): its state, the peer it
 * is connected to, and a ring of the bytes sent to it, as a pipe's
 * (pipe.c): `nread` and `nwrite` count the bytes ever read and written, so
 * it holds nwrite - nread of them, from buf[nread % UNIX_BUF] on. A
 * connection is two sockets, each the other's peer: what one sends goes
 * into the other's ring. A listener holds a backlog of connections made
 * but not yet accepted: connect() makes the accepting end at once, as
 * Linux does, connected to the caller, and accept() takes it.
 *
 * Descriptors passed (SCM_RIGHTS) wait in the receiving socket's queue of
 * `rights`, each entry the files of one sendmsg() and the place of its
 * first byte in the ring: they are taken with that byte, and a read never
 * goes on past the bytes before them into theirs, so a reader always
 * learns which bytes they came with. A file waiting there holds a
 * reference; the socket's close drops them. A socket passed over its own
 * connection (or in a cycle of them) keeps itself open that way until the
 * machine stops: Linux has a garbage collector for those, Camelot doesn't.
 *
 * Locking: one spinlock, unix_lock, for every socket's state, rings and
 * queues, and the list of sockets connect() looks listeners up in. Each
 * socket is also the channel its waiters sleep on: readers of its ring and
 * writers into it, accept() and connect() on a listener; whatever changes
 * one wakes it. User memory is copied under the lock, as a pipe's is
 * (copyin and copyout never sleep). The disk (bind and connect's paths)
 * and closing files (the descriptors of a socket closed) happen without
 * it. Lock order: unix_lock, then proc_lock (sleep, wakeup, poll_wait).
 *
 * SO_PEERCRED, as Linux's: an accepted end's peer is the process that
 * called connect(), the connecting end's the one that called listen(), and
 * each of a pair's the process that made it; they stay what they were when
 * the connection was made, whoever holds the sockets since. */
#include "unix.h"
#include "file.h"
#include "fs.h"
#include "kalloc.h"
#include "log.h"
#include "msg.h"
#include "net.h"
#include "poll.h"
#include "proc.h"
#include "spinlock.h"
#include "string.h"
#include "trap.h"

/* A socket's ring: what may wait to be read. */
#define UNIX_BUF 2048

/* Sends with descriptors whose bytes wait in one ring: a sender with more
 * waits, as it does for room in the ring. */
#define UNIX_RIGHTS 8

/* Connections a listener holds before accept() takes them: listen()'s
 * backlog is taken as 1 to this. connect() waits for room. */
#define UNIX_BACKLOG 8

/* Linux's shutdown bits: no more reading, no more sending. */
#define RCV_SHUT 1
#define SEND_SHUT 2
#define SHUT_BOTH (RCV_SHUT | SEND_SHUT)

#define UNIX_UNCONNECTED 1 /* new, maybe bound */
#define UNIX_LISTENING 2   /* bound, taking connections */
#define UNIX_CONNECTED 3   /* connected; its peer may have gone since */

/* The files one sendmsg() passed, and where its first byte is. */
struct unix_rights {
    u32 at; /* that byte's count in `nwrite` */
    u32 n;
    struct file *files[SCM_MAX_FD];
};

struct unix_sock {
    struct unix_sock *next; /* the list of every socket */
    u32 state;
    u32 shut;  /* RCV_SHUT and SEND_SHUT: shutdown(), the peer's or its close */
    int err;   /* an error for the next read to return: -ECONNRESET */
    u32 rcvtimeo; /* SO_RCVTIMEO, in ticks: 0 waits for as long as it takes */
    i32 pid;      /* UNIX_LISTENING: the process that called listen() */
    i32 peerpid;  /* SO_PEERCRED: the peer's process when they connected (0: none) */
    struct unix_sock *peer; /* UNIX_CONNECTED: NULL once the peer is closed */
    struct inode *ip;       /* the socket inode bind() made, with a reference */
    char name[UNIX_PATH_MAX];     /* its path, or the listener's ("": none) */
    char peername[UNIX_PATH_MAX]; /* the peer's name, kept once it is gone */
    /* The ring and its rights, oldest first from rights[rhead]. */
    u32 nread;
    u32 nwrite;
    u32 rhead;
    u32 rcount;
    struct unix_rights rights[UNIX_RIGHTS];
    /* UNIX_LISTENING: the connections to accept, oldest first. */
    u32 max_backlog;
    u32 nbacklog;
    struct unix_sock *backlog[UNIX_BACKLOG];
    char buf[UNIX_BUF];
};

_Static_assert(sizeof(struct unix_sock) <= PGSIZE, "a socket fits a page");

static struct spinlock unix_lock;
static struct unix_sock *socks; /* every socket, under unix_lock */
/* Sockets closed, waiting for unix_close() to free them, linked by `next`,
 * and whether a thread is freeing them: under unix_lock. */
static struct unix_sock *dying;
static bool reaping;

void unix_init(void) {
    initlock(&unix_lock, "unix");
}

static pagetable_t user_pt(void) {
    return myproc()->pagetable;
}

static u32 min32(u32 a, u32 b) {
    return a < b ? a : b;
}

/* A new socket in `state`, on the list; NULL if no page is free. */
static struct unix_sock *us_new(u32 state) {
    struct unix_sock *u = kalloc();
    if (u == NULL) {
        return NULL;
    }
    memset(u, 0, sizeof *u);
    u->state = state;
    acquire(&unix_lock);
    u->next = socks;
    socks = u;
    release(&unix_lock);
    return u;
}

/* Makes `f` the file of socket `u`, readable and writable. */
static void unix_file(struct file *f, struct unix_sock *u) {
    f->type = FD_UNIX;
    f->readable = true;
    f->writable = true;
    f->usock = u;
}

/* The listener bound to socket inode `ip`, or NULL. Under unix_lock. */
static struct unix_sock *listener(const struct inode *ip) {
    for (struct unix_sock *u = socks; u != NULL; u = u->next) {
        if (u->state == UNIX_LISTENING && u->ip == ip) {
            return u;
        }
    }
    return NULL;
}

/* Sleeps on `chan`, letting go of unix_lock meanwhile, as sleep() does:
 * for at most `timeout` ticks from `start` if `timeout` is not 0 (then on
 * the tick counter too, through a poll table, poll.h). -EAGAIN if the time
 * is up, else 0: the caller checks again either way. */
static int unix_wait(const void *chan, u32 timeout, u32 start) {
    if (timeout == 0) {
        sleep(chan, &unix_lock);
        return 0;
    }
    if (ticks - start >= timeout) {
        return -EAGAIN;
    }
    /* Both channels named before the lock goes, so a wakeup that comes
     * after the caller's check finds them in the table. */
    struct poll_table table;
    poll_begin(&table);
    poll_wait(&table, chan);
    poll_wait(&table, (const void *)&ticks);
    release(&unix_lock);
    poll_sleep(&table);
    poll_end();
    acquire(&unix_lock);
    return 0;
}

/* Cuts `u` off its peer, as `u` is closed: the peer will receive and send
 * no more, and a read of it returns -ECONNRESET first if `reset` or if `u`
 * had bytes it never read (Linux's unix_release_sock). Under unix_lock. */
static void hang_up(struct unix_sock *u, bool reset) {
    struct unix_sock *p = u->peer;
    if (p == NULL) {
        return;
    }
    p->peer = NULL;
    p->shut = SHUT_BOTH;
    if (reset || u->nwrite != u->nread) {
        p->err = -ECONNRESET;
    }
    wakeup(p);
    u->peer = NULL;
}

/* Takes `u` off the list of sockets. Under unix_lock. */
static void unlist(struct unix_sock *u) {
    for (struct unix_sock **pp = &socks; *pp != NULL; pp = &(*pp)->next) {
        if (*pp == u) {
            *pp = u->next;
            return;
        }
    }
}

/* Frees `u`, off the list and cut off from everything: closes the files
 * waiting in it and lets go of its inode. Without unix_lock. */
static void us_free(struct unix_sock *u) {
    for (u32 i = 0; i < u->rcount; i++) {
        struct unix_rights *r = &u->rights[(u->rhead + i) % UNIX_RIGHTS];
        scm_drop(r->files, r->n);
    }
    if (u->ip != NULL) {
        begin_op();
        iput(u->ip);
        end_op();
    }
    kfree(u);
}

/* Closing a socket closes the files waiting in it, which may be sockets
 * holding files of their own, and so on: a process can make that chain as
 * long as there are open files. Freed from a loop, not by recursion, the
 * chain costs no stack: the sockets closed go on the `dying` list, and the
 * one thread freeing them (`reaping`) frees those its closes add too.
 * Another thread's closes meanwhile are left to it as well. */
void unix_close(struct unix_sock *u) {
    acquire(&unix_lock);
    /* The socket and, for a listener, the connections it never accepted:
     * their clients see them reset, as on Linux. */
    for (u32 i = 0; i <= u->nbacklog; i++) {
        struct unix_sock *d = i == 0 ? u : u->backlog[i - 1];
        hang_up(d, i > 0);
        unlist(d);
        /* Its peer's writers wait for room in it; connect() for room in a
         * listener's backlog. */
        wakeup(d);
        d->next = dying;
        dying = d;
    }
    u->nbacklog = 0;
    u->state = UNIX_UNCONNECTED;
    if (reaping) {
        release(&unix_lock);
        return;
    }
    reaping = true;
    while (dying != NULL) {
        struct unix_sock *d = dying;
        dying = d->next;
        release(&unix_lock);
        us_free(d); /* may come back here, to add to `dying` */
        acquire(&unix_lock);
    }
    reaping = false;
    release(&unix_lock);
}

/* ---- moving bytes ---- */

/* Sends what `io` holds into `u`'s peer, with the `nfiles` files of
 * files[] (whose references pass to the receiver, or are dropped here if
 * nothing is sent), waiting for room unless `dontwait`. The bytes sent, or
 * -ENOTCONN, -EPIPE (no more sending: shutdown, or the peer is gone),
 * -EAGAIN, -EINTR, -EFAULT if none could be. */
static isize unix_send(struct unix_sock *u, struct uio *io, struct file **files, u32 nfiles, bool dontwait) {
    u32 n = io->resid;
    u32 done = 0;
    int err = 0;
    bool pending = nfiles > 0;
    acquire(&unix_lock);
    /* Checked first, as Linux does, so sending nothing fails the same. */
    if (u->state != UNIX_CONNECTED) {
        err = -ENOTCONN;
    } else if ((u->shut & SEND_SHUT) != 0) {
        err = -EPIPE;
    }
    while (err == 0 && done < n) {
        if (u->state != UNIX_CONNECTED) {
            err = -ENOTCONN;
            break;
        }
        struct unix_sock *p = u->peer;
        if ((u->shut & SEND_SHUT) != 0 || p == NULL || (p->shut & RCV_SHUT) != 0) {
            err = -EPIPE;
            break;
        }
        u32 room = UNIX_BUF - (p->nwrite - p->nread);
        if (room == 0 || (pending && p->rcount == UNIX_RIGHTS)) {
            if (dontwait) {
                err = -EAGAIN;
                break;
            }
            if (proc_killed()) {
                err = -EINTR;
                break;
            }
            sleep(p, &unix_lock);
            continue;
        }
        u32 m = min32(n - done, room);
        u32 at = p->nwrite % UNIX_BUF;
        u32 first = min32(m, UNIX_BUF - at);
        /* Copied before anything says it is there: a copy that fails (the
         * buffer unmapped by another thread since it was checked) leaves
         * the ring as it was. */
        if (uio_move(io, p->buf + at, first, false) < 0 ||
            (m > first && uio_move(io, p->buf, m - first, false) < 0)) {
            err = -EFAULT;
            break;
        }
        if (pending) {
            struct unix_rights *r = &p->rights[(p->rhead + p->rcount) % UNIX_RIGHTS];
            r->at = p->nwrite;
            r->n = nfiles;
            memcpy(r->files, files, nfiles * sizeof *files);
            p->rcount++;
            pending = false;
        }
        p->nwrite += m;
        done += m;
        wakeup(p);
    }
    release(&unix_lock);
    if (pending) {
        scm_drop(files, nfiles);
    }
    return done > 0 ? (isize)done : err;
}

/* Receives into `io` what waits in `u`'s ring, waiting for some unless
 * `dontwait`, up to the next bytes that came with descriptors; if the
 * first byte did, its files go to files[] (*nfiles of them, the
 * references the caller's). The bytes received, 0 at the end (the peer
 * sends no more, or `io` is empty), or -EINVAL (not connected), the
 * error the peer's close left (-ECONNRESET, once), -EAGAIN (`dontwait`,
 * or SO_RCVTIMEO ran out), -EINTR, -EFAULT. */
static isize unix_recv(struct unix_sock *u, struct uio *io, struct file **files, u32 *nfiles, bool dontwait) {
    u32 n = io->resid;
    *nfiles = 0;
    isize r;
    u32 start = ticks;
    acquire(&unix_lock);
    for (;;) {
        if (u->state != UNIX_CONNECTED) {
            r = -EINVAL;
            break;
        }
        u32 avail = u->nwrite - u->nread;
        if (avail > 0 && n > 0) {
            u32 limit = avail;
            bool took = false;
            if (u->rcount > 0 && u->rights[u->rhead].at == u->nread) {
                struct unix_rights *e = &u->rights[u->rhead];
                memcpy(files, e->files, e->n * sizeof *files);
                *nfiles = e->n;
                u->rhead = (u->rhead + 1) % UNIX_RIGHTS;
                u->rcount--;
                took = true;
            }
            /* Never into the bytes of the next send with descriptors. */
            if (u->rcount > 0) {
                limit = min32(limit, u->rights[u->rhead].at - u->nread);
            }
            u32 m = min32(n, limit);
            u32 at = u->nread % UNIX_BUF;
            u32 first = min32(m, UNIX_BUF - at);
            if (uio_move(io, u->buf + at, first, true) < 0 ||
                (m > first && uio_move(io, u->buf, m - first, true) < 0)) {
                /* Nothing taken: the descriptors wait again. */
                if (took) {
                    u->rhead = (u->rhead + UNIX_RIGHTS - 1) % UNIX_RIGHTS;
                    u->rcount++;
                    *nfiles = 0;
                }
                r = -EFAULT;
                break;
            }
            u->nread += m;
            r = m;
            /* Room for the peer's writers. */
            wakeup(u);
            break;
        }
        if (u->err != 0) {
            r = u->err;
            u->err = 0;
            break;
        }
        if ((u->shut & RCV_SHUT) != 0 || n == 0) {
            r = 0;
            break;
        }
        if (dontwait) {
            r = -EAGAIN;
            break;
        }
        if (proc_killed()) {
            r = -EINTR;
            break;
        }
        r = unix_wait(u, u->rcvtimeo, start);
        if (r < 0) {
            break;
        }
    }
    release(&unix_lock);
    return r;
}

isize unix_read(struct unix_sock *u, pagetable_t pt, uintptr dst, usize n) {
    struct uio io;
    uio_single(&io, pt, dst, n);
    struct file *files[SCM_MAX_FD];
    u32 nfiles;
    isize r = unix_recv(u, &io, files, &nfiles, false);
    /* read() has nowhere to put descriptors: they are closed, as a
     * recvmsg() without room for them closes them. */
    scm_drop(files, nfiles);
    return r;
}

isize unix_write(struct unix_sock *u, pagetable_t pt, uintptr src, usize n) {
    struct uio io;
    uio_single(&io, pt, src, n);
    return unix_send(u, &io, NULL, 0, false);
}

u32 unix_poll(struct unix_sock *u, struct poll_table *pt) {
    acquire(&unix_lock);
    /* Its own channel (bytes come, the peer goes, a connection waits to
     * be accepted) and its peer's (room comes in the peer's ring). */
    poll_wait(pt, u);
    if (u->peer != NULL) {
        poll_wait(pt, u->peer);
    }
    u32 mask = 0;
    if (u->state == UNIX_LISTENING) {
        if (u->nbacklog > 0) {
            mask |= POLL_READY_IN;
        }
        release(&unix_lock);
        return mask;
    }
    if (u->nwrite != u->nread || u->err != 0) {
        mask |= POLL_READY_IN;
    }
    if (u->err != 0) {
        mask |= POLLERR;
    }
    if ((u->shut & RCV_SHUT) != 0) {
        mask |= POLL_READY_IN | POLLRDHUP;
    }
    /* A stream never connected is hung up, as on Linux (unix_poll). */
    if (u->shut == SHUT_BOTH || u->state != UNIX_CONNECTED) {
        mask |= POLLHUP;
    }
    /* A write wouldn't wait: there is room in the peer's ring, or it
     * would fail at once. */
    struct unix_sock *p = u->peer;
    if (u->state != UNIX_CONNECTED || p == NULL || (u->shut & SEND_SHUT) != 0 || (p->shut & RCV_SHUT) != 0 ||
        p->nwrite - p->nread < UNIX_BUF) {
        mask |= POLL_READY_OUT;
    }
    release(&unix_lock);
    return mask;
}

void unix_stat(struct unix_sock *u, struct stat *st) {
    st->type = T_SOCKET;
    acquire(&unix_lock);
    st->size = u->nwrite - u->nread;
    st->nlink = (u->shut & RCV_SHUT) != 0 ? 0 : 1;
    release(&unix_lock);
}

/* ---- the system calls ---- */

/* The socket behind descriptor `fd`, with the open file in *fp (NULL if
 * none), whose reference the caller lets go of with put_file(); NULL with
 * *err -EBADF or -ENOTSOCK. */
static struct unix_sock *fd_unix(u32 fd, struct file **fp, int *err) {
    struct file *f = fd_get(fd);
    *fp = f;
    if (f == NULL) {
        *err = -EBADF;
        return NULL;
    }
    if (f->type != FD_UNIX) {
        *err = -ENOTSOCK;
        return NULL;
    }
    return f->usock;
}

static void put_file(struct file *f) {
    if (f != NULL) {
        fileclose(f);
    }
}

bool unix_fd(u32 fd) {
    struct file *f = fd_get(fd);
    if (f == NULL) {
        return false;
    }
    bool local = f->type == FD_UNIX;
    fileclose(f);
    return local;
}

/* Reads the struct sockaddr_un at user address `uaddr`: its path into
 * `path`. -EFAULT, or -EINVAL: not AF_UNIX, or not a path (empty, or with
 * no NUL in sun_path: Linux's abstract names and unnamed addresses are
 * not Camelot's). */
static int get_path(uintptr uaddr, char *path) {
    struct sockaddr_un a;
    if (copyin(user_pt(), &a, uaddr, sizeof a) < 0) {
        return -EFAULT;
    }
    if (a.sun_family != AF_UNIX) {
        return -EINVAL;
    }
    u32 len = 0;
    while (len < UNIX_PATH_MAX && a.sun_path[len] != 0) {
        len++;
    }
    if (len == 0 || len == UNIX_PATH_MAX) {
        return -EINVAL;
    }
    memcpy(path, a.sun_path, len + 1);
    return 0;
}

/* Stores `name` as a struct sockaddr_un at `uaddr`, which the caller has
 * checked: 0, or -EFAULT if another thread unmapped it since. */
static int put_name(uintptr uaddr, const char *name) {
    struct sockaddr_un a;
    memset(&a, 0, sizeof a);
    a.sun_family = AF_UNIX;
    memcpy(a.sun_path, name, UNIX_PATH_MAX);
    return copyout(user_pt(), uaddr, &a, sizeof a) == 0 ? 0 : -EFAULT;
}

static bool name_writable(uintptr uaddr) {
    return uvm_range_ok(user_pt(), uaddr, sizeof(struct sockaddr_un), PTE_W);
}

int unix_sys_socket(u32 type, u32 protocol) {
    /* Linux's order: the protocol (0, or PF_UNIX), then the type. */
    if (protocol != 0 && protocol != AF_UNIX) {
        return -EPROTONOSUPPORT;
    }
    if (type != SOCK_STREAM) {
        return -ESOCKTNOSUPPORT;
    }
    if (!fd_available()) {
        return -EMFILE;
    }
    struct file *f = filealloc();
    if (f == NULL) {
        return -ENFILE;
    }
    struct unix_sock *u = us_new(UNIX_UNCONNECTED);
    if (u == NULL) {
        fileclose(f); /* nothing set yet: just gives the slot back */
        return -ENOMEM;
    }
    unix_file(f, u);
    int fd = fd_install(f);
    if (fd < 0) {
        fileclose(f); /* closes the socket */
    }
    return fd;
}

int unix_sys_socketpair(u32 domain, u32 type, u32 protocol, uintptr ufds) {
    if (domain == AF_INET) {
        return -EOPNOTSUPP; /* TCP and UDP have no pairs, as on Linux */
    }
    if (domain != AF_UNIX) {
        return -EAFNOSUPPORT;
    }
    if (protocol != 0 && protocol != AF_UNIX) {
        return -EPROTONOSUPPORT;
    }
    if (type != SOCK_STREAM) {
        return -ESOCKTNOSUPPORT;
    }
    pagetable_t pt = user_pt();
    int fds[2];
    if (!uvm_range_ok(pt, ufds, sizeof fds, PTE_W)) {
        return -EFAULT;
    }
    struct file *fa = filealloc();
    struct file *fb = fa != NULL ? filealloc() : NULL;
    if (fb == NULL) {
        put_file(fa);
        return -ENFILE;
    }
    struct unix_sock *a = us_new(UNIX_CONNECTED);
    struct unix_sock *b = a != NULL ? us_new(UNIX_CONNECTED) : NULL;
    if (b == NULL) {
        if (a != NULL) {
            unix_close(a);
        }
        fileclose(fa);
        fileclose(fb);
        return -ENOMEM;
    }
    acquire(&unix_lock);
    a->peer = b;
    b->peer = a;
    a->peerpid = (i32)myproc()->pid;
    b->peerpid = a->peerpid;
    release(&unix_lock);
    unix_file(fa, a);
    unix_file(fb, b);
    if (fd_install2(fa, fb, fds) < 0) {
        fileclose(fa);
        fileclose(fb);
        return -EMFILE;
    }
    /* As pipe(): checked above, but another thread may have unmapped it
     * since, and then the descriptors are taken back. */
    if (copyout(pt, ufds, fds, sizeof fds) != 0) {
        if (fd_take_if((u32)fds[0], fa)) {
            fileclose(fa);
        }
        if (fd_take_if((u32)fds[1], fb)) {
            fileclose(fb);
        }
        return -EFAULT;
    }
    return 0;
}

static int do_bind(struct unix_sock *u, uintptr uaddr) {
    char path[UNIX_PATH_MAX];
    int err = get_path(uaddr, path);
    if (err < 0) {
        return err;
    }
    acquire(&unix_lock);
    bool bound = u->ip != NULL;
    release(&unix_lock);
    if (bound) {
        return -EINVAL;
    }
    if (!fs_mounted) {
        return -ENOENT;
    }
    /* The socket inode: a name for connect() to find, with nothing in it.
     * One that is there already, a socket's or anything else, is taken:
     * the name stays after the socket is closed, until it is unlinked. */
    begin_op();
    struct inode *ip = fs_create(path, T_SOCKET, 0, 0, &err);
    if (ip == NULL) {
        end_op();
        return err == -EEXIST ? -EADDRINUSE : err;
    }
    iunlock(ip);
    end_op();
    acquire(&unix_lock);
    bound = u->ip != NULL;
    if (!bound) {
        u->ip = ip;
        memcpy(u->name, path, UNIX_PATH_MAX);
    }
    release(&unix_lock);
    if (bound) {
        /* Another thread bound it meanwhile: this name stays on the disk,
         * as one a closed socket left. */
        begin_op();
        iput(ip);
        end_op();
        return -EINVAL;
    }
    return 0;
}

static int do_listen(struct unix_sock *u, u32 backlog) {
    int err = 0;
    acquire(&unix_lock);
    if (u->state == UNIX_CONNECTED || u->ip == NULL) {
        err = -EINVAL; /* as on Linux: a listener must have a name */
    } else {
        u->state = UNIX_LISTENING;
        u->pid = (i32)myproc()->pid;
        u->max_backlog = backlog < 1 ? 1 : backlog > UNIX_BACKLOG ? UNIX_BACKLOG : backlog;
        /* A bigger backlog lets waiting connect()s in. */
        wakeup(u);
    }
    release(&unix_lock);
    return err;
}

static int do_accept(struct unix_sock *u, uintptr uaddr) {
    if (uaddr != 0 && !name_writable(uaddr)) {
        return -EFAULT;
    }
    acquire(&unix_lock);
    bool listening = u->state == UNIX_LISTENING;
    release(&unix_lock);
    if (!listening) {
        return -EINVAL;
    }
    /* The descriptor and the open file first, so a connection is never
     * taken off the backlog only to be dropped for want of them. */
    if (!fd_available()) {
        return -EMFILE;
    }
    struct file *f = filealloc();
    if (f == NULL) {
        return -ENFILE;
    }
    struct unix_sock *c = NULL;
    char peer[UNIX_PATH_MAX];
    int err = 0;
    u32 start = ticks;
    acquire(&unix_lock);
    for (;;) {
        if (u->nbacklog > 0) {
            c = u->backlog[0];
            u->nbacklog--;
            memmove(u->backlog, u->backlog + 1, u->nbacklog * sizeof u->backlog[0]);
            memcpy(peer, c->peername, UNIX_PATH_MAX);
            /* Room for a connect() that waits. */
            wakeup(u);
            break;
        }
        if (proc_killed()) {
            err = -EINTR;
            break;
        }
        err = unix_wait(u, u->rcvtimeo, start);
        if (err < 0) {
            break;
        }
    }
    release(&unix_lock);
    if (c == NULL) {
        fileclose(f); /* nothing set yet: just gives the slot back */
        return err;
    }
    unix_file(f, c);
    int fd = fd_install(f);
    if (fd < 0) {
        fileclose(f); /* closes the connection: its client sees an end */
        return fd;
    }
    if (uaddr != 0) {
        (void)put_name(uaddr, peer);
    }
    return fd;
}

static int do_connect(struct unix_sock *u, uintptr uaddr) {
    char path[UNIX_PATH_MAX];
    int err = get_path(uaddr, path);
    if (err < 0) {
        return err;
    }
    acquire(&unix_lock);
    u32 state = u->state;
    release(&unix_lock);
    if (state != UNIX_UNCONNECTED) {
        return state == UNIX_CONNECTED ? -EISCONN : -EINVAL;
    }
    if (!fs_mounted) {
        return -ENOENT;
    }
    begin_op();
    struct inode *ip = namei(path, &err);
    if (ip == NULL) {
        end_op();
        return err;
    }
    ilock(ip);
    bool socket_inode = ip->type == T_SOCKET;
    iunlock(ip);
    end_op();
    /* The accepting end, made now: the listener's backlog holds it. */
    struct unix_sock *s = NULL;
    if (!socket_inode) {
        err = -ECONNREFUSED; /* as on Linux, for a file that isn't a socket */
    } else if ((s = us_new(UNIX_CONNECTED)) == NULL) {
        err = -ENOMEM;
    }
    if (s != NULL) {
        u32 start = ticks;
        acquire(&unix_lock);
        for (;;) {
            if (u->state != UNIX_UNCONNECTED) {
                err = u->state == UNIX_CONNECTED ? -EISCONN : -EINVAL;
                break;
            }
            /* Looked up again after each wait: the listener may be gone. */
            struct unix_sock *l = listener(ip);
            if (l == NULL) {
                err = -ECONNREFUSED; /* a name nobody listens on */
                break;
            }
            if (l->nbacklog < l->max_backlog) {
                s->peer = u;
                u->peer = s;
                u->state = UNIX_CONNECTED;
                memcpy(s->name, l->name, UNIX_PATH_MAX);
                memcpy(s->peername, u->name, UNIX_PATH_MAX);
                memcpy(u->peername, l->name, UNIX_PATH_MAX);
                s->peerpid = (i32)myproc()->pid;
                u->peerpid = l->pid;
                l->backlog[l->nbacklog++] = s;
                s = NULL;
                wakeup(l);
                wakeup(u);
                err = 0;
                break;
            }
            if (proc_killed()) {
                err = -EINTR;
                break;
            }
            /* connect() has no timeout: SO_RCVTIMEO is recv's and
             * accept's. */
            (void)unix_wait(l, 0, start);
        }
        release(&unix_lock);
        if (s != NULL) {
            unix_close(s); /* never connected: just freed */
        }
    }
    begin_op();
    iput(ip);
    end_op();
    return err;
}

static int do_shutdown(struct unix_sock *u, u32 how) {
    if (how > SHUT_RDWR) {
        return -EINVAL;
    }
    /* SHUT_RD stops this end's reading and the peer's sending; SHUT_WR
     * the other way round. Any socket, connected or not, as on Linux. */
    u32 mine = how == SHUT_RD ? RCV_SHUT : how == SHUT_WR ? SEND_SHUT : SHUT_BOTH;
    u32 theirs = ((mine & RCV_SHUT) != 0 ? SEND_SHUT : 0) | ((mine & SEND_SHUT) != 0 ? RCV_SHUT : 0);
    acquire(&unix_lock);
    u->shut |= mine;
    wakeup(u);
    if (u->peer != NULL) {
        u->peer->shut |= theirs;
        wakeup(u->peer);
    }
    release(&unix_lock);
    return 0;
}

static int do_name(struct unix_sock *u, uintptr uaddr, bool peer) {
    if (!name_writable(uaddr)) {
        return -EFAULT;
    }
    char name[UNIX_PATH_MAX];
    int err = 0;
    acquire(&unix_lock);
    if (peer && u->state != UNIX_CONNECTED) {
        err = -ENOTCONN;
    } else {
        memcpy(name, peer ? u->peername : u->name, UNIX_PATH_MAX);
    }
    release(&unix_lock);
    return err < 0 ? err : put_name(uaddr, name);
}

/* Whether `u` is connected: sendto()'s and sendmsg()'s error for an
 * address, -EISCONN, else -EOPNOTSUPP (Linux's, for a stream). */
static int address_given(struct unix_sock *u) {
    acquire(&unix_lock);
    bool connected = u->state == UNIX_CONNECTED;
    release(&unix_lock);
    return connected ? -EISCONN : -EOPNOTSUPP;
}

static isize do_sendto(struct unix_sock *u, uintptr ubuf, usize len, uintptr uaddr) {
    pagetable_t pt = user_pt();
    if (!uvm_range_ok(pt, ubuf, len, PTE_R)) {
        return -EFAULT;
    }
    if (uaddr != 0) {
        if (!uvm_range_ok(pt, uaddr, sizeof(struct sockaddr_un), PTE_R)) {
            return -EFAULT;
        }
        return address_given(u);
    }
    struct uio io;
    uio_single(&io, pt, ubuf, len);
    return unix_send(u, &io, NULL, 0, false);
}

static isize do_recvfrom(struct unix_sock *u, uintptr ubuf, usize len, uintptr uaddr) {
    pagetable_t pt = user_pt();
    if (!uvm_range_ok(pt, ubuf, len, PTE_W) || (uaddr != 0 && !name_writable(uaddr))) {
        return -EFAULT;
    }
    isize r = unix_read(u, pt, ubuf, len);
    if (r >= 0 && uaddr != 0) {
        char name[UNIX_PATH_MAX];
        acquire(&unix_lock);
        memcpy(name, u->peername, UNIX_PATH_MAX);
        release(&unix_lock);
        (void)put_name(uaddr, name);
    }
    return r;
}

static isize do_sendmsg(struct unix_sock *u, uintptr umsg, u32 flags) {
    pagetable_t pt = user_pt();
    struct msghdr m;
    if (msg_get(pt, umsg, &m) < 0) {
        return -EFAULT;
    }
    struct uio io;
    int err = uio_iovec(&io, pt, m.msg_iov, m.msg_iovlen, false);
    if (err < 0) {
        return err;
    }
    if (m.msg_name != 0 && m.msg_namelen != 0) {
        return address_given(u);
    }
    if ((flags & ~(u32)(MSG_DONTWAIT | MSG_NOSIGNAL)) != 0) {
        return -EOPNOTSUPP;
    }
    struct file *files[SCM_MAX_FD];
    u32 nfiles;
    err = scm_get(pt, &m, true, files, &nfiles);
    if (err < 0) {
        return err;
    }
    /* Descriptors come with a byte, the first of what the call sends: with
     * none, there is nothing for them to come with. */
    if (nfiles > 0 && io.resid == 0) {
        scm_drop(files, nfiles);
        return -EINVAL;
    }
    return unix_send(u, &io, files, nfiles, (flags & MSG_DONTWAIT) != 0);
}

static isize do_recvmsg(struct unix_sock *u, uintptr umsg, u32 flags) {
    pagetable_t pt = user_pt();
    struct msghdr m;
    if (msg_get(pt, umsg, &m) < 0 || msg_check_out(pt, umsg, &m, sizeof(struct sockaddr_un)) < 0) {
        return -EFAULT;
    }
    struct uio io;
    int err = uio_iovec(&io, pt, m.msg_iov, m.msg_iovlen, true);
    if (err < 0) {
        return err;
    }
    if ((flags & ~(u32)(MSG_DONTWAIT | MSG_CMSG_CLOEXEC)) != 0) {
        return -EOPNOTSUPP;
    }
    struct file *files[SCM_MAX_FD];
    u32 nfiles;
    isize r = unix_recv(u, &io, files, &nfiles, (flags & MSG_DONTWAIT) != 0);
    if (r < 0) {
        return r;
    }
    u32 oflags = 0;
    usize controllen = scm_install(pt, &m, files, nfiles, (flags & MSG_CMSG_CLOEXEC) != 0, &oflags);
    /* The sender's name, if it has one: a connection's peer's (Linux
     * stores nothing, and a length of 0, for an unnamed one). */
    u32 namelen = 0;
    if (m.msg_name != 0) {
        char name[UNIX_PATH_MAX];
        acquire(&unix_lock);
        memcpy(name, u->peername, UNIX_PATH_MAX);
        release(&unix_lock);
        if (name[0] != 0) {
            struct sockaddr_un a;
            memset(&a, 0, sizeof a);
            a.sun_family = AF_UNIX;
            memcpy(a.sun_path, name, UNIX_PATH_MAX);
            namelen = (u32)(offsetof(struct sockaddr_un, sun_path) + strlen(name) + 1);
            (void)copyout(pt, m.msg_name, &a, min32(namelen, m.msg_namelen));
        }
    }
    (void)msg_put(pt, umsg, namelen, controllen, oflags);
    return r;
}

/* The calls by descriptor: each takes the open file for as long as it
 * runs, so the socket stays even if another thread closes `fd`. */

int unix_sys_bind(u32 fd, uintptr uaddr) {
    struct file *f;
    int err;
    struct unix_sock *u = fd_unix(fd, &f, &err);
    if (u != NULL) {
        err = do_bind(u, uaddr);
    }
    put_file(f);
    return err;
}

int unix_sys_listen(u32 fd, u32 backlog) {
    struct file *f;
    int err;
    struct unix_sock *u = fd_unix(fd, &f, &err);
    if (u != NULL) {
        err = do_listen(u, backlog);
    }
    put_file(f);
    return err;
}

int unix_sys_accept(u32 fd, uintptr uaddr) {
    struct file *f;
    int err;
    struct unix_sock *u = fd_unix(fd, &f, &err);
    if (u != NULL) {
        err = do_accept(u, uaddr);
    }
    put_file(f);
    return err;
}

int unix_sys_connect(u32 fd, uintptr uaddr) {
    struct file *f;
    int err;
    struct unix_sock *u = fd_unix(fd, &f, &err);
    if (u != NULL) {
        err = do_connect(u, uaddr);
    }
    put_file(f);
    return err;
}

isize unix_sys_sendto(u32 fd, uintptr ubuf, usize len, uintptr uaddr) {
    struct file *f;
    int err;
    struct unix_sock *u = fd_unix(fd, &f, &err);
    isize r = u != NULL ? do_sendto(u, ubuf, len, uaddr) : err;
    put_file(f);
    return r;
}

isize unix_sys_recvfrom(u32 fd, uintptr ubuf, usize len, uintptr uaddr) {
    struct file *f;
    int err;
    struct unix_sock *u = fd_unix(fd, &f, &err);
    isize r = u != NULL ? do_recvfrom(u, ubuf, len, uaddr) : err;
    put_file(f);
    return r;
}

int unix_sys_shutdown(u32 fd, u32 how) {
    struct file *f;
    int err;
    struct unix_sock *u = fd_unix(fd, &f, &err);
    if (u != NULL) {
        err = do_shutdown(u, how);
    }
    put_file(f);
    return err;
}

int unix_sys_name(u32 fd, uintptr uaddr, bool peer) {
    struct file *f;
    int err;
    struct unix_sock *u = fd_unix(fd, &f, &err);
    if (u != NULL) {
        err = do_name(u, uaddr, peer);
    }
    put_file(f);
    return err;
}

int unix_sys_setsockopt(u32 fd, u32 level, u32 option, uintptr uvalue, u32 len) {
    struct file *f;
    int err;
    struct unix_sock *u = fd_unix(fd, &f, &err);
    if (u != NULL) {
        u32 timeo;
        err = sock_opt_rcvtimeo(level, option, uvalue, len, &timeo);
        if (err == 0) {
            acquire(&unix_lock);
            u->rcvtimeo = timeo;
            release(&unix_lock);
        }
    }
    put_file(f);
    return err;
}

int unix_sys_getsockopt(u32 fd, u32 level, u32 option, uintptr uvalue, uintptr ulen) {
    struct file *f;
    int err;
    struct unix_sock *u = fd_unix(fd, &f, &err);
    u32 len;
    if (u != NULL && (err = sock_opt_len(ulen, &len)) == 0) {
        if (level != SOL_SOCKET || option != SO_PEERCRED) {
            err = -ENOPROTOOPT;
        } else {
            /* No users: uid and gid 0. */
            struct ucred cred = {0, 0, 0};
            acquire(&unix_lock);
            cred.pid = u->peerpid;
            release(&unix_lock);
            err = sock_opt_put(uvalue, ulen, len, &cred, sizeof cred);
        }
    }
    put_file(f);
    return err;
}

isize unix_sys_sendmsg(u32 fd, uintptr umsg, u32 flags) {
    struct file *f;
    int err;
    struct unix_sock *u = fd_unix(fd, &f, &err);
    isize r = u != NULL ? do_sendmsg(u, umsg, flags) : err;
    put_file(f);
    return r;
}

isize unix_sys_recvmsg(u32 fd, uintptr umsg, u32 flags) {
    struct file *f;
    int err;
    struct unix_sock *u = fd_unix(fd, &f, &err);
    isize r = u != NULL ? do_recvmsg(u, umsg, flags) : err;
    put_file(f);
    return r;
}
