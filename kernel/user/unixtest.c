/* unixtest: checks local sockets (docs/camelot-syscalls.md, "Local
 * sockets" and "Messages"; kernel/net/unix.c): socketpair, names bound on
 * the disk, connections, shutdown and close, descriptors passed with
 * SCM_RIGHTS, and sendmsg and recvmsg on TCP and UDP too. On a root image
 * it can write to, with a network card (for TCP and UDP over loopback).
 *
 *   unixtest                the checks: "unixtest: N checks, F failed".
 *                           The last of them share a memfd with a second
 *                           process: this one binds /unixtest.share and
 *                           starts "unixtest client", which connects and
 *                           is sent the object; both map it, and each sees
 *                           what the other wrote
 *   unixtest client PATH    that second process: exits 0 if all went
 *                           well, else 1 */
#include "user.h"

static int checks;
static int failed;

static void check(int ok, const char *what) {
    checks++;
    if (!ok) {
        failed++;
        printf("unixtest: FAILED: %s\n", what);
    }
}

static void check_eq(long got, long want, const char *what) {
    checks++;
    if (got != want) {
        failed++;
        printf("unixtest: FAILED: %s: got %ld, want %ld\n", what, got, want);
    }
}

#define PATH "/unixtest.sock"
#define OTHER "/unixtest.other"
#define SHARE "/unixtest.share"

/* The bytes a socket's ring holds (kernel/net/unix.c's UNIX_BUF). */
#define RING 2048

static void set_name(struct sockaddr_un *a, const char *path) {
    memset(a, 0, sizeof *a);
    a->sun_family = AF_UNIX;
    strcpy(a->sun_path, path);
}

/* A control buffer for `n` descriptors at most, 8-byte aligned. */
union control {
    struct cmsghdr h;
    char buf[CMSG_SPACE(16 * sizeof(int))];
};

/* sendmsg() of `len` bytes at `data` with the `n` descriptors of fds[]. */
static long send_fds(int sock, const char *data, size_t len, const int *fds, int n, int flags) {
    struct iovec v;
    v.iov_base = (void *)data;
    v.iov_len = len;
    union control c;
    memset(&c, 0, sizeof c);
    struct msghdr m;
    memset(&m, 0, sizeof m);
    m.msg_iov = &v;
    m.msg_iovlen = 1;
    if (n > 0) {
        m.msg_control = c.buf;
        m.msg_controllen = CMSG_SPACE(n * sizeof(int));
        struct cmsghdr *h = CMSG_FIRSTHDR(&m);
        h->cmsg_len = CMSG_LEN(n * sizeof(int));
        h->cmsg_level = SOL_SOCKET;
        h->cmsg_type = SCM_RIGHTS;
        memcpy(CMSG_DATA(h), fds, n * sizeof(int));
    }
    return sendmsg(sock, &m, flags);
}

/* What recv_fds() got besides the bytes. */
struct got {
    int nfds;
    int fds[16];
    int flags;
    size_t controllen;
};

/* recvmsg() of up to `size` bytes into `data`, with room for `room`
 * descriptors' bytes of control buffer (`controllen` exactly if not 0). */
static long recv_fds(int sock, char *data, size_t size, size_t controllen, int flags, struct got *g) {
    struct iovec v;
    v.iov_base = data;
    v.iov_len = size;
    union control c;
    memset(&c, 0, sizeof c);
    struct msghdr m;
    memset(&m, 0, sizeof m);
    m.msg_iov = &v;
    m.msg_iovlen = 1;
    m.msg_control = c.buf;
    m.msg_controllen = controllen;
    m.msg_flags = 0x5555;
    long r = recvmsg(sock, &m, flags);
    g->nfds = 0;
    g->flags = m.msg_flags;
    g->controllen = m.msg_controllen;
    for (struct cmsghdr *h = CMSG_FIRSTHDR(&m); h != NULL; h = CMSG_NXTHDR(&m, h)) {
        if (h->cmsg_level == SOL_SOCKET && h->cmsg_type == SCM_RIGHTS) {
            int n = (int)((h->cmsg_len - CMSG_LEN(0)) / sizeof(int));
            memcpy(g->fds + g->nfds, CMSG_DATA(h), n * sizeof(int));
            g->nfds += n;
        }
    }
    return r;
}

