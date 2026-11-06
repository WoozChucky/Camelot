/* polltest: checks poll() (docs/camelot-syscalls.md, "Polling";
 * kernel/include/poll.h) on every kind of file, its timeouts and its
 * errors, from user space. On a root image, which has the device nodes and
 * a README.
 *
 *   polltest                the checks: "polltest: N checks, F failed"
 *   polltest stress R       two pairs of threads, each pair passing a byte
 *                           back and forth R times, one pair through two
 *                           pipes and the other through a local socket
 *                           pair, each thread waiting for it in poll() on
 *                           its own end and on a pipe nothing is written
 *                           to. A thread
 *                           writes and polls at once, while the other one,
 *                           on another hart, may already be answering: a
 *                           wakeup lost would stop its pair for good.
 *                           "polltest: stress R rounds, F failed"
 *   polltest console        prints "polltest: ready", waits in poll() for
 *                           the console to have a line, then prints
 *                           "polltest: revents R" and the line read
 *   polltest keys           prints "polltest: ready", waits in poll() for
 *                           the keyboard (/dev/input/event0), then prints
 *                           "polltest: revents R" and the first event,
 *                           "type code value"
 *   polltest forever        prints "polltest: ready" and waits in poll() on
 *                           an empty pipe with no timeout: for a kill */
#include "user.h"

static int checks;
static int failed;

static void check(int ok, const char *what) {
    checks++;
    if (!ok) {
        failed++;
        printf("polltest: FAILED: %s\n", what);
    }
}

static void check_eq(long got, long want, const char *what) {
    checks++;
    if (got != want) {
        failed++;
        printf("polltest: FAILED: %s: got %ld, want %ld\n", what, got, want);
    }
}

#define IN (POLLIN | POLLRDNORM)
#define OUT (POLLOUT | POLLWRNORM)
#define ALL (POLLIN | POLLRDNORM | POLLOUT | POLLWRNORM | POLLRDHUP)

/* poll() on one descriptor for `events`, at once: its revents, or the
 * call's error. */
static long poll1(int fd, int events, int timeout) {
    struct pollfd p;
    p.fd = fd;
    p.events = (short)events;
    p.revents = 0x7fff;
    int r = poll(&p, 1, timeout);
    if (r < 0) {
        return r;
    }
    if (r == 0 && p.revents != 0) {
        return -1000; /* no event counted, yet revents set */
    }
    return p.revents;
}

static void error_checks(void) {
    struct pollfd fds[65];
    check_eq(poll(0, 0, 0), 0, "no descriptors, no wait");
    check_eq(poll(fds, 65, 0), -EINVAL, "more than 64 descriptors");
    check_eq(poll((struct pollfd *)0x10, 1, 0), -EFAULT, "a bad pointer");
    check_eq(poll((struct pollfd *)0x80000000ul, 1, 0), -EFAULT, "a kernel pointer");
    /* A descriptor not open: POLLNVAL, whatever was asked; a negative one
     * is skipped. */
    fds[0].fd = 15;
    fds[0].events = 0;
    fds[1].fd = -1;
    fds[1].events = POLLIN;
    fds[1].revents = 0x55;
    check_eq(poll(fds, 2, 0), 1, "one descriptor with an event");
    check_eq(fds[0].revents, POLLNVAL, "POLLNVAL for a descriptor not open");
    check_eq(fds[1].revents, 0, "a negative descriptor skipped, its revents 0");
}

static void timeout_checks(void) {
    uint t0 = uptime();
    check_eq(poll(0, 0, 300), 0, "poll(NULL, 0, 300) times out");
    uint slept = uptime() - t0;
    check(slept >= 3 && slept <= 4, "300 ms: 3 ticks");
    int p[2];
    pipe(p);
    t0 = uptime();
    check_eq(poll1(p[0], POLLIN, 150), 0, "an empty pipe for 150 ms");
    slept = uptime() - t0;
    check(slept >= 2 && slept <= 3, "150 ms: 2 ticks");
    close(p[0]);
    close(p[1]);
}

