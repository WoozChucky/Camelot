/* socktest: checks the socket calls do what docs/camelot-syscalls.md says
 * ("Sockets"), from user space, over loopback (127.0.0.1) only, so it runs
 * on any machine with a network card, whatever its network (without one,
 * it says so and exits with 1): bad
 * descriptors, bad pointers, bad addresses, ports in use, TCP and UDP
 * between two sockets of its own, timeouts, shutdown, running out of
 * sockets, and a socket as another program's output. Prints a line for
 * each check that fails and, at the end, "socktest: N checks, F failed";
 * exits with F.
 *
 * It also runs as its own child, for what needs a second process:
 *   socktest tcpfill     makes TCP sockets until one fails; exits with how
 *                        many it made if that was -ENOBUFS, else 100
 *   socktest udpwait P   binds a UDP socket to port P and waits in recv;
 *                        exits 0 if it got "x", else 1 (or is killed)
 *   socktest acceptwait P listens on port P and waits in accept, the same
 *                        way: exits 0 if a connection came */
#include "user.h"

/* Descriptors per process (NOFILE in kernel/include/file.h). */
#define NOFILE 32
#define NPROC 8
/* Sockets of each kind (TCP_SOCKETS, UDP_SOCKETS in kernel/include/net.h). */
#define TCP_SOCKETS 16
#define UDP_SOCKETS 8
/* Datagrams a UDP socket holds (UDP_QUEUE_MAX), the most one carries
 * (UDP_MAX_DATA), and the first ephemeral port (TCP_EPHEMERAL_FIRST, the
 * same for UDP). */
#define UDP_QUEUE_MAX 4
#define UDP_MAX_DATA 1472
#define TCP_EPHEMERAL_FIRST 49152
/* The kernel's first page: mapped in every process, but without U. */
#define KERNEL 0xffffffff80000000ul

static int checks;
static int failures;

static const char rodata[] = "read-only";
/* Big enough for the largest datagram, and static: a stack is one page. */
static char buf[UDP_MAX_DATA + 8];

static void check(int ok, const char *what) {
    checks++;
    if (!ok) {
        failures++;
        printf("socktest: FAILED: %s\n", what);
    }
}

static void check_eq(long got, long want, const char *what) {
    checks++;
    if (got != want) {
        failures++;
        printf("socktest: FAILED: %s: got %ld, want %ld\n", what, got, want);
    }
}

static struct sockaddr_in addr_of(uint addr, uint port) {
    struct sockaddr_in a;
    sockaddr_set(&a, addr, port);
    return a;
}

/* The port of a socket, from getsockname (host order), or 0. */
static uint port_of(int fd) {
    struct sockaddr_in a;
    return getsockname(fd, &a) == 0 ? ntohs(a.sin_port) : 0;
}

/* Waits for child `pid`; returns its status, or -1000 if it isn't one. */
static int status_of(int pid) {
    int status;
    return pid > 0 && wait(pid, &status) == pid ? status : -1000;
}

static int state_of(int pid) {
    struct procinfo info[NPROC];
    int n = procs(info, NPROC);
    for (int i = 0; i < n && i < NPROC; i++) {
        if ((int)info[i].pid == pid) {
            return (int)info[i].state;
        }
    }
    return -1;
}

/* Gives up turns until `pid` has been asleep for three looks in a row. */
static void until_asleep(int pid) {
    for (int asleep = 0; asleep < 3; yield()) {
        asleep = state_of(pid) == PROC_SLEEPING ? asleep + 1 : 0;
    }
}

/* Gives up turns until `pid` is a zombie, at most 100; whether it got
 * there. */
static int ends_soon(int pid) {
    for (int i = 0; i < 100; i++) {
        if (state_of(pid) == PROC_ZOMBIE) {
            return 1;
        }
        yield();
    }
    return state_of(pid) == PROC_ZOMBIE;
}

/* A TCP listener on 127.0.0.1:`port`, or an error. */
static int listener(uint port) {
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    struct sockaddr_in a = addr_of(INADDR_LOOPBACK, port);
    if (fd < 0 || bind(fd, &a) < 0 || listen(fd, 4) < 0) {
        return -1;
    }
    return fd;
}

/* Connects a new TCP socket to 127.0.0.1:`port`. */
static int connected(uint port) {
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    struct sockaddr_in a = addr_of(INADDR_LOOPBACK, port);
    if (fd < 0) {
        return fd;
    }
    int err = connect(fd, &a);
    if (err < 0) {
        close(fd);
        return err;
    }
    return fd;
}

