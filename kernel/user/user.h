/* What user programs get: the system calls (usys.s, docs/camelot-syscalls.md)
 * and a small C library (lib/, described in lib/README.md). No libc:
 * programs are compiled freestanding, with only this header. */
#ifndef CAMELOT_USER_H
#define CAMELOT_USER_H

typedef unsigned char uchar;
typedef unsigned short ushort;
typedef unsigned int uint;
typedef unsigned long size_t;

#define NULL ((void *)0)

/* Error numbers: a system call that fails returns minus one of them. They
 * are kernel/include/syscall.h's: a test checks they agree. */
#define EPERM 1         /* link: not for a directory; kill: a kernel process, or the shell */
#define ENOENT 2        /* no such file or directory */
#define ESRCH 3         /* kill: no such process; thread_join: no such thread */
#define EINTR 4         /* a wait ended because the process was killed */
#define E2BIG 7         /* spawn, exec: argument list too long */
#define ENOEXEC 8       /* spawn, exec: the file is not a valid executable */
#define EBADF 9         /* not an open file descriptor, or not open for that */
#define ECHILD 10       /* wait: no such child */
#define EAGAIN 11       /* spawn, thread_create: every slot is taken; futex_wait: not the value; a socket's receive timeout ran out */
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
#define ENOTSOCK 88     /* a socket call on a descriptor that is not a socket */
#define EDESTADDRREQ 89 /* send: a UDP socket with no peer, and no address given */
#define EMSGSIZE 90     /* a datagram too big for one packet */
#define ENOPROTOOPT 92  /* setsockopt: an option sockets don't have */
#define EPROTONOSUPPORT 93 /* socket: a protocol the type doesn't have */
#define EOPNOTSUPP 95   /* listen, accept: not on a UDP socket */
#define EAFNOSUPPORT 97 /* an address family other than AF_INET */
#define EADDRINUSE 98   /* a port that is taken */
#define EADDRNOTAVAIL 99 /* bind: an address that is not this host's */
#define ENETUNREACH 101 /* no route: no address yet, or no gateway */
#define ECONNRESET 104  /* TCP: the peer reset the connection */
#define ENOBUFS 105     /* the network's buffers or sockets are all in use */
#define EISCONN 106     /* TCP: the socket is connected already */
#define ENOTCONN 107    /* TCP: the socket is not connected */
#define ETIMEDOUT 110   /* ping: no reply in time; TCP: the peer stopped answering */
#define ECONNREFUSED 111 /* TCP: nothing listens on that port */
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
#define T_SOCKET 5

