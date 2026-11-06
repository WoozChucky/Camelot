/* ptytest: checks the terminals (docs/camelot-syscalls.md, "Terminals";
 * kernel/tty.c, kernel/pty.c): pseudo-terminal pairs, the line
 * discipline's canonical and raw modes, VMIN and VTIME, termios, the
 * window size, Ctrl-C and the foreground, hanging up, the C library's
 * functions; and the console's termios. Then it runs the shell on a slave
 * and types commands at it from the master. On a root image (/dev/ptmx and
 * /dev/pts).
 *
 *   ptytest    the checks: "ptytest: N checks, F failed" */
#include "user.h"

static int checks;
static int failed;

static void check(int ok, const char *what) {
    checks++;
    if (!ok) {
        failed++;
        printf("ptytest: FAILED: %s\n", what);
    }
}

static void check_eq(long got, long want, const char *what) {
    checks++;
    if (got != want) {
        failed++;
        printf("ptytest: FAILED: %s: got %ld, want %ld\n", what, got, want);
    }
}

/* What the master reads within `ticks` ticks, NUL-terminated, until it
 * has `want` (if not NULL) or the time is up: its length. */
static int expect(int m, char *buf, int size, const char *want, int ticks) {
    int len = 0;
    buf[0] = 0;
    long end = uptime() + ticks;
    while (len < size - 1 && (want == NULL || strstr(buf, want) == NULL)) {
        long left = end - uptime();
        if (left <= 0) {
            break;
        }
        struct pollfd p = {m, POLLIN, 0};
        if (poll(&p, 1, (int)left * 100) <= 0 || (p.revents & POLLIN) == 0) {
            break;
        }
        int n = read(m, buf + len, size - 1 - len);
        if (n <= 0) {
            break;
        }
        len += n;
        buf[len] = 0;
    }
    return len;
}

/* Whether `fd` would be ready for `events` at once. */
static int ready(int fd, int events) {
    struct pollfd p = {fd, (short)events, 0};
    return poll(&p, 1, 0) == 1 ? p.revents : 0;
}

/* A new pair: the master in *m, the slave, open, in *s. 0 or an error. */
static int open_pair(int *m, int *s) {
    *m = posix_openpt(O_RDWR);
    if (*m < 0) {
        return *m;
    }
    char *name = ptsname(*m);
    if (name == NULL || unlockpt(*m) != 0) {
        close(*m);
        return -EIO;
    }
    *s = open(name, O_RDWR);
    if (*s < 0) {
        close(*m);
        return *s;
    }
    return 0;
}

static void set_mode(int s, int canonical, int vmin, int vtime) {
    struct termios t;
    tcgetattr(s, &t);
    if (canonical) {
        t.c_lflag |= ICANON | ECHO | ISIG;
        t.c_iflag |= ICRNL;
        t.c_oflag |= OPOST;
    } else {
        cfmakeraw(&t);
    }
    t.c_cc[VMIN] = (unsigned char)vmin;
    t.c_cc[VTIME] = (unsigned char)vtime;
    tcsetattr(s, TCSANOW, &t);
}