static void test_sockets(void) {
    check_eq(socket(10, SOCK_STREAM, 0), -EAFNOSUPPORT, "a socket of family 10 (IPv6)");
    check_eq(socket(AF_INET, 3, 0), -EINVAL, "a socket of type 3");
    check_eq(socket(AF_INET, SOCK_STREAM, IPPROTO_UDP), -EPROTONOSUPPORT, "a stream socket for UDP");
    check_eq(socket(AF_INET, SOCK_DGRAM, IPPROTO_TCP), -EPROTONOSUPPORT, "a datagram socket for TCP");
    int t = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    check_eq(t, 3, "a TCP socket gets the lowest free descriptor");
    int u = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    check_eq(u, 4, "a UDP socket too");
    struct stat st;
    check(fstat(t, &st) == 0 && st.type == T_SOCKET && st.size == 0 && st.nlink == 1,
          "fstat: a socket, nothing to read, nlink 1");
    check_eq(lseek(u, 0, SEEK_SET), -ESPIPE, "lseek on a socket");
    struct sockaddr_in a;
    check(getsockname(t, &a) == 0 && a.sin_family == AF_INET && a.sin_port == 0 && a.sin_addr.s_addr == 0,
          "a new socket's name: 0.0.0.0:0");
    check_eq(getpeername(t, &a), -ENOTCONN, "a new socket has no peer");
    check_eq(getpeername(u, &a), -ENOTCONN, "nor a new UDP socket");
    char c;
    check_eq(recv(t, &c, 1), -ENOTCONN, "recv on an unconnected TCP socket");
    check_eq(read(t, &c, 1), -ENOTCONN, "read too");
    check_eq(send(t, "x", 1), -ENOTCONN, "send on an unconnected TCP socket");
    check_eq(write(t, "x", 1), -ENOTCONN, "write too");
    check_eq(shutdown(t, SHUT_WR), -ENOTCONN, "shutdown of an unconnected TCP socket");
    check_eq(recv(u, &c, 1), -EINVAL, "recv on a UDP socket with no port");
    check_eq(send(u, "x", 1), -EDESTADDRREQ, "send on a UDP socket with no peer");
    check_eq(shutdown(u, SHUT_RD), -ENOTCONN, "shutdown of an unconnected UDP socket");
    check_eq(listen(u, 1), -EOPNOTSUPP, "listen on a UDP socket");
    check_eq(accept(u, NULL), -EOPNOTSUPP, "accept on a UDP socket");
    check_eq(accept(t, NULL), -EINVAL, "accept on a TCP socket that doesn't listen");
    /* dup shares the socket; it closes with the last descriptor. */
    int d = dup(t);
    check_eq(d, 5, "dup of a socket");
    check_eq(close(t), 0, "close the original");
    check(getsockname(d, &a) == 0, "the copy is still a socket");
    check_eq(close(d), 0, "close the copy");
    check_eq(close(u), 0, "close a UDP socket");
}

static void test_bad_descriptors(void) {
    struct sockaddr_in a = addr_of(INADDR_LOOPBACK, 7);
    char buf[4];
    struct timeval tv;
    tv.tv_sec = 1;
    tv.tv_usec = 0;
    int fds[] = {-1, NOFILE, 9};
    for (int i = 0; i < 3; i++) {
        int fd = fds[i];
        check_eq(bind(fd, &a), -EBADF, "bind on a bad descriptor");
        check_eq(listen(fd, 1), -EBADF, "listen on a bad descriptor");
        check_eq(accept(fd, &a), -EBADF, "accept on a bad descriptor");
        check_eq(connect(fd, &a), -EBADF, "connect on a bad descriptor");
        check_eq(send(fd, buf, 1), -EBADF, "send on a bad descriptor");
        check_eq(recv(fd, buf, 1), -EBADF, "recv on a bad descriptor");
        check_eq(sendto(fd, buf, 1, &a), -EBADF, "sendto on a bad descriptor");
        check_eq(recvfrom(fd, buf, 1, &a), -EBADF, "recvfrom on a bad descriptor");
        check_eq(shutdown(fd, SHUT_RDWR), -EBADF, "shutdown on a bad descriptor");
        check_eq(getsockname(fd, &a), -EBADF, "getsockname on a bad descriptor");
        check_eq(getpeername(fd, &a), -EBADF, "getpeername on a bad descriptor");
        check_eq(setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv), -EBADF, "setsockopt on a bad descriptor");
    }
    /* A descriptor that is open but no socket: the console, a pipe. Bad
     * pointers don't matter: the descriptor is checked first. */
    int p[2];
    check_eq(pipe(p), 0, "a pipe");
    int others[] = {STDOUT, p[0]};
    for (int i = 0; i < 2; i++) {
        int fd = others[i];
        check_eq(bind(fd, (struct sockaddr_in *)KERNEL), -ENOTSOCK, "bind on no socket");
        check_eq(listen(fd, 1), -ENOTSOCK, "listen on no socket");
        check_eq(accept(fd, &a), -ENOTSOCK, "accept on no socket");
        check_eq(connect(fd, &a), -ENOTSOCK, "connect on no socket");
        check_eq(send(fd, buf, 1), -ENOTSOCK, "send on no socket");
        check_eq(recv(fd, buf, 1), -ENOTSOCK, "recv on no socket");
        check_eq(sendto(fd, buf, 1, &a), -ENOTSOCK, "sendto on no socket");
        check_eq(recvfrom(fd, buf, 1, &a), -ENOTSOCK, "recvfrom on no socket");
        check_eq(shutdown(fd, SHUT_RD), -ENOTSOCK, "shutdown on no socket");
        check_eq(getsockname(fd, &a), -ENOTSOCK, "getsockname on no socket");
        check_eq(getpeername(fd, &a), -ENOTSOCK, "getpeername on no socket");
        check_eq(setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv), -ENOTSOCK, "setsockopt on no socket");
    }
    close(p[0]);
    close(p[1]);
}