static int is_open(int fd) {
    struct stat st;
    return fstat(fd, &st) == 0;
}

static void pair_checks(void) {
    int sv[2];
    check_eq(socketpair(AF_UNIX, SOCK_STREAM, 0, sv), 0, "socketpair");
    check_eq(write(sv[0], "hello", 5), 5, "write one end");
    struct stat st;
    check(fstat(sv[1], &st) == 0 && st.type == T_SOCKET && st.size == 5 && st.nlink == 1,
          "fstat: a socket, with 5 bytes to read");
    char buf[64];
    check_eq(read(sv[1], buf, sizeof buf), 5, "read the other");
    check(memcmp(buf, "hello", 5) == 0, "the bytes");
    check_eq(send(sv[1], "back", 4), 4, "send the other way");
    check_eq(recv(sv[0], buf, sizeof buf), 4, "recv");

    /* Nothing waiting: MSG_DONTWAIT says so rather than wait. */
    struct got g;
    check_eq(recv_fds(sv[0], buf, sizeof buf, 0, MSG_DONTWAIT, &g), -EAGAIN, "MSG_DONTWAIT on nothing");

    /* The ring takes RING bytes, then a writer that doesn't wait stops. */
    char block[256];
    memset(block, 'x', sizeof block);
    long total = 0;
    for (int i = 0; i < 20; i++) {
        long r = send_fds(sv[0], block, sizeof block, 0, 0, MSG_DONTWAIT);
        if (r < 0) {
            check_eq(r, -EAGAIN, "a full ring: -EAGAIN");
            break;
        }
        total += r;
    }
    check_eq(total, RING, "the ring holds 2 KiB");
    check(fstat(sv[1], &st) == 0 && st.size == RING, "fstat counts them");
    while (total > 0) {
        total -= read(sv[1], block, sizeof block);
    }

    /* shutdown: SHUT_WR ends the peer's reading, not its writing. */
    check_eq(shutdown(sv[0], SHUT_WR), 0, "shutdown SHUT_WR");
    check_eq(read(sv[1], buf, sizeof buf), 0, "the peer reads the end");
    check_eq(write(sv[0], "x", 1), -EPIPE, "and this end sends no more");
    check_eq(write(sv[1], "y", 1), 1, "the peer still sends");
    check_eq(read(sv[0], buf, sizeof buf), 1, "and this end still reads");
    check_eq(shutdown(sv[0], 3), -EINVAL, "shutdown 3");

    /* Close one end: the other reads the end and can't send. */
    check_eq(close(sv[0]), 0, "close one end");
    check_eq(write(sv[1], "z", 1), -EPIPE, "write to a closed peer");
    check_eq(read(sv[1], buf, sizeof buf), 0, "read after the peer closed");
    check(fstat(sv[1], &st) == 0 && st.nlink == 0, "fstat: nothing more will come");
    close(sv[1]);

    /* Unread bytes when the peer closes: reset, once. */
    socketpair(AF_UNIX, SOCK_STREAM, 0, sv);
    write(sv[0], "lost", 4);
    close(sv[1]);
    check_eq(read(sv[0], buf, sizeof buf), -ECONNRESET, "the peer closed with bytes unread: reset");
    check_eq(read(sv[0], buf, sizeof buf), 0, "then the end");
    close(sv[0]);

    check_eq(socketpair(AF_INET, SOCK_STREAM, 0, sv), -EOPNOTSUPP, "socketpair of TCP");
    check_eq(socketpair(99, SOCK_STREAM, 0, sv), -EAFNOSUPPORT, "socketpair of family 99");
    check_eq(socketpair(AF_UNIX, SOCK_DGRAM, 0, sv), -ESOCKTNOSUPPORT, "a local SOCK_DGRAM pair");
    check_eq(socketpair(AF_UNIX, SOCK_STREAM, 7, sv), -EPROTONOSUPPORT, "protocol 7");
    check_eq(socketpair(AF_UNIX, SOCK_STREAM, 0, (int *)0x10), -EFAULT, "a bad pointer");
}