static void pipe_checks(void) {
    int p[2];
    check_eq(pipe(p), 0, "pipe");
    check_eq(poll1(p[0], POLLIN, 0), 0, "an empty pipe: nothing to read");
    check_eq(poll1(p[1], ALL, 0), OUT, "its write end: room");
    check_eq(poll1(p[0], ALL, 0), 0, "the read end can't be written");
    write(p[1], "x", 1);
    check_eq(poll1(p[0], ALL, 0), IN, "a byte: ready to read");
    check_eq(poll1(p[0], POLLOUT, 0), 0, "only what was asked");
    char buf[512];
    memset(buf, 'y', sizeof buf);
    check_eq(write(p[1], buf, 511), 511, "fill the pipe");
    check_eq(poll1(p[1], POLLOUT, 0), 0, "full: no room");
    read(p[0], buf, sizeof buf);
    check_eq(poll1(p[1], OUT, 0), OUT, "room again");
    write(p[1], "z", 1);
    close(p[1]);
    check_eq(poll1(p[0], IN, 0), IN | POLLHUP, "a byte, and no writer: POLLHUP too");
    read(p[0], buf, 1);
    check_eq(poll1(p[0], POLLIN, 0), POLLHUP, "empty and no writer: POLLHUP, asked or not");
    close(p[0]);

    check_eq(pipe(p), 0, "pipe");
    close(p[0]);
    check_eq(poll1(p[1], POLLOUT, 0), POLLOUT | POLLERR, "no reader: POLLERR, and only POLLOUT of what was asked");
    close(p[1]);
}

static void file_checks(void) {
    int fd = open("/README", O_RDONLY);
    check(fd >= 0, "open /README");
    check_eq(poll1(fd, ALL, 0), IN | OUT, "a file: always ready");
    close(fd);
    fd = memfd_create("polltest", 0);
    check(fd >= 0, "memfd_create");
    check_eq(poll1(fd, ALL, 0), IN | OUT, "a shared-memory object: always ready");
    close(fd);
    check_eq(poll1(1, ALL, 0), OUT, "the console's output: ready to write");
}

static void tcp_checks(void) {
    struct sockaddr_in a;
    sockaddr_set(&a, INADDR_LOOPBACK, 7100);
    int l = socket(AF_INET, SOCK_STREAM, 0);
    check(l >= 0 && bind(l, &a) == 0 && listen(l, 2) == 0, "a TCP listener");
    int fresh = socket(AF_INET, SOCK_STREAM, 0);
    check_eq(poll1(fresh, ALL, 0), OUT | POLLHUP, "a socket not connected: Linux's POLLOUT | POLLHUP");
    check_eq(poll1(l, ALL, 0), 0, "a listener with nobody to accept");
    check_eq(connect(fresh, &a), 0, "connect");
    int c = fresh;
    check_eq(poll1(l, IN, 1000), IN, "a connection to accept");
    int s = accept(l, 0);
    check(s >= 0, "accept");
    check_eq(poll1(s, ALL, 0), OUT, "connected: room to send, nothing to read");
    check_eq(send(c, "ping", 4), 4, "send");
    check_eq(poll1(s, IN, 1000), IN, "the bytes came");
    char buf[8];
    check_eq(recv(s, buf, sizeof buf), 4, "recv");
    check_eq(shutdown(c, SHUT_WR), 0, "the client done sending");
    check_eq(poll1(s, IN | POLLRDHUP, 1000), IN | POLLRDHUP, "its FIN: the end to read");
    check_eq(shutdown(s, SHUT_WR), 0, "the server done too");
    check_eq(poll1(c, IN, 1000), IN | POLLHUP, "both done: hung up");
    close(c);
    close(s);
    close(l);
}

static void udp_checks(void) {
    struct sockaddr_in a;
    sockaddr_set(&a, INADDR_LOOPBACK, 7101);
    int u = socket(AF_INET, SOCK_DGRAM, 0);
    check(u >= 0 && bind(u, &a) == 0, "a UDP socket");
    check_eq(poll1(u, ALL, 0), OUT, "UDP: always room, nothing to read");
    check_eq(sendto(u, "hi", 2, &a), 2, "a datagram to itself");
    check_eq(poll1(u, IN, 1000), IN, "it came");
    close(u);
}