static void test_bad_pointers(void) {
    int t = socket(AF_INET, SOCK_STREAM, 0);
    int u = socket(AF_INET, SOCK_DGRAM, 0);
    struct sockaddr_in *kernel = (struct sockaddr_in *)KERNEL;
    struct sockaddr_in *ro = (struct sockaddr_in *)rodata;
    char *brk = sbrk(0);
    struct sockaddr_in *past = (struct sockaddr_in *)(brk - 8); /* runs past the heap */
    check_eq(bind(t, NULL), -EFAULT, "bind to address NULL");
    check_eq(bind(t, kernel), -EFAULT, "bind to kernel memory");
    check_eq(bind(u, past), -EFAULT, "bind to an address past the heap");
    check_eq(connect(t, NULL), -EFAULT, "connect to NULL");
    check_eq(connect(u, kernel), -EFAULT, "connect a UDP socket to kernel memory");
    check_eq(sendto(u, "x", 1, kernel), -EFAULT, "sendto kernel memory");
    check_eq(sendto(u, (char *)KERNEL, 1, NULL), -EFAULT, "sendto from kernel memory");
    check_eq(send(u, NULL, 1), -EFAULT, "send from NULL");
    check_eq(write(u, NULL, 1), -EFAULT, "write from NULL");
    check_eq(recv(u, (char *)rodata, 1), -EFAULT, "recv into read-only memory");
    check_eq(read(u, (char *)rodata, 1), -EFAULT, "read too");
    char c;
    check_eq(recvfrom(u, &c, 1, ro), -EFAULT, "recvfrom with its sender into read-only memory");
    check_eq(getsockname(t, NULL), -EFAULT, "getsockname into NULL");
    check_eq(getsockname(t, ro), -EFAULT, "getsockname into read-only memory");
    check_eq(getpeername(t, kernel), -EFAULT, "getpeername into kernel memory");
    check_eq(setsockopt(t, SOL_SOCKET, SO_RCVTIMEO, kernel, sizeof(struct timeval)), -EFAULT,
             "setsockopt from kernel memory");
    /* accept checks its pointer before it waits. */
    struct sockaddr_in a = addr_of(INADDR_LOOPBACK, 0);
    check_eq(bind(t, &a), 0, "bind to an ephemeral port");
    check_eq(listen(t, 1), 0, "listen");
    check_eq(accept(t, ro), -EFAULT, "accept into read-only memory");
    check_eq(accept(t, kernel), -EFAULT, "accept into kernel memory");
    /* Nothing above changed anything: the UDP socket still has no port. */
    check_eq(port_of(u), 0, "the UDP socket is still unbound");
    close(t);
    close(u);
}

