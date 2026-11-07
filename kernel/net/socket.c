/* Sockets (docs/camelot-syscalls.md, "Sockets"; docs/camelot-net.md,
 * "Sockets"): the system calls that put the kernel-internal TCP and UDP
 * APIs (tcp.c, udp.c) behind file descriptors.
 *
 * A socket is an open file of type FD_SOCKET (file.h) whose `sock` points
 * at a struct socket of the table here, which holds the TCP socket or the
 * UDP one. So everything a descriptor does works on a socket too: read()
 * and write() are recv() and send() (file.c calls socket_read() and
 * socket_write()), dup() and spawn() share it, and the last close() closes
 * it (socket_close()). The calls here are what only a socket has.
 *
 * Addresses come and go as Linux's struct sockaddr_in (syscall.h): the
 * port and the address big-endian, as on the wire; the stack below works
 * in host order (net.h), so get_addr() and put_addr() convert.
 *
 * The checks come in the document's order: the descriptor (-EBADF, then
 * -ENOTSOCK for one that is not a socket), then the pointers (-EFAULT),
 * then everything else. A call that waits (accept, connect, send and recv
 * on a full or empty buffer) sleeps in net_sleep() and gives up with
 * -EINTR as soon as the process is killed.
 *
 * Another thread of the process may close the descriptor while a call
 * sleeps: fd_sock() takes a reference to the open file, so that the socket
 * stays, and leaves it in the thread's `net_file` for syscall() to let go
 * of once net_lock is free (the last close takes net_lock). */
#include "file.h"
#include "fs.h"
#include "kalloc.h"
#include "msg.h"
#include "net.h"
#include "poll.h"
#include "proc.h"
#include "string.h"
#include "timer.h"
#include "trap.h"
#include "vm.h"

/* One per open file at most, so a socket slot is never what runs out. */
static struct socket socks[NFILE];

static pagetable_t user_pt(void) {
    return myproc()->pagetable;
}

static struct socket *sock_alloc(u32 type) {
    for (u32 i = 0; i < NFILE; i++) {
        if (socks[i].type == 0) {
            memset(&socks[i], 0, sizeof socks[i]);
            socks[i].type = type;
            return &socks[i];
        }
    }
    panic("sock_alloc: no free socket");
    return NULL; /* not reached */
}

/* The socket behind descriptor `fd` of the current process; NULL with *err
 * -EBADF (not an open descriptor) or -ENOTSOCK (not a socket). */
static struct socket *fd_sock(u32 fd, int *err) {
    struct file *f = fd_get(fd);
    if (f == NULL) {
        *err = -EBADF;
        return NULL;
    }
    /* One per call: syscall() closes it. */
    struct thread *t = mythread();
    if (t->net_file != NULL) {
        panic("fd_sock: twice in one call");
    }
    t->net_file = f;
    if (f->type != FD_SOCKET) {
        *err = -ENOTSOCK;
        return NULL;
    }
    return f->sock;
}

/* Gives socket file `f` (all set up, with socket `s`) a descriptor: the
 * descriptor, or -EMFILE if another thread took the last free one since
 * the caller checked (fd_available()); then the socket is closed again,
 * here, since net_lock is held, and the file goes. */
static int sock_install(struct file *f, struct socket *s) {
    int fd = fd_install(f);
    if (fd < 0) {
        if (s->type == SOCK_STREAM) {
            tcp_close(s->tcp);
        } else if (s->udp != NULL) {
            udp_close(s->udp);
        }
        s->type = 0;
        f->type = FD_NONE; /* so fileclose() just gives the slot back */
        fileclose(f);
    }
    return fd;
}

/* Makes `f` the socket file of `s`, readable and writable. */
static void sock_file(struct file *f, struct socket *s) {
    f->type = FD_SOCKET;
    f->readable = true;
    f->writable = true;
    f->sock = s;
}

/* ---- addresses ---- */