static void name_checks(void) {
    unlink(PATH);
    unlink(OTHER);
    check_eq(socket(AF_UNIX, SOCK_DGRAM, 0), -ESOCKTNOSUPPORT, "a local SOCK_DGRAM socket");
    int l = socket(AF_UNIX, SOCK_STREAM, 0);
    check(l >= 0, "socket(AF_UNIX)");
    char buf[64];
    check_eq(read(l, buf, sizeof buf), -EINVAL, "read on a socket not connected");
    check_eq(write(l, "x", 1), -ENOTCONN, "write on one");
    struct sockaddr_un a;
    struct sockaddr_un got;
    check_eq(getpeername(l, &got), -ENOTCONN, "getpeername: not connected");
    check_eq(listen(l, 1), -EINVAL, "listen before bind");
    check_eq(accept(l, 0), -EINVAL, "accept on a socket not listening");

    set_name(&a, PATH);
    check_eq(bind(l, &a), 0, "bind");
    check_eq(bind(l, &a), -EINVAL, "bind twice");
    struct stat st;
    check(stat(PATH, &st) == 0 && st.type == T_SOCKET && st.size == 0 && st.nlink == 1, "stat: a socket inode");
    check_eq(open(PATH, O_RDONLY), -ENXIO, "open a socket's name");
    check_eq(getsockname(l, &got), 0, "getsockname");
    check(got.sun_family == AF_UNIX && strcmp(got.sun_path, PATH) == 0, "its name");
    int dup_name = socket(AF_UNIX, SOCK_STREAM, 0);
    check_eq(bind(dup_name, &a), -EADDRINUSE, "a name taken");
    set_name(&a, "/README");
    check_eq(bind(dup_name, &a), -EADDRINUSE, "a file's name");
    a.sun_family = AF_INET;
    check_eq(bind(dup_name, &a), -EINVAL, "not AF_UNIX");
    memset(a.sun_path, 'a', sizeof a.sun_path);
    a.sun_family = AF_UNIX;
    check_eq(bind(dup_name, &a), -EINVAL, "a path with no NUL");
    a.sun_path[0] = 0;
    check_eq(bind(dup_name, &a), -EINVAL, "an empty path");
    check_eq(bind(dup_name, (void *)0x10), -EFAULT, "a bad pointer");

    /* A name nobody listens on, a file, nothing at all. */
    set_name(&a, OTHER);
    check_eq(bind(dup_name, &a), 0, "bind a second name");
    int c = socket(AF_UNIX, SOCK_STREAM, 0);
    check_eq(connect(c, &a), -ECONNREFUSED, "connect to a name nobody listens on");
    set_name(&a, "/README");
    check_eq(connect(c, &a), -ECONNREFUSED, "connect to a file");
    set_name(&a, "/nope.sock");
    check_eq(connect(c, &a), -ENOENT, "connect to nothing");
    close(dup_name);

    set_name(&a, PATH);
    check_eq(listen(l, 2), 0, "listen");
    check_eq(connect(c, &a), 0, "connect, before any accept");
    check_eq(connect(c, &a), -EISCONN, "connect twice");
    check_eq(getpeername(c, &got), 0, "the client's peer");
    check(strcmp(got.sun_path, PATH) == 0, "is the name");
    memset(&got, 0x55, sizeof got);
    int s = accept(l, &got);
    check(s >= 0, "accept");
    check(got.sun_family == AF_UNIX && got.sun_path[0] == 0, "an unnamed client");
    check_eq(getsockname(s, &got), 0, "getsockname of the accepted end");
    check(strcmp(got.sun_path, PATH) == 0, "is the listener's name");
    check_eq(write(c, "ping", 4), 4, "the client sends");
    memset(&got, 0x55, sizeof got);
    check_eq(recvfrom(s, buf, sizeof buf, &got), 4, "the server receives");
    check(got.sun_family == AF_UNIX && got.sun_path[0] == 0, "from an unnamed sender");
    check_eq(sendto(s, "pong", 4, &a), -EISCONN, "sendto with an address, connected");
    check_eq(sendto(l, "pong", 4, &a), -EOPNOTSUPP, "and not connected");
    check_eq(send(s, "pong", 4), 4, "the server answers");
    check_eq(read(c, buf, sizeof buf), 4, "the client reads it");

    /* SO_RCVTIMEO: recv gives up. */
    struct timeval tv;
    tv.tv_sec = 0;
    tv.tv_usec = 100000;
    check_eq(setsockopt(s, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv), 0, "SO_RCVTIMEO");
    uint before = uptime();
    check_eq(read(s, buf, sizeof buf), -EAGAIN, "recv times out");
    check(uptime() - before >= 1, "after a tick");
    check_eq(setsockopt(s, SOL_SOCKET, 99, &tv, sizeof tv), -ENOPROTOOPT, "another option");

    /* A connection never accepted when the listener closes: reset. */
    int c2 = socket(AF_UNIX, SOCK_STREAM, 0);
    check_eq(connect(c2, &a), 0, "a second connection");
    check_eq(close(l), 0, "close the listener");
    check_eq(read(c2, buf, sizeof buf), -ECONNRESET, "the one it never accepted is reset");
    int late = socket(AF_UNIX, SOCK_STREAM, 0);
    check_eq(connect(late, &a), -ECONNREFUSED, "its name stays, refused");
    close(late);
    check_eq(write(c, "still", 5), 5, "the accepted one goes on");
    close(c2);
    close(c);
    close(s);
    check_eq(unlink(PATH), 0, "unlink the name");
    check_eq(unlink(OTHER), 0, "and the other");
}