static void pair_checks(void) {
    int m = posix_openpt(O_RDWR);
    check(m >= 0, "posix_openpt");
    check(isatty(m), "a master is a terminal");
    check_eq(grantpt(m), 0, "grantpt");
    char *name = ptsname(m);
    check(name != NULL && strncmp(name, "/dev/pts/", 9) == 0, "ptsname: /dev/pts/N");
    char path[24];
    strcpy(path, name != NULL ? name : "");
    uint n = 99;
    check_eq(ioctl(m, TIOCGPTN, &n), 0, "TIOCGPTN");
    check_eq(atoi(path + 9), n, "its N");
    check_eq(open(path, O_RDWR), -EIO, "a locked slave doesn't open");
    check_eq(unlockpt(m), 0, "unlockpt");
    int s = open(path, O_RDWR);
    check(s >= 0, "the slave opens");
    check(isatty(s), "the slave is a terminal");
    char *tty = ttyname(s);
    check(tty != NULL && strcmp(tty, path) == 0, "ttyname: its path");
    check_eq(grantpt(s), -ENOTTY, "grantpt on a slave");
    check_eq(ioctl(s, TIOCGPTN, &n), -ENOTTY, "TIOCGPTN on a slave");
    check(ttyname(m) == NULL, "a master has no ttyname");

    /* Linux's defaults. */
    struct termios t;
    check_eq(tcgetattr(s, &t), 0, "tcgetattr");
    check_eq(t.c_iflag, ICRNL | IXON, "c_iflag");
    check_eq(t.c_oflag, OPOST | ONLCR, "c_oflag");
    check_eq(t.c_cflag, B38400 | CS8 | CREAD | HUPCL, "c_cflag");
    check_eq(t.c_lflag, ISIG | ICANON | ECHO | ECHOE | ECHOK | ECHOCTL | ECHOKE | IEXTEN, "c_lflag");
    check(t.c_cc[VINTR] == 3 && t.c_cc[VERASE] == 0x7f && t.c_cc[VKILL] == 0x15 && t.c_cc[VEOF] == 4 &&
              t.c_cc[VMIN] == 1 && t.c_cc[VTIME] == 0 && t.c_cc[VWERASE] == 0x17,
          "c_cc");
    check_eq(tcsetattr(s, 7, &t), -EINVAL, "tcsetattr when 7");
    check_eq(ioctl(s, TCGETS, (void *)0x10), -EFAULT, "TCGETS to a bad address");
    check_eq(ioctl(s, 0x4600, 0), -ENOTTY, "a request terminals don't have");
    check_eq(ready(m, POLLIN | POLLOUT), POLLOUT, "the master: room to write");
    check_eq(ready(s, POLLIN | POLLOUT), POLLOUT, "the slave: room to write");

    /* The window size, one for the pair. */
    struct winsize w = {24, 80, 0, 0};
    check_eq(ioctl(m, TIOCSWINSZ, &w), 0, "TIOCSWINSZ on the master");
    struct winsize got;
    memset(&got, 0, sizeof got);
    check_eq(ioctl(s, TIOCGWINSZ, &got), 0, "TIOCGWINSZ on the slave");
    check(got.ws_row == 24 && got.ws_col == 80, "the same size");

    /* A second pair has another number. */
    int m2 = posix_openpt(O_RDWR);
    char *name2 = ptsname(m2);
    check(m2 >= 0 && name2 != NULL && strcmp(name2, path) != 0, "a second pair, another N");
    close(m2);
    close(s);
    close(m);
    check_eq(open(path, O_RDWR), -ENXIO, "its pair gone, the slave doesn't open");
}

