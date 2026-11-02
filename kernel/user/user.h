/* What user programs get: the system calls (usys.s, docs/camelot-syscalls.md)
 * and a tiny C library (ulib.c). No libc: programs are compiled freestanding,
 * with only this header. */
#ifndef CAMELOT_USER_H
#define CAMELOT_USER_H

typedef unsigned char uchar;
typedef unsigned short ushort;
typedef unsigned int uint;
typedef unsigned long size_t;

#define NULL ((void *)0)

/* Error numbers: a system call that fails returns minus one of them. They
 * are kernel/include/syscall.h's: a test checks they agree. */
#define EPERM 1         /* link: not for a directory */
#define ENOENT 2        /* no such file or directory */
#define E2BIG 7         /* spawn, exec: argument list too long */
#define ENOEXEC 8       /* spawn, exec: the file is not a valid executable */
#define EBADF 9         /* not an open file descriptor, or not open for that */
#define ECHILD 10       /* wait: no such child */
#define EAGAIN 11       /* spawn: every process slot is taken */
#define ENOMEM 12       /* out of memory, or the heap would reach the stack */
#define EACCES 13       /* spawn, exec: not a regular file */
#define EFAULT 14       /* a pointer argument is not valid user memory */
#define EEXIST 17       /* the name is taken */
#define ENODEV 19       /* open: a device node with no driver */
#define ENOTDIR 20      /* a name on the way is not a directory */
#define EISDIR 21       /* open: a directory, for writing */
#define EINVAL 22       /* an argument is out of range */
#define ENFILE 23       /* every slot of the kernel's open file table is taken */
#define EMFILE 24       /* every descriptor of the process is taken */
#define EFBIG 27        /* write: past the largest file */
#define ENOSPC 28       /* no free block or inode on the disk */
#define ESPIPE 29       /* lseek: not on a pipe or a device */
#define EROFS 30        /* the filesystem is read-only */
#define EPIPE 32        /* write: a pipe nobody reads any more */
#define ENAMETOOLONG 36 /* a name longer than DIRSIZ, or a path longer than MAXPATH */
#define ENOSYS 38       /* no such system call */
#define ENOTEMPTY 39    /* unlink: a directory with entries */
#define EMSGSIZE 90     /* a datagram too big for one packet */
#define EADDRINUSE 98   /* a UDP port that is taken */
#define ENETUNREACH 101 /* no route: no address yet, or no gateway */
#define ENOBUFS 105     /* the network's buffers or sockets are all in use */
#define ETIMEDOUT 110   /* ping: no reply in time */
#define EHOSTUNREACH 113 /* ARP got no answer from the address */

/* The standard file descriptors: the console's in the shell, and what a
 * child inherits from its parent. */
#define STDIN 0
#define STDOUT 1
#define STDERR 2

/* spawn's and exec's limits: arguments, argv[0] included, and the bytes
 * their strings take with their NULs. */
#define MAXARG 16
#define ARG_MAX 512

/* open's flags: one access mode, plus O_CREAT (make the file if it is
 * missing) and O_TRUNC (empty it, when opened for writing). */
#define O_RDONLY 0x000
#define O_WRONLY 0x001
#define O_RDWR 0x002
#define O_CREAT 0x040
#define O_TRUNC 0x200

/* lseek's `whence`. */
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2

/* The longest path, without its NUL, and the longest name in a directory. */
#define MAXPATH 127
#define DIRSIZ 14

/* What fstat stores. */
#define T_DIR 1
#define T_FILE 2
#define T_DEVICE 3
#define T_PIPE 4

struct stat {
    uint ino;     /* the inode; 0 for a pipe or the shell's console */
    ushort type;  /* T_DIR, T_FILE, T_DEVICE or T_PIPE */
    ushort nlink; /* directory entries naming it */
    uint size;    /* bytes; for a pipe, those waiting to be read */
    ushort major; /* a device's numbers */
    ushort minor;
};

/* What reading a directory returns: one of these per slot, inum 0 for a
 * free one. A name of DIRSIZ bytes has no NUL. */
struct dirent {
    ushort inum;
    char name[DIRSIZ];
};

/* The network (docs/camelot-net.md). Addresses are in host order:
 * 10.0.2.15 is 0x0a00020f. These are kernel/include/syscall.h's: keep them
 * in sync. */