static void test_addresses_and_ports(void) {
    int t = socket(AF_INET, SOCK_STREAM, 0);
    int u = socket(AF_INET, SOCK_DGRAM, 0);
    struct sockaddr_in a = addr_of(INADDR_LOOPBACK, 7000);
    a.sin_family = 1;
    check_eq(bind(t, &a), -EAFNOSUPPORT, "bind with family 1");
    check_eq(connect(t, &a), -EAFNOSUPPORT, "connect with family 1");
    check_eq(sendto(u, "x", 1, &a), -EAFNOSUPPORT, "sendto with family 1");
    a = addr_of(0x08080808, 7000);
    check_eq(bind(t, &a), -EADDRNOTAVAIL, "bind to another host's address");
    check_eq(bind(u, &a), -EADDRNOTAVAIL, "bind a UDP socket to another host's address");
    a = addr_of(INADDR_LOOPBACK, 0);
    check_eq(connect(t, &a), -EINVAL, "connect to port 0");
    a = addr_of(0, 80);
    check_eq(connect(t, &a), -EINVAL, "connect to 0.0.0.0");
    a = addr_of(0xffffffff, 80);
    check_eq(connect(t, &a), -EINVAL, "connect to the broadcast address");
    a = addr_of(0xe0000001, 80);
    check_eq(connect(t, &a), -EINVAL, "connect to a multicast address");
    a = addr_of(INADDR_LOOPBACK, 0);
    check_eq(sendto(u, "x", 1, &a), -EINVAL, "sendto port 0");
    a = addr_of(0, 7);
    check_eq(sendto(u, "x", 1, &a), -EINVAL, "sendto 0.0.0.0");
    check_eq(connect(u, &a), -EINVAL, "connect a UDP socket to 0.0.0.0");
    close(t);
    close(u);

    /* Ports in use: a TCP listener's port, a bound TCP socket's, a UDP one's. */
    int l = listener(7001);
    check(l >= 0, "a listener on 7001");
    t = socket(AF_INET, SOCK_STREAM, 0);
    a = addr_of(INADDR_ANY, 7001);
    check_eq(bind(t, &a), -EADDRINUSE, "bind to a listener's port");
    a = addr_of(INADDR_ANY, 7002);
    check_eq(bind(t, &a), 0, "bind to a free port");
    check_eq(bind(t, &a), -EINVAL, "bind twice");
    int t2 = socket(AF_INET, SOCK_STREAM, 0);
    check_eq(bind(t2, &a), -EADDRINUSE, "bind to a bound socket's port");
    check_eq(listen(t, 1), 0, "listen on the bound one");
    check_eq(listen(t, 1), -EINVAL, "listen twice");
    close(t);
    check_eq(bind(t2, &a), 0, "the port is free once its socket is closed");
    close(t2);
    close(l);
    u = socket(AF_INET, SOCK_DGRAM, 0);
    int u2 = socket(AF_INET, SOCK_DGRAM, 0);
    a = addr_of(INADDR_LOOPBACK, 7003);
    check_eq(bind(u, &a), 0, "bind a UDP socket");
    check_eq(port_of(u), 7003, "its port");
    check_eq(bind(u2, &a), -EADDRINUSE, "bind a second UDP socket to the port");
    check_eq(bind(u, &a), -EINVAL, "bind a UDP socket twice");
    /* UDP and TCP ports are apart. */
    l = listener(7003);
    check(l >= 0, "a TCP listener on a UDP socket's port");
    close(l);
    close(u);
    close(u2);

    /* Ephemeral ports: a listener without bind, a UDP socket's first send. */
    t = socket(AF_INET, SOCK_STREAM, 0);
    check_eq(listen(t, 1), 0, "listen without bind");
    check(port_of(t) >= TCP_EPHEMERAL_FIRST, "takes an ephemeral port");
    close(t);
    u = socket(AF_INET, SOCK_DGRAM, 0);
    a = addr_of(INADDR_LOOPBACK, 9);
    check_eq(sendto(u, "x", 1, &a), 1, "a UDP socket's first sendto");
    check(port_of(u) >= TCP_EPHEMERAL_FIRST, "binds it to an ephemeral port");
    close(u);
}