static void canonical_checks(void) {
    int m;
    int s;
    check_eq(open_pair(&m, &s), 0, "a pair");
    char buf[256];
    /* A line: the slave reads it; the read echoes it, NL as CR NL. */
    check_eq(write(m, "hello\r", 6), 6, "the master types a line");
    check(ready(s, POLLIN) & POLLIN, "the slave can read it");
    check_eq(read(s, buf, sizeof buf), 6, "the slave reads it");
    check(memcmp(buf, "hello\n", 6) == 0, "CR made NL (ICRNL)");
    expect(m, buf, sizeof buf, NULL, 1);
    check(strcmp(buf, "hello\r\n") == 0, "the echo");

    /* Erasing: DEL and BS a character, Ctrl-W a word, Ctrl-U the line. */
    write(m, "ab\x7f" "c\x08" "d\n", 7);
    int n = read(s, buf, sizeof buf);
    check(n == 3 && memcmp(buf, "ad\n", 3) == 0, "DEL and BS erase");
    expect(m, buf, sizeof buf, NULL, 1);
    check(strcmp(buf, "ab\b \bc\b \bd\r\n") == 0, "their echo");
    write(m, "one two\x17three\n", 14);
    n = read(s, buf, sizeof buf);
    check(n == 10 && memcmp(buf, "one three\n", 10) == 0, "Ctrl-W erases a word");
    expect(m, buf, sizeof buf, NULL, 1);
    write(m, "xyz\x15ok\n", 7);
    n = read(s, buf, sizeof buf);
    check(n == 3 && memcmp(buf, "ok\n", 3) == 0, "Ctrl-U erases the line");
    expect(m, buf, sizeof buf, NULL, 1);
    check(strcmp(buf, "xyz\b \b\b \b\b \bok\r\n") == 0, "its echo");

    /* Control characters are dropped; CR LF is one line end. */
    write(m, "a\tb\x01" "c\r\n", 7);
    n = read(s, buf, sizeof buf);
    check(n == 4 && memcmp(buf, "abc\n", 4) == 0, "control characters dropped");
    expect(m, buf, sizeof buf, NULL, 1);
    check(!(ready(s, POLLIN) & POLLIN), "CR LF: one line end");

    /* Ctrl-D: the line without a newline; on an empty line, the end. */
    write(m, "ab\x04", 3);
    check_eq(read(s, buf, sizeof buf), 2, "Ctrl-D ends a line");
    write(m, "\x04", 1);
    check_eq(read(s, buf, sizeof buf), 0, "Ctrl-D on an empty line: 0");
    expect(m, buf, sizeof buf, NULL, 1);

    /* A long line: 127 characters kept. A read takes what it asks for. */
    for (int i = 0; i < 200; i++) {
        write(m, "x", 1);
    }
    write(m, "\n", 1);
    check_eq(read(s, buf, 100), 100, "a read takes 100");
    check_eq(read(s, buf, sizeof buf), 28, "the rest: 27 and the newline");
    while (expect(m, buf, sizeof buf, NULL, 1) > 0) {
    }

    /* Without ECHO nothing comes back. */
    struct termios t;
    tcgetattr(s, &t);
    t.c_lflag &= ~(uint)ECHO;
    tcsetattr(s, TCSANOW, &t);
    write(m, "secret\n", 7);
    check_eq(read(s, buf, sizeof buf), 7, "a line, not echoed");
    check(!(ready(m, POLLIN) & POLLIN), "no echo");

    /* The slave's output: ONLCR, then none with OPOST off. */
    check_eq(write(s, "a\nb", 3), 3, "the slave writes");
    expect(m, buf, sizeof buf, NULL, 1);
    check(strcmp(buf, "a\r\nb") == 0, "the master reads NL as CR NL");
    int count = -1;
    check_eq(ioctl(m, FIONREAD, &count), 0, "FIONREAD on the master");
    check_eq(count, 0, "nothing left");
    t.c_oflag &= ~(uint)OPOST;
    tcsetattr(s, TCSANOW, &t);
    write(s, "c\n", 2);
    expect(m, buf, sizeof buf, NULL, 1);
    check(strcmp(buf, "c\n") == 0, "without OPOST, as it is");

    /* FIONREAD: a whole line only, in canonical mode. */
    write(m, "par", 3);
    check_eq(ioctl(s, FIONREAD, &count), 0, "FIONREAD on the slave");
    check_eq(count, 0, "no line yet");
    write(m, "t\n", 2);
    ioctl(s, FIONREAD, &count);
    check_eq(count, 5, "a line of 5");
    check_eq(tcsetattr(s, TCSAFLUSH, &t), 0, "TCSAFLUSH");
    ioctl(s, FIONREAD, &count);
    check_eq(count, 0, "the input thrown away");
    close(s);
    close(m);
}