/* Reads the struct sockaddr_in at user address `uaddr` into a host-order
 * address and port. -EFAULT, or -EAFNOSUPPORT for a family other than
 * AF_INET. */
static int get_addr(uintptr uaddr, u32 *addr, u32 *port) {
    struct sockaddr_in a;
    if (copyin(user_pt(), &a, uaddr, sizeof a) < 0) {
        return -EFAULT;
    }
    if (a.sin_family != AF_INET) {
        return -EAFNOSUPPORT;
    }
    *addr = get32((const u8 *)&a.sin_addr);
    *port = get16((const u8 *)&a.sin_port);
    return 0;
}

/* Stores `addr`:`port` (host order) as a struct sockaddr_in at `uaddr`,
 * which the caller has checked. 0, or -EFAULT if another thread of the
 * process has unmapped it since (sbrk). accept and recvfrom, which have
 * taken a connection or a datagram by then, still return it: the address
 * is lost, as it would be on a bad pointer the check missed. */
static int put_addr(uintptr uaddr, u32 addr, u32 port) {
    struct sockaddr_in a;
    memset(&a, 0, sizeof a);
    a.sin_family = AF_INET;
    put16((u8 *)&a.sin_port, port);
    put32((u8 *)&a.sin_addr, addr);
    return copyout(user_pt(), uaddr, &a, sizeof a) == 0 ? 0 : -EFAULT;
}

/* Whether `uaddr` can take a struct sockaddr_in (0, for the calls where it
 * is optional, can). */
static bool addr_writable(uintptr uaddr, bool optional) {
    if (optional && uaddr == 0) {
        return true;
    }
    return uvm_range_ok(user_pt(), uaddr, sizeof(struct sockaddr_in), PTE_W);
}

/* An address a socket of this host can be bound to: any (0), the
 * interface's, or one of 127.0.0.0/8. */
static bool bindable(u32 addr) {
    return addr == 0 || (netif.addr != 0 && addr == netif.addr) ||
           (addr & IP_LOOPBACK_MASK) == (IP_LOOPBACK & IP_LOOPBACK_MASK);
}

/* Gives the UDP socket `s` a port, the next ephemeral one, if it has none
 * yet (sendto, connect). */
static int udp_autobind(struct socket *s) {
    if (s->udp != NULL) {
        return 0;
    }
    int err;
    s->udp = udp_open(0, &err);
    return s->udp != NULL ? 0 : err;
}

/* ---- sending and receiving ---- */

/* The most one call sends or receives: INT_MAX bytes, as Linux's sendto
 * and recvfrom clamp `len` (and read and write their count, file.c), so
 * that the result fits an int and the stack's lengths stay 32-bit. */
#define MAX_LEN 0x7fffffffu

static u32 clamp_len(usize n) {
    return n > MAX_LEN ? MAX_LEN : (u32)n;
}

/* Sends `n` bytes at address `src` (in `pt`'s user memory, or kernel
 * memory when `pt` is NULL): on a stream, into the connection; as a
 * datagram, to `addr`:`port` if `to`, else to the peer the socket is
 * connected to. */
static int sock_send(struct socket *s, pagetable_t pt, uintptr src, u32 n, bool to, u32 addr, u32 port) {
    if (s->type == SOCK_STREAM) {
        /* An address is ignored: the connection says where (as on Linux). */
        return tcp_send(s->tcp, pt, src, n);
    }
    if (s->wr_shut) {
        return -EPIPE;
    }
    if (!to) {
        if (s->udp == NULL || s->udp->raddr == 0) {
            return -EDESTADDRREQ;
        }
        addr = s->udp->raddr;
        port = s->udp->rport;
    } else if (addr == 0 || port == 0) {
        return -EINVAL;
    }
    int err = udp_autobind(s);
    if (err < 0) {
        return err;
    }
    return udp_sendto(s->udp, addr, port, pt, src, n);
}

/* Receives up to `n` bytes into address `dst` (in `pt`'s user memory, or
 * kernel memory when `pt` is NULL); where they came from in *addr and
 * *port. */
