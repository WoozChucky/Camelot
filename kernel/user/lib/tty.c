/* Terminals (user.h): termios, the foreground, and pseudo-terminals, over
 * ioctl and fstat, as glibc's are over the same requests. */
#include "../user.h"

/* The device numbers of the console and of the ptys' slaves
 * (kernel/include/file.h). */
#define CONSOLE_MAJOR 1
#define PTS_MAJOR 5

int isatty(int fd) {
    struct termios t;
    return tcgetattr(fd, &t) == 0;
}

int tcgetattr(int fd, struct termios *t) {
    return ioctl(fd, TCGETS, t);
}

int tcsetattr(int fd, int when, const struct termios *t) {
    if (when == TCSANOW) {
        return ioctl(fd, TCSETS, t);
    }
    if (when == TCSADRAIN) {
        return ioctl(fd, TCSETSW, t);
    }
    if (when == TCSAFLUSH) {
        return ioctl(fd, TCSETSF, t);
    }
    return -EINVAL;
}

void cfmakeraw(struct termios *t) {
    t->c_iflag &= ~(uint)(IGNBRK | BRKINT | PARMRK | ISTRIP | INLCR | IGNCR | ICRNL | IXON);
    t->c_oflag &= ~(uint)OPOST;
    t->c_lflag &= ~(uint)(ECHO | ECHONL | ICANON | ISIG | IEXTEN);
    t->c_cflag &= ~(uint)(CSIZE | PARENB);
    t->c_cflag |= CS8;
    t->c_cc[VMIN] = 1;
    t->c_cc[VTIME] = 0;
}

int tcgetpgrp(int fd) {
    int pid;
    int r = ioctl(fd, TIOCGPGRP, &pid);
    return r < 0 ? r : pid;
}

int tcsetpgrp(int fd, int pid) {
    return ioctl(fd, TIOCSPGRP, &pid);
}

int posix_openpt(int flags) {
    return open("/dev/ptmx", flags);
}

int grantpt(int fd) {
    /* The slave's owner and mode are glibc's to set; Camelot has neither.
     * Only a master is accepted, as glibc checks. */
    uint n;
    return ioctl(fd, TIOCGPTN, &n);
}

int unlockpt(int fd) {
    int lock = 0;
    return ioctl(fd, TIOCSPTLCK, &lock);
}

/* ptsname's and ttyname's answer: "/dev/pts/" and a number fits. */
static char path[24];

char *ptsname(int fd) {
    uint n;
    if (ioctl(fd, TIOCGPTN, &n) < 0) {
        return NULL;
    }
    snprintf(path, sizeof path, "/dev/pts/%u", n);
    return path;
}

char *ttyname(int fd) {
    struct stat st;
    if (fstat(fd, &st) < 0 || st.type != T_DEVICE) {
        return NULL;
    }
    if (st.major == CONSOLE_MAJOR) {
        strcpy(path, "/dev/console");
        return path;
    }
    if (st.major == PTS_MAJOR) {
        snprintf(path, sizeof path, "/dev/pts/%u", (uint)st.minor);
        return path;
    }
    return NULL;
}