static void test_tcp(void) {
    int l = listener(7010);
    check(l >= 0, "a listener on 7010");
    /* Over loopback the handshake is over by the time connect returns: the
     * listener took the connection, which waits for accept. */
    int c = connected(7010);
    check(c >= 0, "connect to it");
    struct sockaddr_in peer;
    int s = accept(l, &peer);
    check(s >= 0, "accept the connection");
    struct sockaddr_in here;
    check(getsockname(c, &here) == 0 && ntohl(here.sin_addr.s_addr) == INADDR_LOOPBACK, "the client's address");
    check(ntohl(peer.sin_addr.s_addr) == INADDR_LOOPBACK && peer.sin_port == here.sin_port,
          "accept says who connected");
    struct sockaddr_in there;
    check(getpeername(s, &there) == 0 && there.sin_port == here.sin_port, "getpeername on the accepted one");
    check(getpeername(c, &there) == 0 && ntohs(there.sin_port) == 7010, "getpeername on the client");
    check(getsockname(s, &here) == 0 && ntohs(here.sin_port) == 7010, "the accepted one has the listener's port");
    struct sockaddr_in a = addr_of(INADDR_LOOPBACK, 7010);
    check_eq(connect(c, &a), -EISCONN, "connect twice");
    check_eq(listen(c, 1), -EINVAL, "listen on a connection");

    /* Data both ways, through send/recv and write/read. */
    char buf[16];
    check_eq(send(c, "hello", 5), 5, "send");
    struct stat st;
    check(fstat(s, &st) == 0 && st.size == 5, "fstat says 5 bytes wait");
    check_eq(st.nlink, 1, "and nlink 1: more can come");
    check_eq(recv(s, buf, 2), 2, "recv part of it");
    check_eq(recv(s, buf + 2, sizeof buf), 3, "recv the rest");
    check(memcmp(buf, "hello", 5) == 0, "the bytes");
    check_eq(write(s, "world", 5), 5, "write");
    check_eq(read(c, buf, sizeof buf), 5, "read");
    check(memcmp(buf, "world", 5) == 0, "the bytes back");
    check_eq(recv(c, buf, 0), 0, "recv of 0 bytes");
    check_eq(sendto(c, "!", 1, &a), 1, "sendto on a connection ignores the address");
    check_eq(recvfrom(s, buf, 1, &there), 1, "recvfrom on a connection");
    check(there.sin_port == peer.sin_port && ntohl(there.sin_addr.s_addr) == INADDR_LOOPBACK,
          "recvfrom says the peer");
    struct sockaddr_in other = a;
    other.sin_family = 1;
    check_eq(sendto(c, "?", 1, &other), 1, "even an address of family 1");
    check(recv(s, buf, sizeof buf) == 1 && buf[0] == '?', "it went through the connection");
    check_eq(sendto(c, "?", 1, (struct sockaddr_in *)KERNEL), -EFAULT, "but the pointer is checked");
    check_eq(write(c, "", 0), 0, "write of 0 bytes on a connection");

    /* A receive timeout: nothing comes. */
    struct timeval tv;
    tv.tv_sec = 0;
    tv.tv_usec = 100000; /* a tick */
    check_eq(setsockopt(s, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv), 0, "set a receive timeout");
    uint start = uptime();
    check_eq(recv(s, buf, 1), -EAGAIN, "recv times out");
    check(uptime() - start <= 3, "after about a tick");
    check_eq(setsockopt(s, SOL_SOCKET, 99, &tv, sizeof tv), -ENOPROTOOPT, "an unknown option");
    check_eq(setsockopt(s, 2, SO_RCVTIMEO, &tv, sizeof tv), -ENOPROTOOPT, "an unknown level");
    check_eq(setsockopt(s, SOL_SOCKET, SO_RCVTIMEO, &tv, 4), -EINVAL, "a value of the wrong size");
    tv.tv_usec = 1000000;
    check_eq(setsockopt(s, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv), -EINVAL, "a million microseconds");
    tv.tv_usec = 0;
    tv.tv_sec = -1;
    check_eq(setsockopt(s, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv), -EINVAL, "negative seconds");
    tv.tv_sec = 0;
    tv.tv_usec = 100000;
    check_eq(setsockopt(l, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv), 0, "a timeout on the listener");
    check_eq(accept(l, NULL), -EAGAIN, "accept times out");

    /* Ending: shutdown sends the FIN, the other side reads the end. */
    check_eq(shutdown(c, 7), -EINVAL, "shutdown with an unknown how");
    check_eq(shutdown(c, SHUT_WR), 0, "shutdown for writing");
    check_eq(shutdown(c, SHUT_WR), 0, "shutdown twice");
    check_eq(send(c, "x", 1), -EPIPE, "send after shutdown");
    check_eq(recv(s, buf, sizeof buf), 0, "the other side reads the end");
    check_eq(recv(s, buf, sizeof buf), 0, "and again");
    check(fstat(s, &st) == 0 && st.nlink == 0 && st.size == 0, "fstat: nlink 0, the other end hung up");
    check(fstat(c, &st) == 0 && st.nlink == 1, "fstat: the half-closed side can still receive");
    check_eq(send(s, "late", 4), 4, "it can still send");
    check_eq(recv(c, buf, sizeof buf), 4, "and the half-closed side read");
    check_eq(shutdown(c, SHUT_RD), 0, "shutdown for reading");
    check_eq(send(s, "more", 4), 4, "send to it");
    check_eq(recv(c, buf, sizeof buf), 0, "a socket shut for reading reads the end");
    check(fstat(c, &st) == 0 && st.nlink == 0, "fstat: nlink 0 once shut for reading");
    close(s);
    close(c);

    /* Closing ends the connection: the other side reads what was sent,
     * then the end. */
    c = connected(7010);
    s = accept(l, NULL);
    check(c >= 0 && s >= 0, "a second connection");
    check_eq(send(c, "bye", 3), 3, "send before closing");
    check_eq(close(c), 0, "close it");
    check_eq(recv(s, buf, sizeof buf), 3, "the bytes sent before the close");
    check_eq(recv(s, buf, sizeof buf), 0, "then the end");
    check(fstat(s, &st) == 0 && st.nlink == 0, "fstat: nlink 0 after the other end closed");
    close(s);
    close(l);

    /* Nobody listens: refused. Listening again on the port works. */
    c = socket(AF_INET, SOCK_STREAM, 0);
    a = addr_of(INADDR_LOOPBACK, 7010);
    check_eq(connect(c, &a), -ECONNREFUSED, "connect to a port nobody listens on");
    check_eq(shutdown(c, SHUT_RD), -ECONNREFUSED, "shutdown for reading says what ended it");
    check_eq(shutdown(c, SHUT_WR), -ECONNREFUSED, "for writing too");
    check_eq(shutdown(c, SHUT_RDWR), -ECONNREFUSED, "and both");
    check(fstat(c, &st) == 0 && st.nlink == 0, "fstat: nlink 0 for a refused connection");
    close(c);
    l = listener(7010);
    check(l >= 0, "listen on the port again");
    close(l);
}