static int sock_recv(struct socket *s, pagetable_t pt, uintptr dst, u32 n, u32 *addr, u32 *port) {
    if (s->rd_shut) {
        return 0;
    }
    if (s->type == SOCK_STREAM) {
        u32 laddr;
        u32 lport;
        tcp_ends(s->tcp, &laddr, &lport, addr, port);
        if (n == 0) {
            return 0; /* nothing to wait for */
        }
        return tcp_recv(s->tcp, pt, dst, n, s->rcvtimeo);
    }
    if (s->udp == NULL) {
        /* No port: nothing can ever come. */
        return -EINVAL;
    }
    int len = udp_recvfrom(s->udp, pt, dst, n, addr, port, s->rcvtimeo);
    if (len == -ETIMEDOUT) {
        return -EAGAIN; /* the receive timeout, as on Linux */
    }
    /* What did not fit is lost: the call says what it stored. */
    return len >= 0 && (u32)len > n ? (int)n : len;
}

/* The four calls file.c makes on a socket's file come in from outside the
 * stack, so each takes net_lock (net.h); the system calls below have it
 * already (syscall.c). */

isize socket_read(struct socket *s, pagetable_t pt, uintptr dst, usize n) {
    u32 addr;
    u32 port;
    acquire(&net_lock);
    int r = sock_recv(s, pt, dst, clamp_len(n), &addr, &port);
    release(&net_lock);
    return r;
}

isize socket_write(struct socket *s, pagetable_t pt, uintptr src, usize n) {
    acquire(&net_lock);
    int r = sock_send(s, pt, src, clamp_len(n), false, 0, 0);
    release(&net_lock);
    return r;
}

u32 socket_bytes(struct socket *s) {
    acquire(&net_lock);
    u32 n;
    if (s->type == SOCK_STREAM) {
        n = s->tcp->rlen;
    } else {
        n = s->udp != NULL ? udp_pending(s->udp) : 0;
    }
    release(&net_lock);
    return n;
}

bool socket_hung_up(struct socket *s) {
    acquire(&net_lock);
    bool ended = s->rd_shut;
    if (s->type == SOCK_STREAM) {
        ended = ended || s->tcp->fin_rcvd || s->tcp->err != 0;
    }
    release(&net_lock);
    return ended;
}

u32 socket_poll(struct socket *s, struct poll_table *pt) {
    acquire(&net_lock);
    net_poll_wait(pt);
    u32 mask;
    if (s->type == SOCK_STREAM) {
        mask = tcp_poll(s->tcp);
    } else {
        mask = POLL_READY_OUT;
        if (s->udp != NULL && udp_pending(s->udp) > 0) {
            mask |= POLL_READY_IN;
        }
    }
    if (s->rd_shut) {
        mask |= POLL_READY_IN | POLLRDHUP;
    }
    release(&net_lock);
    return mask;
}

void socket_close(struct socket *s) {
    acquire(&net_lock);
    if (s->type == SOCK_STREAM) {
        tcp_close(s->tcp);
    } else if (s->udp != NULL) {
        udp_close(s->udp);
    }
    s->type = 0;
    release(&net_lock);
}

/* ---- the system calls ---- */

int sock_sys_socket(u32 domain, u32 type, u32 protocol) {
    if (domain != AF_INET) {
        return -EAFNOSUPPORT;
    }
    if (type != SOCK_STREAM && type != SOCK_DGRAM) {
        return -EINVAL;
    }
    if (protocol != 0 && protocol != (type == SOCK_STREAM ? IP_PROTO_TCP : IP_PROTO_UDP)) {
        return -EPROTONOSUPPORT;
    }
    if (!netif.present) {
        return -ENODEV;
    }
    if (!fd_available()) {
        return -EMFILE;
    }
    struct file *f = filealloc();
    if (f == NULL) {
        return -ENFILE;
    }
    struct socket *s = sock_alloc(type);
    if (type == SOCK_STREAM) {
        int err;
        s->tcp = tcp_socket(&err);
        if (s->tcp == NULL) {
            s->type = 0;
            fileclose(f); /* nothing else set: just gives the slot back */
            return err;
        }
    }
    sock_file(f, s);
    return sock_install(f, s);
}