static void rights_checks(void) {
    int sv[2];
    socketpair(AF_UNIX, SOCK_STREAM, 0, sv);
    int p[2];
    pipe(p);
    char buf[64];
    struct got g;

    /* A pipe's read end, passed and used. */
    check_eq(send_fds(sv[0], "x", 1, &p[0], 1, 0), 1, "sendmsg with a descriptor");
    close(p[0]);
    check_eq(recv_fds(sv[1], buf, sizeof buf, CMSG_SPACE(16 * sizeof(int)), 0, &g), 1, "recvmsg");
    check_eq(g.nfds, 1, "one descriptor came");
    check_eq(g.flags, 0, "all of it");
    check_eq(g.controllen, CMSG_SPACE(sizeof(int)), "msg_controllen: one message's room");
    check_eq(write(p[1], "abc", 3), 3, "write the pipe");
    check_eq(read(g.fds[0], buf, sizeof buf), 3, "read it through the descriptor that came");
    close(g.fds[0]);

    /* Descriptors come with their bytes: a read stops before them, and
     * takes them with the first. */
    int q[2];
    pipe(q);
    send_fds(sv[0], "12", 2, 0, 0, 0);
    send_fds(sv[0], "ab", 2, &q[0], 1, 0);
    send_fds(sv[0], "cd", 2, &q[1], 1, 0);
    check_eq(recv_fds(sv[1], buf, sizeof buf, sizeof(union control), 0, &g), 2, "the bytes before them");
    check(memcmp(buf, "12", 2) == 0 && g.nfds == 0, "alone");
    check_eq(recv_fds(sv[1], buf, sizeof buf, sizeof(union control), 0, &g), 2, "the first message's bytes");
    check(memcmp(buf, "ab", 2) == 0 && g.nfds == 1, "with its descriptor");
    int qr = g.fds[0];
    check_eq(recv_fds(sv[1], buf, sizeof buf, sizeof(union control), 0, &g), 2, "the second's");
    check(memcmp(buf, "cd", 2) == 0 && g.nfds == 1, "with its own");
    int qw = g.fds[0];
    check_eq(write(qw, "q", 1), 1, "through the write end that came");
    check_eq(read(qr, buf, 1), 1, "to the read end that came");
    close(qr);
    close(qw);
    close(q[0]);
    close(q[1]);

    /* No room for them all: those that fit, MSG_CTRUNC, the rest closed. */
    int two[2];
    pipe(two);
    send_fds(sv[0], "t", 1, two, 2, 0);
    check_eq(recv_fds(sv[1], buf, sizeof buf, CMSG_LEN(sizeof(int)), 0, &g), 1, "a control buffer for one");
    check_eq(g.nfds, 1, "one came");
    check_eq(g.flags, MSG_CTRUNC, "MSG_CTRUNC");
    close(g.fds[0]);
    send_fds(sv[0], "t", 1, two, 2, 0);
    check_eq(recv_fds(sv[1], buf, sizeof buf, 0, 0, &g), 1, "no control buffer");
    check(g.nfds == 0 && g.flags == MSG_CTRUNC && g.controllen == 0, "none came, MSG_CTRUNC");

    /* read() has nowhere to put them: closed. The write end passed was the
     * last but this one's, closed now: the pipe's reader sees the end. */
    send_fds(sv[0], "r", 1, &two[1], 1, 0);
    close(two[1]);
    check_eq(read(sv[1], buf, sizeof buf), 1, "read() of a message with a descriptor");
    check_eq(read(two[0], buf, sizeof buf), 0, "the descriptor it dropped is closed");
    close(two[0]);

    /* MSG_CMSG_CLOEXEC. */
    send_fds(sv[0], "c", 1, &sv[0], 1, 0);
    check_eq(recv_fds(sv[1], buf, sizeof buf, sizeof(union control), MSG_CMSG_CLOEXEC, &g), 1,
             "MSG_CMSG_CLOEXEC");
    check(g.nfds == 1 && fcntl(g.fds[0], F_GETFD, 0) == FD_CLOEXEC, "the descriptor closes on exec");

    /* A socket passed: the copy is the same end. */
    check_eq(write(g.fds[0], "s", 1), 1, "write through the socket that came");
    check_eq(read(sv[1], buf, sizeof buf), 1, "it is the same end");
    close(g.fds[0]);

    /* A memfd passed, mapped through both descriptors. */
    int m = memfd_create("unixtest", 0);
    check_eq(ftruncate(m, 4096), 0, "a memfd of a page");
    send_fds(sv[0], "m", 1, &m, 1, 0);
    check_eq(recv_fds(sv[1], buf, sizeof buf, sizeof(union control), 0, &g), 1, "the memfd came");
    char *a = mmap(0, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, m, 0);
    char *b = mmap(0, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, g.fds[0], 0);
    check((long)a > 0 && (long)b > 0 && a != b, "two mappings");
    strcpy(a, "one object");
    check(strcmp(b, "one object") == 0, "of one object");
    munmap(a, 4096);
    munmap(b, 4096);
    close(m);
    close(g.fds[0]);

    /* The errors. */
    int bad = 15;
    check_eq(send_fds(sv[0], "e", 1, &bad, 1, 0), -EBADF, "a descriptor not open");
    check_eq(send_fds(sv[0], "", 0, &sv[0], 1, 0), -EINVAL, "descriptors with no bytes");
    int many[17];
    for (int i = 0; i < 17; i++) {
        many[i] = sv[0];
    }
    struct iovec v;
    v.iov_base = "e";
    v.iov_len = 1;
    union {
        struct cmsghdr h;
        char buf[CMSG_SPACE(17 * sizeof(int))];
    } big;
    struct msghdr msg;
    memset(&msg, 0, sizeof msg);
    msg.msg_iov = &v;
    msg.msg_iovlen = 1;
    msg.msg_control = big.buf;
    msg.msg_controllen = sizeof big.buf;
    struct cmsghdr *h = &big.h;
    h->cmsg_len = CMSG_LEN(17 * sizeof(int));
    h->cmsg_level = SOL_SOCKET;
    h->cmsg_type = SCM_RIGHTS;
    memcpy(CMSG_DATA(h), many, sizeof many);
    check_eq(sendmsg(sv[0], &msg, 0), -EINVAL, "17 descriptors");
    h->cmsg_len = CMSG_LEN(sizeof(int));
    h->cmsg_type = 2;
    msg.msg_controllen = CMSG_SPACE(sizeof(int));
    check_eq(sendmsg(sv[0], &msg, 0), -EINVAL, "SCM_CREDENTIALS");
    h->cmsg_type = SCM_RIGHTS;
    h->cmsg_len = CMSG_SPACE(sizeof(int)) + 8;
    check_eq(sendmsg(sv[0], &msg, 0), -EINVAL, "a message longer than its buffer");
    h->cmsg_len = CMSG_LEN(sizeof(int));
    h->cmsg_level = 99;
    check_eq(sendmsg(sv[0], &msg, 0), 1, "another level: ignored");
    check_eq(read(sv[1], buf, sizeof buf), 1, "its byte");
    check_eq(sendmsg(sv[0], &msg, 1), -EOPNOTSUPP, "MSG_OOB");
    msg.msg_iovlen = 17;
    check_eq(sendmsg(sv[0], &msg, 0), -EMSGSIZE, "17 buffers");
    check_eq(sendmsg(sv[0], (struct msghdr *)0x10, 0), -EFAULT, "a bad message");
    check_eq(recvmsg(sv[0], (struct msghdr *)0x10, 0), -EFAULT, "a bad message to fill");
    check_eq(sendmsg(0, &msg, 0), -ENOTSOCK, "not a socket");

    /* Several buffers each way. */
    struct iovec out[3];
    out[0].iov_base = "ab";
    out[0].iov_len = 2;
    out[1].iov_base = "";
    out[1].iov_len = 0;
    out[2].iov_base = "cdef";
    out[2].iov_len = 4;
    memset(&msg, 0, sizeof msg);
    msg.msg_iov = out;
    msg.msg_iovlen = 3;
    check_eq(sendmsg(sv[0], &msg, MSG_NOSIGNAL), 6, "sendmsg gathers");
    char x[3];
    char y[8];
    struct iovec in[2];
    in[0].iov_base = x;
    in[0].iov_len = sizeof x;
    in[1].iov_base = y;
    in[1].iov_len = sizeof y;
    msg.msg_iov = in;
    msg.msg_iovlen = 2;
    check_eq(recvmsg(sv[1], &msg, 0), 6, "recvmsg scatters");
    check(memcmp(x, "abc", 3) == 0 && memcmp(y, "def", 3) == 0, "in order");
    close(sv[0]);
    close(sv[1]);
}