/* A socket as the output of another program: echo writes into it. */
static void test_inherited(void) {
    int l = listener(7020);
    int c = connected(7020);
    int s = accept(l, NULL);
    close(l);
    int out = dup(STDOUT);
    close(STDOUT);
    dup(c);
    int pid = spawn("/bin/echo", (char *[]){"echo", "through", "a", "socket", NULL}, environ);
    close(STDOUT);
    dup(out);
    close(out);
    close(c);
    check_eq(status_of(pid), 0, "echo writes into a socket");
    char buf[32];
    int n = 0;
    for (;;) {
        int r = (int)recv(s, buf + n, sizeof buf - 1 - (size_t)n);
        if (r <= 0) {
            check_eq(r, 0, "the end, once echo exited and closed the last descriptor");
            break;
        }
        n += r;
    }
    buf[n] = 0;
    check(strcmp(buf, "through a socket\n") == 0, "what echo wrote");
    close(s);
}

static void test_udp(void) {
    int a = socket(AF_INET, SOCK_DGRAM, 0);
    int b = socket(AF_INET, SOCK_DGRAM, 0);
    struct sockaddr_in to = addr_of(INADDR_LOOPBACK, 7030);
    check_eq(bind(a, &to), 0, "bind a UDP socket to 7030");
    check_eq(sendto(b, "one", 3, &to), 3, "sendto it");
    uint bport = port_of(b);
    struct stat st;
    check(fstat(a, &st) == 0 && st.size == 3, "fstat: the next datagram's length");
    struct sockaddr_in from;
    check_eq(recvfrom(a, buf, sizeof buf, &from), 3, "recvfrom gets the datagram");
    check(memcmp(buf, "one", 3) == 0, "its bytes");
    check(ntohl(from.sin_addr.s_addr) == INADDR_LOOPBACK && ntohs(from.sin_port) == bport,
          "recvfrom says who sent it");
    /* A datagram bigger than the buffer: the rest is lost. */
    check_eq(sendto(b, "truncated", 9, &to), 9, "a longer datagram");
    check_eq(recv(a, buf, 5), 5, "recv takes what fits");
    check_eq(sendto(b, "next", 4, &to), 4, "and another");
    check_eq(recv(a, buf, sizeof buf), 4, "the rest of the first is gone");
    check_eq(sendto(b, buf, UDP_MAX_DATA + 1, &to), -EMSGSIZE, "a datagram over 1472 bytes");
    memset(buf, 'z', UDP_MAX_DATA);
    check_eq(sendto(b, buf, UDP_MAX_DATA, &to), UDP_MAX_DATA, "a datagram of 1472 bytes");
    check_eq(recv(a, buf, sizeof buf), UDP_MAX_DATA, "arrives whole");
    check_eq(sendto(b, "", 0, &to), 0, "an empty datagram");
    check_eq(recv(a, buf, sizeof buf), 0, "arrives, empty");

    /* The queue holds 4; the 5th is dropped. */
    for (int i = 0; i < UDP_QUEUE_MAX + 1; i++) {
        char d = (char)('0' + i);
        check_eq(sendto(b, &d, 1, &to), 1, "a datagram for the queue");
    }
    for (int i = 0; i < UDP_QUEUE_MAX; i++) {
        check(recv(a, buf, sizeof buf) == 1 && buf[0] == '0' + i, "the queue, in order");
    }
    struct timeval tv;
    tv.tv_sec = 0;
    tv.tv_usec = 1;
    check_eq(setsockopt(a, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv), 0, "a short receive timeout");
    check_eq(recv(a, buf, sizeof buf), -EAGAIN, "the 5th was dropped: recv times out");

    /* Connected: send and recv without addresses, and only the peer's
     * datagrams come. */
    int c = socket(AF_INET, SOCK_DGRAM, 0);
    struct sockaddr_in bto = addr_of(INADDR_LOOPBACK, bport);
    check_eq(connect(a, &bto), 0, "connect a to b");
    struct sockaddr_in peer;
    check(getpeername(a, &peer) == 0 && ntohs(peer.sin_port) == bport, "a's peer is b");
    check_eq(sendto(c, "c", 1, &to), 1, "c sends to a");
    check_eq(sendto(b, "b", 1, &to), 1, "b sends to a");
    check(recv(a, buf, sizeof buf) == 1 && buf[0] == 'b', "a gets only b's");
    check_eq(send(a, "to b", 4), 4, "send without an address goes to b");
    check(recvfrom(b, buf, sizeof buf, &from) == 4 && ntohs(from.sin_port) == 7030, "b gets it from a");
    check_eq(write(a, "w", 1), 1, "write on a connected UDP socket");
    check_eq(read(b, buf, sizeof buf), 1, "read on a UDP socket");
    check_eq(shutdown(a, SHUT_WR), 0, "shutdown a connected UDP socket for sending");
    check_eq(send(a, "x", 1), -EPIPE, "send after it");
    check_eq(shutdown(a, SHUT_RD), 0, "and for reading");
    check_eq(recv(a, buf, sizeof buf), 0, "recv reads the end");
    close(a);
    close(b);
    close(c);

    /* read and write are recv and send, for 0 bytes too: an empty
     * datagram goes, and a read of 0 bytes takes the next one. */
    a = socket(AF_INET, SOCK_DGRAM, 0);
    b = socket(AF_INET, SOCK_DGRAM, 0);
    to = addr_of(INADDR_LOOPBACK, 7031);
    tv.tv_sec = 0;
    tv.tv_usec = 300000;
    check_eq(bind(a, &to), 0, "bind a UDP socket to 7031");
    check_eq(setsockopt(a, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv), 0, "a receive timeout");
    check_eq(connect(b, &to), 0, "connect another to it");
    check_eq(write(b, "", 0), 0, "write of 0 bytes on a connected UDP socket");
    check_eq(recv(a, buf, sizeof buf), 0, "sends an empty datagram");
    check_eq(send(b, "r", 1), 1, "a datagram");
    check_eq(read(a, buf, 0), 0, "read of 0 bytes");
    check_eq(recv(a, buf, sizeof buf), -EAGAIN, "took it");

    /* Port 7 held by a socket connected to someone else: what others send
     * to it is dropped, and the echo service, which answers only while no
     * socket has the port, stays quiet. */
    struct sockaddr_in seven = addr_of(INADDR_LOOPBACK, 7);
    c = socket(AF_INET, SOCK_DGRAM, 0);
    check_eq(setsockopt(c, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv), 0, "a receive timeout");
    int e = socket(AF_INET, SOCK_DGRAM, 0);
    check_eq(bind(e, &seven), 0, "bind a UDP socket to port 7");
    check_eq(setsockopt(e, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv), 0, "a receive timeout");
    struct sockaddr_in elsewhere = addr_of(INADDR_LOOPBACK, 9000);
    check_eq(connect(e, &elsewhere), 0, "connect it elsewhere");
    check_eq(sendto(c, "hi", 2, &seven), 2, "send to port 7");
    check_eq(recv(c, buf, sizeof buf), -EAGAIN, "no echo while a socket has the port");
    check_eq(recv(e, buf, sizeof buf), -EAGAIN, "nor does the socket get it");
    close(e);
    check_eq(sendto(c, "hi", 2, &seven), 2, "send to port 7 once it's free");
    check(recv(c, buf, sizeof buf) == 2 && memcmp(buf, "hi", 2) == 0, "the echo answers again");

    /* Connected to 127.0.0.2: this host too, and the replies, from
     * 127.0.0.1, are the peer's. */
    struct sockaddr_in two = addr_of(INADDR_LOOPBACK + 1, 7031);
    check_eq(connect(c, &two), 0, "connect to 127.0.0.2");
    check_eq(send(c, "q", 1), 1, "send to it");
    check_eq(recvfrom(a, buf, sizeof buf, &from), 1, "it comes");
    check_eq(sendto(a, "r", 1, &from), 1, "a reply");
    check(recv(c, buf, sizeof buf) == 1 && buf[0] == 'r', "the reply comes from the peer");
    close(a);
    close(b);
    close(c);

    /* Running out: 8 UDP sockets with ports. (DHCP's is closed by now.) */
    int fds[UDP_SOCKETS + 1];
    int made = 0;
    int err = 0;
    for (int i = 0; i <= UDP_SOCKETS; i++) {
        fds[i] = socket(AF_INET, SOCK_DGRAM, 0);
        struct sockaddr_in any = addr_of(INADDR_ANY, 0);
        err = bind(fds[i], &any);
        if (err < 0) {
            break;
        }
        made++;
    }
    check_eq(made, UDP_SOCKETS, "UDP sockets with a port");
    check_eq(err, -ENOBUFS, "then no more");
    for (int i = 0; i <= made && i <= UDP_SOCKETS; i++) {
        close(fds[i]);
    }
}