int sock_sys_bind(u32 fd, uintptr uaddr) {
    int err;
    struct socket *s = fd_sock(fd, &err);
    if (s == NULL) {
        return err;
    }
    u32 addr;
    u32 port;
    err = get_addr(uaddr, &addr, &port);
    if (err < 0) {
        return err;
    }
    /* The address is checked, but a socket gets its port on every address
     * of the host (docs/camelot-net.md, "Sockets"). */
    if (!bindable(addr)) {
        return -EADDRNOTAVAIL;
    }
    /* The ports below 1024 are root's, as Unix's privileged ports are
     * (Linux's inet_prot_sock and CAP_NET_BIND_SERVICE): a service on one
     * was started by the machine's administrator. */
    if (port != 0 && port < IPPORT_RESERVED && !capable()) {
        return -EACCES;
    }
    if (s->type == SOCK_STREAM) {
        return tcp_bind(s->tcp, port);
    }
    if (s->udp != NULL) {
        return -EINVAL; /* bound already */
    }
    s->udp = udp_open(port, &err);
    return s->udp != NULL ? 0 : err;
}

int sock_sys_listen(u32 fd, u32 backlog) {
    int err;
    struct socket *s = fd_sock(fd, &err);
    if (s == NULL) {
        return err;
    }
    if (s->type != SOCK_STREAM) {
        return -EOPNOTSUPP;
    }
    /* Not bound yet: the next ephemeral port, as on Linux. */
    if (s->tcp->lport == 0 && s->tcp->state == TCP_CLOSED && s->tcp->raddr == 0) {
        err = tcp_bind(s->tcp, 0);
        if (err < 0) {
            return err;
        }
    }
    return tcp_listen(s->tcp, backlog);
}