static void unix_checks(void) {
    int sv[2];
    check_eq(socketpair(AF_UNIX, SOCK_STREAM, 0, sv), 0, "a local socket pair");
    check_eq(poll1(sv[0], ALL, 0), OUT, "connected: room to send, nothing to read");
    check_eq(write(sv[0], "hi", 2), 2, "a write");
    check_eq(poll1(sv[1], ALL, 0), IN | OUT, "the bytes came");
    char buf[256];
    check_eq(read(sv[1], buf, sizeof buf), 2, "read them");
    /* A full ring: no room to send until the peer reads. */
    memset(buf, 'f', sizeof buf);
    for (int i = 0; i < 8; i++) {
        write(sv[0], buf, sizeof buf);
    }
    check_eq(poll1(sv[0], ALL, 0), 0, "the peer's ring full: no room");
    check_eq(read(sv[1], buf, sizeof buf), sizeof buf, "the peer reads some");
    check_eq(poll1(sv[0], OUT, 0), OUT, "room again");
    check_eq(shutdown(sv[0], SHUT_WR), 0, "shutdown SHUT_WR");
    check_eq(poll1(sv[1], POLLRDHUP, 0), POLLRDHUP, "the peer done sending");
    close(sv[0]);
    check_eq(poll1(sv[1], OUT | POLLRDHUP, 0), OUT | POLLRDHUP | POLLHUP,
             "the peer gone: hung up, and a write fails at once");
    close(sv[1]);

    int l = socket(AF_UNIX, SOCK_STREAM, 0);
    check_eq(poll1(l, ALL, 0), OUT | POLLHUP, "not connected: Linux's POLLOUT | POLLHUP");
    struct sockaddr_un a;
    memset(&a, 0, sizeof a);
    a.sun_family = AF_UNIX;
    strcpy(a.sun_path, "/polltest.sock");
    unlink(a.sun_path);
    check(bind(l, &a) == 0 && listen(l, 1) == 0, "a local listener");
    check_eq(poll1(l, ALL, 0), 0, "a listener with nobody to accept");
    int c = socket(AF_UNIX, SOCK_STREAM, 0);
    check_eq(connect(c, &a), 0, "connect");
    check_eq(poll1(l, ALL, 0), IN, "a connection to accept");
    int s = accept(l, 0);
    check_eq(poll1(s, ALL, 0), OUT, "accepted");
    close(c);
    check_eq(poll1(s, ALL, 0), IN | OUT | POLLRDHUP | POLLHUP, "the client gone");
    close(s);
    close(l);
    unlink(a.sun_path);
}

static void device_checks(void) {
    int kbd = open("/dev/input/event0", O_RDONLY);
    check(kbd >= 0, "open the keyboard");
    check_eq(poll1(kbd, ALL, 0), 0, "no key: nothing to read, and never writable");
    close(kbd);

    int fb = open("/dev/fb0", O_RDWR);
    check(fb >= 0, "open /dev/fb0");
    check_eq(poll1(fb, ALL, 0), OUT, "no flip pending: ready to draw");
    struct fb_var_screeninfo v;
    memset(&v, 0, sizeof v);
    v.xres = 640;
    v.yres = 480;
    v.xres_virtual = 640;
    v.yres_virtual = 960;
    v.bits_per_pixel = 32;
    check_eq(ioctl(fb, FBIOPUT_VSCREENINFO, &v), 0, "640x480, two frames");
    v.yoffset = 480;
    v.activate = 0;
    check_eq(ioctl(fb, FBIOPAN_DISPLAY, &v), 0, "a flip, not waited for");
    check_eq(poll1(fb, POLLOUT, 0), 0, "pending: not ready");
    check_eq(poll1(fb, OUT, 1000), OUT, "shown at the vertical blank: ready again");
    v.yoffset = 0;
    check_eq(ioctl(fb, FBIOPAN_DISPLAY, &v), 0, "and back");
    check_eq(poll1(fb, OUT, 1000), OUT, "shown");
    close(fb);
}

/* A thread that writes a byte to the pipe `arg` after two ticks. */
static int late_writer(void *arg) {
    sleep(2);
    write(*(int *)arg, "w", 1);
    return 0;
}

static char writer_stack[2048];

static void wake_checks(void) {
    int p[2];
    pipe(p);
    int t = thread_start(late_writer, &p[1], writer_stack, sizeof writer_stack);
    check(t > 0, "a writer thread");
    struct pollfd fds[2];
    int q[2];
    pipe(q);
    fds[0].fd = q[0];
    fds[0].events = POLLIN;
    fds[1].fd = p[0];
    fds[1].events = IN;
    check_eq(poll(fds, 2, -1), 1, "woken by the other thread's write");
    check_eq(fds[0].revents, 0, "not by the quiet pipe");
    check_eq(fds[1].revents, IN, "by the pipe written to");
    int status;
    check_eq(thread_join(t, &status), 0, "join the writer");
    close(p[0]);
    close(p[1]);
    close(q[0]);
    close(q[1]);
}

static void kill_checks(void) {
    char *argv[] = {"polltest", "forever", 0};
    int pid = spawn("/bin/polltest", argv, 0);
    check(pid > 0, "spawn a poller");
    sleep(3);
    check_eq(kill(pid), 0, "kill it while it waits");
    int status = 0;
    check_eq(wait(pid, &status), pid, "it ends");
}