static void raw_checks(void) {
    int m;
    int s;
    check_eq(open_pair(&m, &s), 0, "a pair");
    char buf[64];
    set_mode(s, 0, 1, 0);
    write(m, "x\ry\x03", 4);
    check_eq(read(s, buf, sizeof buf), 4, "raw: every byte");
    check(memcmp(buf, "x\ry\x03", 4) == 0, "as they were typed");
    check(!(ready(m, POLLIN) & POLLIN), "no echo");

    /* VMIN 0, VTIME 0: what there is, maybe nothing. */
    set_mode(s, 0, 0, 0);
    check_eq(read(s, buf, sizeof buf), 0, "VMIN 0 VTIME 0: nothing, at once");
    /* VMIN 0, VTIME 3: 0 after 3 ticks. */
    set_mode(s, 0, 0, 3);
    long t0 = uptime();
    check_eq(read(s, buf, sizeof buf), 0, "VMIN 0 VTIME 3: 0");
    check(uptime() - t0 >= 3, "after 3 ticks");
    write(m, "q", 1);
    check_eq(read(s, buf, sizeof buf), 1, "or a byte that came");
    /* VMIN 3: three bytes, or the read waits. */
    set_mode(s, 0, 3, 0);
    write(m, "ab", 2);
    check(!(ready(s, POLLIN) & POLLIN), "VMIN 3: 2 bytes are not enough");
    write(m, "c", 1);
    check(ready(s, POLLIN) & POLLIN, "3 are");
    check_eq(read(s, buf, sizeof buf), 3, "VMIN 3: 3 bytes");
    check_eq(read(s, buf, 0), 0, "a read of nothing");
    /* VMIN 3, VTIME 2: after one byte, 2 ticks without another. */
    set_mode(s, 0, 3, 2);
    write(m, "z", 1);
    t0 = uptime();
    check_eq(read(s, buf, sizeof buf), 1, "VTIME between bytes: what came");
    check(uptime() - t0 >= 1, "once the time was up");
    /* A line not read when the mode changes is read raw. */
    set_mode(s, 1, 1, 0);
    write(m, "half", 4);
    set_mode(s, 0, 1, 0);
    check_eq(read(s, buf, sizeof buf), 4, "a line half typed, read raw");
    check(memcmp(buf, "half", 4) == 0, "its bytes");
    while (expect(m, buf, sizeof buf, NULL, 1) > 0) {
    }
    close(s);
    close(m);
}

static void interrupt_checks(void) {
    int m;
    int s;
    check_eq(open_pair(&m, &s), 0, "a pair");
    char buf[64];
    /* No foreground: Ctrl-C is dropped. */
    check_eq(tcgetpgrp(s), 0, "no foreground");
    write(m, "a\x03" "b\n", 4);
    int n = read(s, buf, sizeof buf);
    check(n == 3 && memcmp(buf, "ab\n", 3) == 0, "Ctrl-C dropped without a foreground");
    expect(m, buf, sizeof buf, NULL, 1);

    check_eq(tcsetpgrp(s, 0), -EINVAL, "tcsetpgrp 0");
    check_eq(tcsetpgrp(s, 99999), -ESRCH, "tcsetpgrp of no process");

    /* A program reading the slave, in the foreground: Ctrl-C kills it,
     * and throws away the line typed. */
    int in = dup(STDIN);
    close(STDIN);
    dup(s);
    char *argv[] = {"cat", 0};
    int pid = spawn("/bin/cat", argv, environ);
    close(STDIN);
    dup(in);
    close(in);
    check(pid > 0, "cat on the slave");
    check_eq(tcsetpgrp(m, pid), 0, "tcsetpgrp on the master");
    check_eq(tcgetpgrp(s), pid, "the slave's foreground");
    write(m, "par\x03", 4);
    int status = 0;
    check_eq(wait(pid, &status), pid, "cat ended");
    check_eq(status, -1, "killed");
    expect(m, buf, sizeof buf, NULL, 2);
    check(strcmp(buf, "^C\r\n") == 0, "^C echoed, the line gone");
    check_eq(ioctl(s, FIONREAD, &n), 0, "FIONREAD");
    check_eq(n, 0, "nothing left to read");
    close(s);
    close(m);
}