struct net_stats {
    uint rx_frames;       /* frames taken from the card */
    uint rx_bytes;
    uint tx_frames;       /* frames handed to the card */
    uint tx_bytes;
    uint rx_card_dropped; /* lost by the card itself */
    uint rx_filtered;     /* not for this host */
    uint rx_bad_ether;    /* an EtherType other than IPv4 and ARP */
    uint rx_bad_arp;      /* a truncated or unsupported ARP packet */
    uint rx_bad_ip;       /* a bad IPv4 header */
    uint rx_bad_ip_sum;   /* a wrong IPv4 header checksum */
    uint rx_fragment;     /* an IPv4 fragment */
    uint rx_bad_proto;    /* an IPv4 protocol other than ICMP and UDP */
    uint rx_bad_icmp;     /* a truncated ICMP message or a wrong checksum */
    uint rx_bad_udp;      /* a truncated UDP datagram or a wrong length */
    uint rx_bad_udp_sum;  /* a wrong UDP checksum */
    uint rx_no_port;      /* a datagram for a port with no socket */
    uint rx_queue_full;   /* a datagram for a full socket */
    uint no_buffer;       /* a frame dropped or not sent for want of a buffer */
    uint arp_failed;      /* a packet whose next hop ARP could not find */
    uint tx_errors;       /* a frame the card refused or could not queue */
};

struct netinfo {
    uchar mac[6];
    ushort flags; /* NETINFO_ */
    uint addr;    /* 0 if none */
    uint mask;
    uint gateway; /* 0: none */
    uint dns;     /* 0: none */
    struct net_stats stats;
};

#define NETINFO_LINK 0x1 /* the card's link is up */
#define NETINFO_UP 0x2   /* the interface has an address */
#define NETINFO_DHCP 0x4 /* which DHCP gave it */

struct ping_reply {
    uint from;   /* who answered */
    uint ttl;    /* the reply's time to live */
    uint len;    /* its ICMP message's length */
    uint rtt_us; /* the round trip, in microseconds */
};

/* The system calls. */
void exit(int status);
int write(int fd, const void *buf, uint len);
int read(int fd, void *buf, uint len);
int getpid(void);
int yield(void);
int sleep(uint ticks);
int spawn(const char *path, char *const argv[]);
int wait(int pid, int *status);
int sbrk(int increment);
uint uptime(void);
int open(const char *path, int flags);
int close(int fd);
int lseek(int fd, int offset, int whence);
int fstat(int fd, struct stat *st);
int mkdir(const char *path);
int unlink(const char *path);
int link(const char *old, const char *new);
int chdir(const char *path);
int dup(int fd);
int pipe(int *fds);
int exec(const char *path, char *const argv[]);
int netinfo(struct netinfo *info);
int netconf(uint addr, uint mask, uint gateway);
int ping(uint addr, uint seq, struct ping_reply *reply);
/* Any call by number (for tests). */
int syscall(int number, uint a0, uint a1, uint a2);

/* Strings and memory. */
size_t strlen(const char *s);
int strcmp(const char *a, const char *b);
char *strchr(const char *s, int c);
void *memset(void *dst, int c, size_t n);
void *memcpy(void *dst, const void *src, size_t n);
void *memmove(void *dst, const void *src, size_t n);
int memcmp(const void *a, const void *b, size_t n);
/* A decimal number, with an optional '-'; stops at the first non-digit. */
int atoi(const char *s);

/* Formatted output: %d %u %x %p %s %c %%, with an optional '-' (left
 * align), '0' (zero padding) and width, like the kernel's kprintf. Returns
 * the bytes written, or a negative error number. */
int printf(const char *fmt, ...);
int fprintf(int fd, const char *fmt, ...);
/* `s` and a newline on stdout. */
int puts(const char *s);

/* Reads a line from stdin into `buf` (`size` bytes, at least 2) with one
 * read, NUL-terminated and without its newline. The console returns a line
 * per read: ended by Enter, or by Ctrl-D without a newline. One longer than
 * `size` - 1 bytes comes back in pieces; the console's longest, 127
 * characters and the newline, fits in 129 bytes. Returns its length, or -1
 * at the end of the input or on an error. */
int getline(char *buf, int size);

/* What error number `err` (positive) means, e.g. "no such file or
 * directory". */
const char *strerror(int err);

/* fstat() on the file at `path`: opens it for reading, then closes it.
 * Returns 0 or the error. */
int stat(const char *path, struct stat *st);

/* Busy-waits for about `n` iterations of a short loop, a few instructions
 * each: CPU time, as opposed to sleep(). */
void spin(uint n);

#endif