static void inet_checks(void) {
    /* UDP: an address and buffers each way; a datagram cut short. */
    struct sockaddr_in a1;
    struct sockaddr_in a2;
    sockaddr_set(&a1, INADDR_LOOPBACK, 7201);
    sockaddr_set(&a2, INADDR_LOOPBACK, 7202);
    int u1 = socket(AF_INET, SOCK_DGRAM, 0);
    int u2 = socket(AF_INET, SOCK_DGRAM, 0);
    check(bind(u1, &a1) == 0 && bind(u2, &a2) == 0, "two UDP sockets");
    struct iovec out[2];
    out[0].iov_base = "ab";
    out[0].iov_len = 2;
    out[1].iov_base = "cde";
    out[1].iov_len = 3;
    struct msghdr msg;
    memset(&msg, 0, sizeof msg);
    msg.msg_name = &a2;
    msg.msg_namelen = sizeof a2;
    msg.msg_iov = out;
    msg.msg_iovlen = 2;
    check_eq(sendmsg(u1, &msg, 0), 5, "UDP sendmsg to an address");
    char x[3];
    char y[1];
    struct iovec in[2];
    in[0].iov_base = x;
    in[0].iov_len = sizeof x;
    in[1].iov_base = y;
    in[1].iov_len = sizeof y;
    struct sockaddr_in from;
    memset(&msg, 0, sizeof msg);
    msg.msg_name = &from;
    msg.msg_namelen = sizeof from;
    msg.msg_iov = in;
    msg.msg_iovlen = 2;
    check_eq(recvmsg(u2, &msg, 0), 4, "UDP recvmsg: what fits");
    check(memcmp(x, "abc", 3) == 0 && y[0] == 'd', "scattered");
    check_eq(msg.msg_flags, MSG_TRUNC, "MSG_TRUNC: the rest lost");
    check(msg.msg_namelen == sizeof from && from.sin_port == htons(7201), "from u1");
    int fd = 0;
    check_eq(send_fds(u1, "x", 1, &fd, 1, 0), -EINVAL, "no descriptors over UDP");
    check_eq(send_fds(u1, "x", 1, 0, 0, MSG_DONTWAIT), -EOPNOTSUPP, "nor MSG_DONTWAIT");
    close(u1);
    close(u2);

    /* TCP: a stream gathered and scattered. */
    struct sockaddr_in a;
    sockaddr_set(&a, INADDR_LOOPBACK, 7203);
    int l = socket(AF_INET, SOCK_STREAM, 0);
    check(bind(l, &a) == 0 && listen(l, 1) == 0, "a TCP listener");
    int c = socket(AF_INET, SOCK_STREAM, 0);
    check_eq(connect(c, &a), 0, "connect");
    int s = accept(l, 0);
    memset(&msg, 0, sizeof msg);
    msg.msg_iov = out;
    msg.msg_iovlen = 2;
    check_eq(sendmsg(c, &msg, 0), 5, "TCP sendmsg");
    char all[8];
    struct iovec one;
    one.iov_base = all;
    one.iov_len = sizeof all;
    memset(&msg, 0, sizeof msg);
    msg.msg_name = &from;
    msg.msg_namelen = sizeof from;
    msg.msg_iov = &one;
    msg.msg_iovlen = 1;
    long got = 0;
    while (got < 5) {
        long r = recvmsg(s, &msg, 0);
        if (r <= 0) {
            break;
        }
        got += r;
        one.iov_base = all + got;
        one.iov_len = sizeof all - got;
    }
    check(got == 5 && memcmp(all, "abcde", 5) == 0, "TCP recvmsg");
    check_eq(msg.msg_namelen, 0, "a stream's sender is not stored");
    close(c);
    close(s);
    close(l);
}