static void hangup_checks(void) {
    int m;
    int s;
    char buf[16];
    /* The slave closes: the master reads what was left, then -EIO. */
    check_eq(open_pair(&m, &s), 0, "a pair");
    write(s, "bye", 3);
    close(s);
    check(ready(m, POLLIN) & POLLHUP, "the master sees the hang-up");
    check_eq(read(m, buf, sizeof buf), 3, "what was left");
    check_eq(read(m, buf, sizeof buf), -EIO, "then -EIO");
    check_eq(write(m, "x", 1), -EIO, "and its writes");
    /* The slave opens again: the master works again. */
    s = open(ptsname(m), O_RDWR);
    check(s >= 0, "the slave opens again");
    check_eq(write(m, "ok\n", 3), 3, "the master writes again");
    check_eq(read(s, buf, sizeof buf), 3, "the slave reads it");
    expect(m, buf, sizeof buf, NULL, 1);
    /* The master closes: the slave reads 0, its writes fail. */
    close(m);
    check(ready(s, POLLIN) & POLLHUP, "the slave sees the hang-up");
    check_eq(read(s, buf, sizeof buf), 0, "the slave reads the end");
    check_eq(write(s, "x", 1), -EIO, "its writes fail");
    close(s);
}

static void console_checks(void) {
    /* The console keeps its old ways: no output processing, no Ctrl-U or
     * Ctrl-W, Ctrl-C dropped (no foreground). */
    struct termios t;
    check_eq(tcgetattr(STDIN, &t), 0, "the console's termios");
    check_eq(t.c_oflag, 0, "no output processing");
    check(t.c_cc[VKILL] == 0 && t.c_cc[VWERASE] == 0, "no VKILL or VWERASE");
    check((t.c_lflag & (ICANON | ECHO | ISIG)) == (ICANON | ECHO | ISIG), "canonical, echo, ISIG");
    check(isatty(STDOUT), "the console is a terminal");
    check_eq(tcgetpgrp(STDIN), 0, "no foreground");
    int p[2];
    pipe(p);
    check(!isatty(p[0]), "a pipe isn't");
    close(p[0]);
    close(p[1]);
}

/* The shell on a slave, typed at from the master. */
static void shell_checks(void) {
    int m;
    int s;
    check_eq(open_pair(&m, &s), 0, "a pair");
    int saved[3];
    for (int fd = 0; fd < 3; fd++) {
        saved[fd] = dup(fd);
    }
    for (int fd = 0; fd < 3; fd++) {
        close(fd);
        dup(s);
    }
    char *argv[] = {"sh", 0};
    int pid = spawn("/bin/sh", argv, environ);
    for (int fd = 0; fd < 3; fd++) {
        close(fd);
        dup(saved[fd]);
        close(saved[fd]);
    }
    close(s);
    check(pid > 0, "sh on the slave");
    char buf[512];
    expect(m, buf, sizeof buf, "$ ", 50);
    check(strstr(buf, "Type help") != NULL && strstr(buf, "$ ") != NULL, "its prompt");
    write(m, "echo hello  there\r", 18);
    expect(m, buf, sizeof buf, "$ ", 50);
    check(strcmp(buf, "echo hello  there\r\nhello there\r\n$ ") == 0, "echo: the echo, the output, a prompt");
    write(m, "ecg\x7fho edited\r", 15);
    expect(m, buf, sizeof buf, "$ ", 50);
    check(strcmp(buf, "ecg\b \bho edited\r\nedited\r\n$ ") == 0, "an edited line");
    write(m, "cat\r", 4);
    expect(m, buf, sizeof buf, "cat\r\n", 50);
    write(m, "one\rtwo\r\x04", 9);
    expect(m, buf, sizeof buf, "$ ", 50);
    check(strcmp(buf, "one\r\none\r\ntwo\r\ntwo\r\n$ ") == 0, "cat reads the slave until Ctrl-D");
    write(m, "exit 3\r", 7);
    int status = 0;
    check_eq(wait(pid, &status), pid, "sh ends");
    check_eq(status, 3, "with exit's status");
    close(m);
}

int main(void) {
    pair_checks();
    canonical_checks();
    raw_checks();
    interrupt_checks();
    hangup_checks();
    console_checks();
    shell_checks();
    check(!isatty(3) && !isatty(4) && !isatty(5), "no descriptor left open");
    printf("ptytest: %d checks, %d failed\n", checks, failed);
    return failed != 0;
}