struct stat {
    uint ino;     /* the inode; 0 for a pipe or the shell's console */
    ushort type;  /* T_DIR, T_FILE, T_DEVICE, T_PIPE or T_SOCKET */
    ushort nlink; /* directory entries naming it; a socket: 0 once its other end hung up */
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

/* What procs stores for each process: 32 bytes. */
struct procinfo {
    uint pid;
    uint ppid;  /* its parent's pid, 0 if it has none */
    uint state; /* PROC_RUNNABLE, PROC_RUNNING, PROC_ZOMBIE or PROC_SLEEPING */
    uint size;  /* the end of its heap: program, data and heap, from 0 */
    char name[16];
};

/* What harts stores for each hart: 16 bytes. */
struct hartinfo {
    uint hart;     /* its id, 0 to the number of harts - 1 */
    uint pid;      /* the process it runs (the caller, on the caller's hart), 0 while idle */
    uint ticks;    /* timer ticks it has taken */
    uint switches; /* processes it has switched to */
};

#define PROC_RUNNABLE 1 /* waiting for its turn */
#define PROC_RUNNING 2  /* on the CPU: the caller */
#define PROC_ZOMBIE 3   /* ended, not collected by its parent yet */
#define PROC_SLEEPING 4 /* waiting: for input, a child, the clock, a disk */

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
    uint rx_bad_proto;    /* an IPv4 protocol other than ICMP, UDP and TCP */
    uint rx_bad_icmp;     /* a truncated ICMP message or a wrong checksum */
    uint rx_bad_udp;      /* a truncated UDP datagram or a wrong length */
    uint rx_bad_udp_sum;  /* a wrong UDP checksum */
    uint rx_no_port;      /* a datagram for a port with no socket */
    uint rx_queue_full;   /* a datagram for a full socket */
    uint no_buffer;       /* a frame dropped or not sent for want of a buffer */
    uint arp_failed;      /* a packet whose next hop ARP could not find */
    uint tx_errors;       /* a frame the card refused or could not queue */
    uint rx_bad_tcp;      /* a truncated TCP segment or a bad header length */
    uint rx_bad_tcp_sum;  /* a wrong TCP checksum */
    uint tcp_no_socket;   /* a segment for no connection or listener */
    uint tcp_backlog;     /* a SYN dropped: backlog full, or no socket */
    uint tcp_out_of_order; /* a segment past the next byte expected */
    uint tcp_retransmits; /* segments sent again after a timeout */
    uint tcp_resets;      /* connections the peer reset */
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

/* Sockets (docs/camelot-syscalls.md, "Sockets"): IPv4 only, TCP
 * (SOCK_STREAM) and UDP (SOCK_DGRAM). Linux's values and layout. */
#define AF_INET 2
#define SOCK_STREAM 1
#define SOCK_DGRAM 2
#define IPPROTO_TCP 6
#define IPPROTO_UDP 17
#define SHUT_RD 0   /* shutdown: no more reading */
#define SHUT_WR 1   /* no more sending: TCP sends its FIN */
#define SHUT_RDWR 2 /* neither */
#define SOL_SOCKET 1   /* setsockopt: the socket's own options */
#define SO_RCVTIMEO 20 /* how long recv and accept wait (a struct timeval) */

/* A time: seconds and microseconds. */
struct timeval {
    int tv_sec;
    int tv_usec;
};

/* An address in network byte order (big-endian): use htonl and ntohl. */
struct in_addr {
    uint s_addr;
};

#define INADDR_ANY 0u                /* bind: every address of the host */
#define INADDR_NONE 0xffffffffu      /* inet_addr: not an address */
#define INADDR_LOOPBACK 0x7f000001u  /* 127.0.0.1, in host order */

/* An IPv4 address and port: 16 bytes, the port in network byte order too. */
struct sockaddr_in {
    ushort sin_family; /* AF_INET */
    ushort sin_port;
    struct in_addr sin_addr;
    uchar sin_zero[8];
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
int kill(int pid);
int procs(struct procinfo *buf, uint max);
int harts(struct hartinfo *buf, uint max);
int netinfo(struct netinfo *info);
int netconf(uint addr, uint mask, uint gateway);
int ping(uint addr, uint seq, struct ping_reply *reply);
int socket(int domain, int type, int protocol);
int bind(int fd, const struct sockaddr_in *addr);
int listen(int fd, int backlog);
int accept(int fd, struct sockaddr_in *addr);
int connect(int fd, const struct sockaddr_in *addr);
int send(int fd, const void *buf, uint len);
int recv(int fd, void *buf, uint len);
int sendto(int fd, const void *buf, uint len, const struct sockaddr_in *to);
int recvfrom(int fd, void *buf, uint len, struct sockaddr_in *from);
int shutdown(int fd, int how);
int getsockname(int fd, struct sockaddr_in *addr);
int getpeername(int fd, struct sockaddr_in *addr);
int setsockopt(int fd, int level, int option, const void *value, uint len);
int thread_create(void (*entry)(void *), void *arg, void *stack);
void thread_exit(int status);
int thread_join(int tid, int *status);
int gettid(void);
int futex_wait(int *addr, int val);
int futex_wake(int *addr, int n);
/* Any call by number (for tests). */
int syscall(int number, uint a0, uint a1, uint a2);

/* ---- The C library (lib/, see lib/README.md) ---- */

/* Strings and memory (lib/string.c). */
size_t strlen(const char *s);
int strcmp(const char *a, const char *b);
int strncmp(const char *a, const char *b, size_t n);
char *strcpy(char *dst, const char *src);
char *strcat(char *dst, const char *src);
char *strchr(const char *s, int c);
char *strrchr(const char *s, int c);
char *strstr(const char *haystack, const char *needle);
void *memset(void *dst, int c, size_t n);
void *memcpy(void *dst, const void *src, size_t n);
void *memmove(void *dst, const void *src, size_t n);
int memcmp(const void *a, const void *b, size_t n);

/* Numbers (lib/stdlib.c). atoi: a decimal number with an optional '-',
 * up to the first non-digit; overflow wraps around. strtol: an optional
 * sign, then digits in `base` (2 to 36; 0 means 10, or 16 after "0x", or 8
 * after "0"); *end (if `end` isn't NULL) is where it stopped, `s` itself
 * when there were no digits. A number out of range gives LONG_MAX, or
 * LONG_MIN with a '-', as in C (*end is still past all its digits). */
#define LONG_MAX 2147483647
#define LONG_MIN (-LONG_MAX - 1)
int atoi(const char *s);
long strtol(const char *s, char **end, int base);

/* The heap (lib/malloc.c), on top of sbrk: blocks aligned to 8 bytes,
 * a free list with neighbours merged. malloc(0) and a failure return NULL.
 * calloc zeroes; realloc keeps the old bytes (up to the smaller size), and
 * realloc(NULL, n) is malloc(n). */
void *malloc(size_t n);
void free(void *p);
void *calloc(size_t count, size_t size);
void *realloc(void *p, size_t n);

/* Threads and locks (lib/atomic.s, lib/thread.s, lib/mutex.c; lib/README.md,
 * "Threads").
 *
 * The atomics are each one indivisible step on a word, ordered against
 * every load and store around them, and return the word as it was:
 * atomic_swap stores v, atomic_add adds v, atomic_cas stores `desired` only
 * if the word was `expect`; atomic_load and atomic_store are a plain load
 * and store with that ordering. */
int atomic_swap(int *p, int v);
int atomic_add(int *p, int v);
int atomic_cas(int *p, int expect, int desired);
int atomic_load(int *p);
void atomic_store(int *p, int v);

/* A spinlock spins (on amoswap.w) until it is free: for short stretches
 * between threads running on different harts. A mutex sleeps in the
 * kernel (futex_wait) while another thread holds it, and costs no system
 * call when nobody else wants it. Both are free when all zero, so a static
 * one needs no setting up; mutex_trylock takes it only if it is free and
 * returns 1 if it did. Neither may be taken twice by one thread. */
struct spinlock {
    int locked;
};
void spin_lock(struct spinlock *l);
void spin_unlock(struct spinlock *l);

struct mutex {
    int state;
};
void mutex_lock(struct mutex *m);
int mutex_trylock(struct mutex *m);
void mutex_unlock(struct mutex *m);

/* Runs fn(arg) in a new thread of this process, on the `size` bytes of
 * stack at `stack` (which must stay until the thread has ended); fn's
 * return value is the thread's exit status, which thread_join collects.
 * Returns the new thread's tid, or a negative error number. */
int thread_start(int (*fn)(void *), void *arg, void *stack, uint size);

/* Formatted output (lib/printf.c): %d %u %x %p %s %c %%, with an optional
 * '-' (left align), '0' (zero padding) and width; an 'l' before d/u/x is
 * accepted and ignored (long is int's size). printf and fprintf return the
 * bytes written, or a negative error number; snprintf writes at most
 * `size` bytes into `buf`, the last one a NUL, and returns the length the
 * whole text has (longer than `size` - 1 if it was cut). */
int printf(const char *fmt, ...);
int fprintf(int fd, const char *fmt, ...);
int snprintf(char *buf, size_t size, const char *fmt, ...);
/* `s` and a newline on stdout. */
int puts(const char *s);

/* Line input (lib/getline.c).
 *
 * getline reads a line from stdin into `buf` (`size` bytes, at least 2)
 * with one read, NUL-terminated and without its newline. The console
 * returns a line per read: ended by Enter, or by Ctrl-D without a newline.
 * One longer than `size` - 1 bytes comes back in pieces; the console's
 * longest, 127 characters and the newline, fits in 129 bytes. Returns its
 * length, or -1 at the end of the input or on an error. Nothing is read
 * ahead, so what follows the line is left for the next reader (the shell
 * passes its input on to the programs it runs).
 *
 * A reader reads any descriptor (a file, a pipe) a block at a time and
 * hands out lines: reader_getline stores the next one in `line` (`size`
 * bytes, at least 2), with its newline if it has one, and a NUL. Returns
 * the bytes stored (not counting the NUL), 0 at the end of the input, or a
 * negative error number. A line longer than `size` - 1 bytes comes back in
 * pieces, only the last one with the newline. */
int getline(char *buf, int size);

#define READER_SIZE 512

struct reader {
    int fd;
    int pos; /* the next byte of buf to hand out */
    int len; /* bytes in buf */
    char buf[READER_SIZE];
};

void reader_init(struct reader *r, int fd);
int reader_getline(struct reader *r, char *line, int size);

/* What error number `err` (positive) means, e.g. "no such file or
 * directory" (lib/strerror.c). */
const char *strerror(int err);

/* Addresses and names (lib/inet.c, lib/resolve.c).
 *
 * htons, ntohs, htonl, ntohl convert between host and network byte order.
 * inet_aton reads "a.b.c.d" (four decimal numbers up to 255) into *a and
 * returns 1, or 0 if `s` is not one; inet_addr returns it, or INADDR_NONE.
 * inet_ntoa writes one as "a.b.c.d" in a buffer of its own, overwritten by
 * the next call. sockaddr_set fills in *sa with AF_INET and `addr` and
 * `port` given in host order.
 *
 * resolve finds the address of `host`: "a.b.c.d" as it is, any other name
 * by asking the DNS server the network gave (netinfo's `dns`: 10.0.2.3
 * behind Grail's user-mode gateway) for its A record, over UDP, three
 * times at most, a second each. Stores it in *addr (network byte order) and
 * returns 0; or -ENOENT (no such host: the server said so, or had no
 * address for it), -ETIMEDOUT (no answer), -ENETUNREACH (no DNS server),
 * -EINVAL (not a name), or a socket call's error. resolve_error says what
 * its error means ("unknown host" for -ENOENT, else strerror). */
ushort htons(ushort x);
ushort ntohs(ushort x);
uint htonl(uint x);
uint ntohl(uint x);
int inet_aton(const char *s, struct in_addr *a);
uint inet_addr(const char *s);
char *inet_ntoa(struct in_addr a);
void sockaddr_set(struct sockaddr_in *sa, uint addr, uint port);
int resolve(const char *host, struct in_addr *addr);
const char *resolve_error(int err);

/* fstat() on the file at `path`: opens it for reading, then closes it.
 * Returns 0 or the error (lib/stat.c). */
int stat(const char *path, struct stat *st);

/* Kills every process `pid` started that has not ended, and theirs, down
 * to the last generation, from one procs() snapshot: the youngest first,
 * so that none is left an orphan. Not `pid` itself (which may be the
 * caller: kill_descendants(getpid()) ends what a program started). Returns
 * how many it killed. One thread at a time (lib/proc.c). */
int kill_descendants(int pid);

/* Busy-waits for about `n` iterations of a short loop, a few instructions
 * each: CPU time, as opposed to sleep() (lib/spin.c). */
void spin(uint n);

#endif