int sock_sys_accept(u32 fd, uintptr uaddr) {
    int err;
    struct socket *s = fd_sock(fd, &err);
    if (s == NULL) {
        return err;
    }
    if (!addr_writable(uaddr, true)) {
        return -EFAULT;
    }
    if (s->type != SOCK_STREAM) {
        return -EOPNOTSUPP;
    }
    if (s->tcp->state != TCP_LISTEN) {
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
    struct tcp_sock *c = tcp_accept(s->tcp, s->rcvtimeo, &err);
    if (c == NULL) {
        fileclose(f);
        return err;
    }
    struct socket *ns = sock_alloc(SOCK_STREAM);
    ns->tcp = c;
    sock_file(f, ns);
    int nfd = sock_install(f, ns);
    if (nfd < 0) {
        return nfd;
    }
    if (uaddr != 0) {
        u32 laddr;
        u32 lport;
        u32 raddr;
        u32 rport;
        tcp_ends(c, &laddr, &lport, &raddr, &rport);
        (void)put_addr(uaddr, raddr, rport);
    }
    return nfd;
}

int sock_sys_connect(u32 fd, uintptr uaddr) {
    int err;
    struct socket *s = fd_sock(fd, &err);
    if (s == NULL) {
        return err;
    }
    u32 addr;
    u32 port;
    err = get_addr(uaddr, &addr, &port);
    if (err < 0) {
        return err;
    }
    if (s->type == SOCK_STREAM) {
        return tcp_connect(s->tcp, addr, port);
    }
    if (addr == 0 || port == 0) {
        return -EINVAL;
    }
    err = udp_autobind(s);
    if (err < 0) {
        return err;
    }
    s->udp->raddr = addr;
    s->udp->rport = (u16)port;
    return 0;
}

isize sock_sys_send(u32 fd, uintptr ubuf, usize len) {
    return sock_sys_sendto(fd, ubuf, len, 0);
}

isize sock_sys_recv(u32 fd, uintptr ubuf, usize len) {
    return sock_sys_recvfrom(fd, ubuf, len, 0);
}

isize sock_sys_sendto(u32 fd, uintptr ubuf, usize len, uintptr uaddr) {
    int err;
    struct socket *s = fd_sock(fd, &err);
    if (s == NULL) {
        return err;
    }
    if (!uvm_range_ok(user_pt(), ubuf, len, PTE_R)) {
        return -EFAULT;
    }
    u32 addr = 0;
    u32 port = 0;
    if (uaddr != 0 && s->type == SOCK_STREAM) {
        /* Ignored, as the connection says where: only the pointer is
         * checked, as for every call. */
        if (!uvm_range_ok(user_pt(), uaddr, sizeof(struct sockaddr_in), PTE_R)) {
            return -EFAULT;
        }
    } else if (uaddr != 0) {
        err = get_addr(uaddr, &addr, &port);
        if (err < 0) {
            return err;
        }
    }
    return sock_send(s, user_pt(), ubuf, clamp_len(len), uaddr != 0, addr, port);
}

isize sock_sys_recvfrom(u32 fd, uintptr ubuf, usize len, uintptr uaddr) {
    int err;
    struct socket *s = fd_sock(fd, &err);
    if (s == NULL) {
        return err;
    }
    if (!uvm_range_ok(user_pt(), ubuf, len, PTE_W) || !addr_writable(uaddr, true)) {
        return -EFAULT;
    }
    u32 addr = 0;
    u32 port = 0;
    int n = sock_recv(s, user_pt(), ubuf, clamp_len(len), &addr, &port);
    if (n >= 0 && uaddr != 0) {
        (void)put_addr(uaddr, addr, port);
    }
    return n;
}

int sock_sys_shutdown(u32 fd, u32 how) {
    int err;
    struct socket *s = fd_sock(fd, &err);
    if (s == NULL) {
        return err;
    }
    if (how > SHUT_RDWR) {
        return -EINVAL;
    }
    if (s->type == SOCK_STREAM) {
        if (s->tcp->raddr == 0) {
            return -ENOTCONN; /* never connected, or a listener */
        }
        if (s->tcp->state == TCP_CLOSED) {
            /* Over: what ended it (refused, reset, timed out), whichever
             * side `how` names; -ENOTCONN once it closed normally. */
            return s->tcp->err != 0 ? s->tcp->err : -ENOTCONN;
        }
        if (how != SHUT_RD) {
            err = tcp_shutdown(s->tcp);
            if (err < 0) {
                return err;
            }
        }
    } else {
        if (s->udp == NULL || s->udp->raddr == 0) {
            return -ENOTCONN;
        }
        if (how != SHUT_RD) {
            s->wr_shut = true;
        }
    }
    if (how != SHUT_WR) {
        s->rd_shut = true;
    }
    /* A thread in poll() on it sees the shut side now, not at the next
     * tick. */
    net_wakeup();
    return 0;
}

int sock_sys_name(u32 fd, uintptr uaddr, bool peer) {
    int err;
    struct socket *s = fd_sock(fd, &err);
    if (s == NULL) {
        return err;
    }
    if (!addr_writable(uaddr, false)) {
        return -EFAULT;
    }
    u32 laddr = 0;
    u32 lport = 0;
    u32 raddr = 0;
    u32 rport = 0;
    if (s->type == SOCK_STREAM) {
        tcp_ends(s->tcp, &laddr, &lport, &raddr, &rport);
    } else if (s->udp != NULL) {
        lport = s->udp->port;
        raddr = s->udp->raddr;
        rport = s->udp->rport;
        /* A connected socket's datagrams leave from one address. */
        laddr = raddr != 0 ? ip_source(raddr) : 0;
    }
    if (peer) {
        if (raddr == 0) {
            return -ENOTCONN;
        }
        return put_addr(uaddr, raddr, rport);
    }
    return put_addr(uaddr, laddr, lport);
}

int sock_sys_setsockopt(u32 fd, u32 level, u32 option, uintptr uvalue, u32 len) {
    int err;
    struct socket *s = fd_sock(fd, &err);
    if (s == NULL) {
        return err;
    }
    return sock_opt_rcvtimeo(level, option, uvalue, len, &s->rcvtimeo);
}

int sock_sys_getsockopt(u32 fd, u32 level, u32 option, uintptr uvalue, uintptr ulen) {
    (void)level;
    (void)option;
    (void)uvalue;
    int err;
    struct socket *s = fd_sock(fd, &err);
    if (s == NULL) {
        return err;
    }
    u32 len;
    err = sock_opt_len(ulen, &len);
    if (err < 0) {
        return err;
    }
    /* SO_PEERCRED is a local socket's: TCP and UDP have no process at
     * their other end, and no other option is read back. */
    return -ENOPROTOOPT;
}

int sock_opt_len(uintptr ulen, u32 *len) {
    return copyin(user_pt(), len, ulen, sizeof *len) < 0 ? -EFAULT : 0;
}

int sock_opt_put(uintptr uvalue, uintptr ulen, u32 len, const void *value, u32 size) {
    pagetable_t pt = user_pt();
    u32 n = len < size ? len : size;
    if (copyout(pt, uvalue, value, n) != 0 || copyout(pt, ulen, &n, sizeof n) != 0) {
        return -EFAULT;
    }
    return 0;
}

/* ---- sendmsg and recvmsg ---- */

isize sock_sys_sendmsg(u32 fd, uintptr umsg, u32 flags) {
    int err;
    struct socket *s = fd_sock(fd, &err);
    if (s == NULL) {
        return err;
    }
    pagetable_t pt = user_pt();
    struct msghdr m;
    if (msg_get(pt, umsg, &m) < 0) {
        return -EFAULT;
    }
    struct uio io;
    err = uio_iovec(&io, pt, m.msg_iov, m.msg_iovlen, false);
    if (err < 0) {
        return err;
    }
    /* An address as sendto's: a stream's is only checked. */
    bool to = m.msg_name != 0 && m.msg_namelen != 0;
    u32 addr = 0;
    u32 port = 0;
    if (to && s->type == SOCK_STREAM) {
        if (!uvm_range_ok(pt, m.msg_name, m.msg_namelen, PTE_R)) {
            return -EFAULT;
        }
    } else if (to) {
        if (m.msg_namelen < sizeof(struct sockaddr_in)) {
            return -EINVAL;
        }
        err = get_addr(m.msg_name, &addr, &port);
        if (err < 0) {
            return err;
        }
    }
    /* MSG_DONTWAIT is local sockets' only. */
    if ((flags & ~(u32)MSG_NOSIGNAL) != 0) {
        return -EOPNOTSUPP;
    }
    struct file *files[SCM_MAX_FD];
    u32 nfiles;
    err = scm_get(pt, &m, false, files, &nfiles);
    if (err < 0) {
        return err;
    }
    /* Gathered a page at a time into the kernel's memory: a datagram whole
     * (no bigger than UDP_MAX_DATA in any case), a stream piece by piece. */
    u8 *page = kalloc();
    if (page == NULL) {
        return -ENOBUFS;
    }
    isize done = 0;
    if (s->type != SOCK_STREAM) {
        if (io.resid > PGSIZE) {
            err = -EMSGSIZE;
        } else {
            u32 n = io.resid;
            err = uio_move(&io, page, n, false);
            if (err == 0) {
                err = sock_send(s, NULL, (uintptr)page, n, to, addr, port);
            }
        }
        done = err;
    } else {
        do {
            u32 n = io.resid < PGSIZE ? io.resid : PGSIZE;
            int r = uio_move(&io, page, n, false);
            if (r == 0) {
                r = sock_send(s, NULL, (uintptr)page, n, false, 0, 0);
            }
            if (r < 0) {
                done = done > 0 ? done : r;
                break;
            }
            done += r;
            if ((u32)r < n) {
                break; /* killed part way */
            }
        } while (io.resid > 0);
    }
    kfree(page);
    return done;
}

isize sock_sys_recvmsg(u32 fd, uintptr umsg, u32 flags) {
    int err;
    struct socket *s = fd_sock(fd, &err);
    if (s == NULL) {
        return err;
    }
    pagetable_t pt = user_pt();
    struct msghdr m;
    if (msg_get(pt, umsg, &m) < 0 || msg_check_out(pt, umsg, &m, sizeof(struct sockaddr_in)) < 0) {
        return -EFAULT;
    }
    struct uio io;
    err = uio_iovec(&io, pt, m.msg_iov, m.msg_iovlen, true);
    if (err < 0) {
        return err;
    }
    if ((flags & ~(u32)MSG_CMSG_CLOEXEC) != 0) {
        return -EOPNOTSUPP;
    }
    u8 *page = kalloc();
    if (page == NULL) {
        return -ENOBUFS;
    }
    /* A datagram whole, to learn whether it fits; a stream's bytes as many
     * as the buffers take, a page at most. */
    u32 want = s->type == SOCK_STREAM ? (io.resid < PGSIZE ? io.resid : PGSIZE) : PGSIZE;
    u32 addr = 0;
    u32 port = 0;
    int n = sock_recv(s, NULL, (uintptr)page, want, &addr, &port);
    u32 oflags = 0;
    if (n >= 0) {
        u32 keep = (u32)n < io.resid ? (u32)n : io.resid;
        if (keep < (u32)n) {
            oflags |= MSG_TRUNC; /* the rest of the datagram is lost */
        }
        /* Checked (uio_iovec): another thread may have unmapped a buffer
         * since, and then what was received is lost. */
        if (uio_move(&io, page, keep, true) < 0) {
            n = -EFAULT;
        } else {
            n = (int)keep;
        }
    }
    kfree(page);
    if (n < 0) {
        return n;
    }
    /* A datagram's sender; a stream's none, as Linux's TCP stores. */
    u32 namelen = 0;
    if (s->type != SOCK_STREAM && m.msg_name != 0) {
        struct sockaddr_in a;
        memset(&a, 0, sizeof a);
        a.sin_family = AF_INET;
        put16((u8 *)&a.sin_port, port);
        put32((u8 *)&a.sin_addr, addr);
        namelen = sizeof a;
        (void)copyout(pt, m.msg_name, &a, m.msg_namelen < namelen ? m.msg_namelen : namelen);
    }
    (void)msg_put(pt, umsg, namelen, 0, oflags);
    return n;
}

int sock_opt_rcvtimeo(u32 level, u32 option, uintptr uvalue, u32 len, u32 *ticks) {
    struct timeval tv;
    if (len != sizeof tv) {
        return -EINVAL;
    }
    if (copyin(user_pt(), &tv, uvalue, sizeof tv) < 0) {
        return -EFAULT;
    }
    if (level != SOL_SOCKET || option != SO_RCVTIMEO) {
        return -ENOPROTOOPT;
    }
    if (tv.tv_sec < 0 || tv.tv_usec < 0 || tv.tv_usec >= 1000000) {
        return -EINVAL;
    }
    /* In ticks, rounded up, so a short timeout still waits: 0 is none. */
    u32 hz = TICK_HZ;
    u32 usec_per_tick = 1000000 / hz;
    u32 max_sec = 0xffffffffu / hz - 1;
    /* Compared as a time_t: a 64-bit tv_sec past 2^32 is long, not short. */
    u32 sec = tv.tv_sec > (time_t)max_sec ? max_sec : (u32)tv.tv_sec;
    *ticks = sec * hz + ((u32)tv.tv_usec + usec_per_tick - 1) / usec_per_tick;
    return 0;
}