/* ---- a memfd shared by two processes ---- */

#define SERVER_TEXT "written by the server"
#define CLIENT_TEXT "written by the client"

static void share_checks(void) {
    unlink(SHARE);
    int l = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un a;
    set_name(&a, SHARE);
    check(bind(l, &a) == 0 && listen(l, 1) == 0, "a listener for the client");
    int pid = spawn("/bin/unixtest", (char *[]){"unixtest", "client", SHARE, NULL}, environ);
    check(pid > 0, "start the client");
    int s = accept(l, 0);
    check(s >= 0, "the client connected");
    int m = memfd_create("shared", 0);
    check_eq(ftruncate(m, 8192), 0, "two pages");
    char *p = mmap(0, 8192, PROT_READ | PROT_WRITE, MAP_SHARED, m, 0);
    check((long)p > 0, "the server maps it");
    strcpy(p, SERVER_TEXT);
    check_eq(send_fds(s, "m", 1, &m, 1, 0), 1, "send the client the memfd");
    close(m);
    char c;
    check_eq(read(s, &c, 1), 1, "the client is done");
    check(strcmp(p + 4096, CLIENT_TEXT) == 0, "the server sees what the client wrote");
    int status = -1;
    check_eq(wait(pid, &status), pid, "the client ends");
    check_eq(status, 0, "having seen what the server wrote");
    munmap(p, 8192);
    close(s);
    close(l);
    check_eq(unlink(SHARE), 0, "unlink its name");
}

static int client(const char *path) {
    int s = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un a;
    set_name(&a, path);
    if (connect(s, &a) != 0) {
        return 1;
    }
    char buf[4];
    struct got g;
    if (recv_fds(s, buf, sizeof buf, sizeof(union control), 0, &g) != 1 || g.nfds != 1) {
        return 1;
    }
    struct stat st;
    if (fstat(g.fds[0], &st) != 0 || st.size != 8192) {
        return 1;
    }
    char *p = mmap(0, 8192, PROT_READ | PROT_WRITE, MAP_SHARED, g.fds[0], 0);
    if ((long)p < 0) {
        return 1;
    }
    int ok = strcmp(p, SERVER_TEXT) == 0;
    strcpy(p + 4096, CLIENT_TEXT);
    write(s, "d", 1);
    return ok ? 0 : 1;
}

int main(int argc, char **argv) {
    if (argc == 3 && strcmp(argv[1], "client") == 0) {
        return client(argv[2]);
    }
    pair_checks();
    name_checks();
    rights_checks();
    inet_checks();
    share_checks();
    check(!is_open(3) && !is_open(4) && !is_open(5), "no descriptor left open");
    printf("unixtest: %d checks, %d failed\n", checks, failed);
    return failed != 0;
}