/* ---- stress ---- */

/* Threads 0 and 1 are a pair, 2 and 3 the other: thread i waits on and
 * reads from[i], and writes to[i], which its partner reads. The first pair
 * through two pipes, the second through the two ends of a local socket
 * pair. */
#define RING 4

static int from[RING];
static int to[RING];
static int quiet[2];
static int rounds;
static int stress_failed;
static char stacks[RING][2048];

/* The quiet pipe, as many times as this: the scan after a thread's own
 * pipe is long enough that its partner's answer comes in the middle of
 * it, which the scan must not miss. */
#define QUIET 40

static int passer(void *arg) {
    int i = (int)(long)arg;
    char c;
    for (int r = 0; r < rounds; r++) {
        struct pollfd fds[1 + QUIET];
        fds[0].fd = from[i];
        fds[0].events = IN;
        for (int q = 1; q <= QUIET; q++) {
            fds[q].fd = quiet[0];
            fds[q].events = POLLIN;
        }
        int bad = poll(fds, 1 + QUIET, -1) != 1 || fds[0].revents != IN;
        for (int q = 1; q <= QUIET; q++) {
            bad |= fds[q].revents != 0;
        }
        if (bad) {
            atomic_add(&stress_failed, 1);
        }
        if (read(from[i], &c, 1) != 1) {
            atomic_add(&stress_failed, 1);
        }
        write(to[i], &c, 1);
    }
    return 0;
}

static int stress(int n) {
    rounds = n;
    pipe(quiet);
    int p0[2];
    int p1[2];
    pipe(p0);
    pipe(p1);
    from[0] = p0[0];
    to[1] = p0[1];
    from[1] = p1[0];
    to[0] = p1[1];
    int sv[2];
    socketpair(AF_UNIX, SOCK_STREAM, 0, sv);
    from[2] = to[2] = sv[0];
    from[3] = to[3] = sv[1];
    int tids[RING];
    for (int i = 0; i < RING; i++) {
        tids[i] = thread_start(passer, (void *)(long)i, stacks[i], sizeof stacks[i]);
    }
    /* A byte for each pair: each thread takes R and passes R on, and
     * every byte it waits for comes. */
    write(to[1], "t", 1);
    write(to[3], "t", 1);
    for (int i = 0; i < RING; i++) {
        int status;
        thread_join(tids[i], &status);
    }
    printf("polltest: stress %d rounds, %d failed\n", n, stress_failed);
    return stress_failed != 0;
}

/* ---- interactive modes ---- */

static int wait_console(void) {
    printf("polltest: ready\n");
    struct pollfd p;
    p.fd = 0;
    p.events = POLLIN;
    int r = poll(&p, 1, -1);
    char line[64];
    int n = read(0, line, sizeof line - 1);
    line[n > 0 ? n : 0] = 0;
    printf("polltest: revents %d (%d)\n", p.revents, r);
    printf("polltest: line %s", line);
    return 0;
}

static int wait_keys(void) {
    int fd = open("/dev/input/event0", O_RDONLY);
    if (fd < 0) {
        printf("polltest: open: %d\n", fd);
        return 1;
    }
    printf("polltest: ready\n");
    struct pollfd p;
    p.fd = fd;
    p.events = POLLIN;
    int r = poll(&p, 1, -1);
    struct input_event ev;
    read(fd, &ev, sizeof ev);
    printf("polltest: revents %d (%d)\n", p.revents, r);
    printf("%d %d %d\n", ev.type, ev.code, ev.value);
    return 0;
}

static int forever(void) {
    int p[2];
    pipe(p);
    printf("polltest: ready\n");
    poll1(p[0], POLLIN, -1);
    printf("polltest: woke\n");
    return 1;
}

int main(int argc, char **argv) {
    if (argc == 3 && strcmp(argv[1], "stress") == 0) {
        return stress(atoi(argv[2]));
    }
    if (argc == 2 && strcmp(argv[1], "console") == 0) {
        return wait_console();
    }
    if (argc == 2 && strcmp(argv[1], "keys") == 0) {
        return wait_keys();
    }
    if (argc == 2 && strcmp(argv[1], "forever") == 0) {
        return forever();
    }
    error_checks();
    timeout_checks();
    pipe_checks();
    file_checks();
    tcp_checks();
    udp_checks();
    unix_checks();
    device_checks();
    wake_checks();
    kill_checks();
    printf("polltest: %d checks, %d failed\n", checks, failed);
    return failed != 0;
}