/* Waits, killed while it does: the call ends at once. */
static void test_killed_waits(void) {
    int pid = spawn("/bin/socktest", (char *[]){"socktest", "udpwait", "7040", NULL}, environ);
    until_asleep(pid);
    check_eq(kill(pid), 0, "kill a process in recv");
    check(ends_soon(pid), "it ends at once");
    check_eq(status_of(pid), -1, "killed");
    pid = spawn("/bin/socktest", (char *[]){"socktest", "acceptwait", "7041", NULL}, environ);
    until_asleep(pid);
    check_eq(kill(pid), 0, "kill a process in accept");
    check(ends_soon(pid), "it ends at once");
    check_eq(status_of(pid), -1, "killed");
    /* Its sockets went with it: the ports are free. */
    int l = listener(7041);
    check(l >= 0, "a killed process's port is free again");
    close(l);
    /* And one that gets what it waits for. */
    pid = spawn("/bin/socktest", (char *[]){"socktest", "udpwait", "7040", NULL}, environ);
    until_asleep(pid);
    int u = socket(AF_INET, SOCK_DGRAM, 0);
    struct sockaddr_in to = addr_of(INADDR_LOOPBACK, 7040);
    check_eq(sendto(u, "x", 1, &to), 1, "send to the waiting process");
    check_eq(status_of(pid), 0, "it got it");
    close(u);
}

