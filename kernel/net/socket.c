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
 * -EINTR as soon as the process is killed. */
#include "file.h"
#include "net.h"
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
}

/* The socket behind descriptor `fd` of the current process; NULL with *err
 * -EBADF (not an open descriptor) or -ENOTSOCK (not a socket). */
static struct socket *fd_sock(u32 fd, int *err) {
    struct file *f = fd < NOFILE ? myproc()->ofile[fd] : NULL;
    if (f == NULL) {
        *err = -EBADF;
        return NULL;
    }
    if (f->type != FD_SOCKET) {
        *err = -ENOTSOCK;
        return NULL;
    }
    return f->sock;
}

/* The lowest free descriptor of the current process, or -EMFILE. Nothing
 * else can take it before the caller does: a process is one thread. */
static int fd_free(void) {
    struct proc *p = myproc();
    for (u32 fd = 0; fd < NOFILE; fd++) {
        if (p->ofile[fd] == NULL) {
            return (int)fd;
        }
    }
    return -EMFILE;
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
static int get_addr(u32 uaddr, u32 *addr, u32 *port) {
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
 * which the caller has checked. */
static void put_addr(u32 uaddr, u32 addr, u32 port) {
    struct sockaddr_in a;
    memset(&a, 0, sizeof a);
    a.sin_family = AF_INET;
    put16((u8 *)&a.sin_port, port);
    put32((u8 *)&a.sin_addr, addr);
    copyout(user_pt(), uaddr, &a, sizeof a);
}

/* Whether `uaddr` can take a struct sockaddr_in (0, for the calls where it
 * is optional, can). */
static bool addr_writable(u32 uaddr, bool optional) {
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

/* Sends `n` bytes at user address `src`: on a stream, into the
 * connection; as a datagram, to `addr`:`port` if `to`, else to the peer
 * the socket is connected to. */
static int sock_send(struct socket *s, u32 src, u32 n, bool to, u32 addr, u32 port) {
    if (s->type == SOCK_STREAM) {
        /* An address is ignored: the connection says where (as on Linux). */
        return tcp_send(s->tcp, user_pt(), src, n);
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
    return udp_sendto(s->udp, addr, port, user_pt(), src, n);
}

/* Receives up to `n` bytes into user address `dst`; where they came from
 * in *addr and *port. */
static int sock_recv(struct socket *s, u32 dst, u32 n, u32 *addr, u32 *port) {
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
        return tcp_recv(s->tcp, user_pt(), dst, n, s->rcvtimeo);
    }
    if (s->udp == NULL) {
        /* No port: nothing can ever come. */
        return -EINVAL;
    }
    int len = udp_recvfrom(s->udp, user_pt(), dst, n, addr, port, s->rcvtimeo);
    if (len == -ETIMEDOUT) {
        return -EAGAIN; /* the receive timeout, as on Linux */
    }
    /* What did not fit is lost: the call says what it stored. */
    return len >= 0 && (u32)len > n ? (int)n : len;
}

int socket_read(struct socket *s, pagetable_t pt, u32 dst, u32 n) {
    (void)pt; /* the current process's, as for every system call */
    u32 addr;
    u32 port;
    return sock_recv(s, dst, n, &addr, &port);
}

int socket_write(struct socket *s, pagetable_t pt, u32 src, u32 n) {
    (void)pt;
    return sock_send(s, src, n, false, 0, 0);
}

u32 socket_bytes(struct socket *s) {
    if (s->type == SOCK_STREAM) {
        return s->tcp->rlen;
    }
    return s->udp != NULL ? udp_pending(s->udp) : 0;
}

void socket_close(struct socket *s) {
    if (s->type == SOCK_STREAM) {
        tcp_close(s->tcp);
    } else if (s->udp != NULL) {
        udp_close(s->udp);
    }
    s->type = 0;
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
    int fd = fd_free();
    if (fd < 0) {
        return fd;
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
            f->ref = 0; /* nothing else set: just give the slot back */
            return err;
        }
    }
    sock_file(f, s);
    myproc()->ofile[fd] = f;
    return fd;
}

int sock_sys_bind(u32 fd, u32 uaddr) {
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

int sock_sys_accept(u32 fd, u32 uaddr) {
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
    int nfd = fd_free();
    if (nfd < 0) {
        return nfd;
    }
    struct file *f = filealloc();
    if (f == NULL) {
        return -ENFILE;
    }
    struct tcp_sock *c = tcp_accept(s->tcp, s->rcvtimeo, &err);
    if (c == NULL) {
        f->ref = 0;
        return err;
    }
    struct socket *ns = sock_alloc(SOCK_STREAM);
    ns->tcp = c;
    sock_file(f, ns);
    myproc()->ofile[nfd] = f;
    if (uaddr != 0) {
        u32 laddr;
        u32 lport;
        u32 raddr;
        u32 rport;
        tcp_ends(c, &laddr, &lport, &raddr, &rport);
        put_addr(uaddr, raddr, rport);
    }
    return nfd;
}

int sock_sys_connect(u32 fd, u32 uaddr) {
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

int sock_sys_send(u32 fd, u32 ubuf, u32 len) {
    return sock_sys_sendto(fd, ubuf, len, 0);
}

int sock_sys_recv(u32 fd, u32 ubuf, u32 len) {
    return sock_sys_recvfrom(fd, ubuf, len, 0);
}

int sock_sys_sendto(u32 fd, u32 ubuf, u32 len, u32 uaddr) {
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
    if (uaddr != 0) {
        err = get_addr(uaddr, &addr, &port);
        if (err < 0) {
            return err;
        }
    }
    return sock_send(s, ubuf, len, uaddr != 0, addr, port);
}

int sock_sys_recvfrom(u32 fd, u32 ubuf, u32 len, u32 uaddr) {
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
    int n = sock_recv(s, ubuf, len, &addr, &port);
    if (n >= 0 && uaddr != 0) {
        put_addr(uaddr, addr, port);
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
    return 0;
}

int sock_sys_name(u32 fd, u32 uaddr, bool peer) {
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
        put_addr(uaddr, raddr, rport);
    } else {
        put_addr(uaddr, laddr, lport);
    }
    return 0;
}

int sock_sys_setsockopt(u32 fd, u32 level, u32 option, u32 uvalue, u32 len) {
    int err;
    struct socket *s = fd_sock(fd, &err);
    if (s == NULL) {
        return err;
    }
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
    u32 sec = (u32)tv.tv_sec > max_sec ? max_sec : (u32)tv.tv_sec;
    s->rcvtimeo = sec * hz + ((u32)tv.tv_usec + usec_per_tick - 1) / usec_per_tick;
    return 0;
}