/* Running out of TCP sockets: this process takes all but 3, a child makes
 * the 3 left, then the process the child gave back; and with a socket
 * free but every descriptor taken, socket() is -EMFILE. */
static void test_running_out(void) {
    int fds[NOFILE];
    int n = 0;
    while (n < TCP_SOCKETS - 3) {
        int fd = socket(AF_INET, SOCK_STREAM, 0);
        if (fd < 0) {
            break;
        }
        fds[n++] = fd;
    }
    check_eq(n, TCP_SOCKETS - 3, "TCP sockets but 3");
    int pid = spawn("/bin/socktest", (char *[]){"socktest", "tcpfill", NULL}, environ);
    check_eq(status_of(pid), TCP_SOCKETS - n, "the child gets the rest, then -ENOBUFS");
    for (;;) {
        int fd = socket(AF_INET, SOCK_STREAM, 0);
        if (fd < 0) {
            check_eq(fd, -ENOBUFS, "then the sockets run out, descriptors left");
            break;
        }
        fds[n++] = fd;
    }
    check_eq(n, TCP_SOCKETS, "every TCP socket");
    close(fds[--n]);
    for (;;) {
        int fd = dup(fds[0]);
        if (fd < 0) {
            break;
        }
        fds[n++] = fd;
    }
    check_eq(n, NOFILE - 3, "every descriptor taken");
    check_eq(socket(AF_INET, SOCK_STREAM, 0), -EMFILE, "a socket free, but no descriptor for it");
    for (int i = 0; i < n; i++) {
        close(fds[i]);
    }
}

/* accept with no descriptor free: -EMFILE, and the connection stays on
 * the listener for the next accept, which a descriptor freed lets through. */
static void test_accept_without_a_descriptor(void) {
    int l = listener(7050);
    int c = connected(7050);
    check(l >= 0 && c >= 0, "a connection waits on a listener");
    /* A connection lost would make the next accept wait: not for ever. */
    struct timeval tv;
    tv.tv_sec = 1;
    tv.tv_usec = 0;
    check_eq(setsockopt(l, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv), 0, "a timeout on the listener");
    int fds[NOFILE];
    int n = 0;
    for (;;) {
        int fd = dup(c);
        if (fd < 0) {
            check_eq(fd, -EMFILE, "dup until no descriptor is left");
            break;
        }
        fds[n] = fd;
        n++;
    }
    struct sockaddr_in peer;
    check_eq(accept(l, &peer), -EMFILE, "accept with no descriptor free");
    check_eq(accept(l, &peer), -EMFILE, "again");
    close(fds[n - 1]);
    int s = accept(l, &peer);
    check_eq(s, fds[n - 1], "accept takes the descriptor freed");
    struct sockaddr_in here;
    check(s >= 0 && getsockname(c, &here) == 0 && peer.sin_port == here.sin_port,
          "and the connection that waited");
    char b = 0;
    check_eq(send(c, "z", 1), 1, "send on it");
    check(recv(s, &b, 1) == 1 && b == 'z', "it arrives");
    for (int i = 0; i < n - 1; i++) {
        close(fds[i]);
    }
    close(s);
    close(c);
    close(l);
}

static int child(int argc, char **argv) {
    if (strcmp(argv[1], "tcpfill") == 0) {
        int made = 0;
        for (;;) {
            int fd = socket(AF_INET, SOCK_STREAM, 0);
            if (fd < 0) {
                return fd == -ENOBUFS ? made : 100;
            }
            made++;
        }
    }
    if (strcmp(argv[1], "udpwait") == 0 && argc == 3) {
        int u = socket(AF_INET, SOCK_DGRAM, 0);
        struct sockaddr_in a = addr_of(INADDR_LOOPBACK, (uint)atoi(argv[2]));
        char c = 0;
        return bind(u, &a) == 0 && recv(u, &c, 1) == 1 && c == 'x' ? 0 : 1;
    }
    if (strcmp(argv[1], "acceptwait") == 0 && argc == 3) {
        int l = listener((uint)atoi(argv[2]));
        return l >= 0 && accept(l, NULL) >= 0 ? 0 : 1;
    }
    return 2;
}

int main(int argc, char **argv) {
    if (argc > 1) {
        return child(argc, argv);
    }
    int probe = socket(AF_INET, SOCK_DGRAM, 0);
    if (probe == -ENODEV) {
        fprintf(STDERR, "socktest: no network card\n");
        return 1;
    }
    close(probe);
    test_sockets();
    test_bad_descriptors();
    test_bad_pointers();
    test_addresses_and_ports();
    test_tcp();
    test_inherited();
    test_udp();
    test_killed_waits();
    test_running_out();
    test_accept_without_a_descriptor();
    printf("socktest: %d checks, %d failed\n", checks, failures);
    return failures;
}
