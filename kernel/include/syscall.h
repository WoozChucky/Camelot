/* The Camelot system call ABI (docs/camelot-syscalls.md), Linux RISC-V
 * style: `ecall` from U-mode with the call number in a7 and the arguments in
 * a0-a5; the result comes back in a0, a negative value being an error, minus
 * one of the E* numbers below. Every other register is preserved.
 *
 * kernel/user/usys.s (the stubs) and kernel/user/user.h (the error numbers)
 * have their own copies of these numbers, and the document a table of them;
 * a test checks that all four agree. */
#ifndef CAMELOT_SYSCALL_H
#define CAMELOT_SYSCALL_H

#include "trap.h"
#include "types.h"

/* Call numbers. */
#define SYS_exit 1    /* exit(status): never returns */
#define SYS_write 2   /* write(fd, buf, len): bytes written */
#define SYS_read 3    /* read(fd, buf, len): bytes read, 0 at the end of input */
#define SYS_getpid 4  /* getpid(): the caller's pid */
#define SYS_yield 5   /* yield(): 0, after the other runnable processes had a turn */
#define SYS_sleep 6   /* sleep(ticks): 0, at least `ticks` timer ticks later */
#define SYS_spawn 7   /* spawn(path, argv): the new process's pid */
#define SYS_wait 8    /* wait(pid, status): the pid of the child that ended */
#define SYS_sbrk 9    /* sbrk(increment): the old end of the heap */
#define SYS_uptime 10 /* uptime(): timer ticks since boot */
#define SYS_open 11   /* open(path, flags): a new file descriptor */
#define SYS_close 12  /* close(fd): 0 */
#define SYS_lseek 13  /* lseek(fd, offset, whence): the new offset */
#define SYS_fstat 14  /* fstat(fd, st): 0, with *st filled in */
#define SYS_mkdir 15  /* mkdir(path): 0 */
#define SYS_unlink 16 /* unlink(path): 0 */
#define SYS_link 17   /* link(old, new): 0 */
#define SYS_chdir 18  /* chdir(path): 0 */
#define SYS_dup 19    /* dup(fd): a new descriptor for the same open file */
#define SYS_pipe 20   /* pipe(fds): 0, with fds[0] the read end and fds[1] the write end */
#define SYS_exec 21   /* exec(path, argv): does not return */
#define SYS_kill 22   /* kill(pid): 0, the process ends before it runs U-mode code again */
#define SYS_procs 23  /* procs(buf, max): the number of processes, up to max described in buf */
#define SYS_netinfo 24 /* netinfo(info): 0, with *info filled in */
#define SYS_netconf 25 /* netconf(addr, mask, gateway): 0 */
#define SYS_ping 26    /* ping(addr, seq, reply): 0, with *reply filled in */
#define SYS_socket 27  /* socket(domain, type, protocol): a new socket's descriptor */
#define SYS_bind 28    /* bind(fd, addr): 0, the socket has its port */
#define SYS_listen 29  /* listen(fd, backlog): 0, the TCP socket takes connections */
#define SYS_accept 30  /* accept(fd, addr): a descriptor for the next connection */
#define SYS_connect 31 /* connect(fd, addr): 0, connected (TCP) or given its peer (UDP) */
#define SYS_send 32    /* send(fd, buf, len): bytes sent */
#define SYS_recv 33    /* recv(fd, buf, len): bytes received, 0 at the end */
#define SYS_sendto 34  /* sendto(fd, buf, len, addr): bytes sent, to addr */
#define SYS_recvfrom 35 /* recvfrom(fd, buf, len, addr): bytes received, with the sender in *addr */
#define SYS_shutdown 36 /* shutdown(fd, how): 0, no more reading or sending */
#define SYS_getsockname 37 /* getsockname(fd, addr): 0, this end's address in *addr */
#define SYS_getpeername 38 /* getpeername(fd, addr): 0, the other end's address in *addr */
#define SYS_setsockopt 39 /* setsockopt(fd, level, option, value, len): 0 */
#define SYS_harts 40   /* harts(buf, max): the number of harts, up to max described in buf */

/* Error numbers (Linux's values); a call returns minus one of them. */
#define EPERM 1         /* link: not for a directory; kill: not a kernel process */
#define ENOENT 2        /* no such file or directory */
#define ESRCH 3         /* kill: no such process */
#define EINTR 4         /* a wait ended because the process was killed */
#define E2BIG 7         /* spawn, exec: argument list too long */
#define ENOEXEC 8       /* spawn, exec: the file is not a valid executable */
#define EBADF 9         /* not an open file descriptor, or not open for that */
#define ECHILD 10       /* wait: no such child */
#define EAGAIN 11       /* spawn: every process slot is taken; a socket's receive timeout ran out */
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

/* The console's file descriptors, open in the first process (0 for
 * reading, 1 and 2 for writing) and passed on by spawn. */
#define FD_STDIN 0
#define FD_STDOUT 1
#define FD_STDERR 2

/* open's flags (Linux's values): one access mode, plus O_CREAT and
 * O_TRUNC. */
#define O_RDONLY 0x000
#define O_WRONLY 0x001
#define O_RDWR 0x002
#define O_ACCMODE 0x003
#define O_CREAT 0x040
#define O_TRUNC 0x200

/* lseek's `whence`. */
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2

/* The longest path a call takes, without its NUL. */
#define MAXPATH 127

/* What fstat stores: 16 bytes. `type` is the inode's (T_DIR, T_FILE or
 * T_DEVICE, fs.h), T_PIPE or T_SOCKET; `ino` and `nlink` are 0 for a pipe
 * and for the console the first process got without opening a path, and a
 * pipe's size is the bytes waiting in it. A socket's size is the bytes a
 * read would get now, and its nlink 1, or 0 once nothing more will arrive
 * (socket_hung_up(), net.h). */
struct stat {
    u32 ino;
    u16 type;
    u16 nlink;
    u32 size;
    u16 major;
    u16 minor;
};

#define T_PIPE 4
#define T_SOCKET 5

/* What procs stores for each process: 32 bytes. `state` is one of the
 * PROC_ values (proc.h), `ppid` 0 for a process without a parent, `size`
 * the end of its heap (its program, data and heap, from address 0), 0 for
 * a kernel process. */
struct procinfo {
    u32 pid;
    u32 ppid;
    u32 state;
    u32 size;
    char name[16];
};

/* What harts stores for each hart running the kernel: 16 bytes. `pid` is
 * the process running on it (the caller, on the caller's hart), 0 while the
 * hart idles; `ticks` the timer ticks it has taken and `switches` the
 * processes it has switched to, both since it started. */
struct hartinfo {
    u32 hart;
    u32 pid;
    u32 ticks;
    u32 switches;
};

/* The network's counters, in netinfo's `stats` (docs/camelot-net.md,
 * "Counters"): frames and bytes through the card, then everything
 * dropped, by reason. kernel/user/user.h has a copy: keep them in sync. */
struct net_stats {
    u32 rx_frames;       /* frames taken from the card */
    u32 rx_bytes;
    u32 tx_frames;       /* frames handed to the card */
    u32 tx_bytes;
    u32 rx_card_dropped; /* lost by the card itself (NET_RX_DROPPED) */
    u32 rx_filtered;     /* not for this host: another MAC or IPv4 address */
    u32 rx_bad_ether;    /* an EtherType other than IPv4 and ARP */
    u32 rx_bad_arp;      /* a truncated or unsupported ARP packet */
    u32 rx_bad_ip;       /* a bad IPv4 header: truncated, version, lengths */
    u32 rx_bad_ip_sum;   /* a wrong IPv4 header checksum */
    u32 rx_fragment;     /* an IPv4 fragment (not supported) */
    u32 rx_bad_proto;    /* an IPv4 protocol other than ICMP, UDP and TCP */
    u32 rx_bad_icmp;     /* a truncated ICMP message or a wrong checksum */
    u32 rx_bad_udp;      /* a truncated UDP datagram or a wrong length */
    u32 rx_bad_udp_sum;  /* a wrong UDP checksum */
    u32 rx_no_port;      /* a datagram for a port with no socket */
    u32 rx_queue_full;   /* a datagram for a socket holding UDP_QUEUE_MAX */
    u32 no_buffer;       /* a frame dropped, or not sent, for want of a buffer */
    u32 arp_failed;      /* a packet whose next hop ARP could not find */
    u32 tx_errors;       /* a frame the card refused, or the transmit queue was full */
    u32 rx_bad_tcp;      /* a truncated TCP segment or a bad header length */
    u32 rx_bad_tcp_sum;  /* a wrong TCP checksum */
    u32 tcp_no_socket;   /* a segment for no connection or listener (answered by a reset) */
    u32 tcp_backlog;     /* a SYN dropped: the listener's backlog was full, or no socket or memory */
    u32 tcp_out_of_order; /* a segment past the next byte expected, dropped */
    u32 tcp_retransmits; /* segments sent again after a timeout */
    u32 tcp_resets;      /* connections the peer reset */
};

/* What netinfo stores: the interface and its counters. */
struct netinfo {
    u8 mac[6];
    u16 flags;   /* NETINFO_ */
    u32 addr;    /* in host order, 0 if none */
    u32 mask;
    u32 gateway; /* 0: none */
    u32 dns;     /* 0: none */
    struct net_stats stats;
};

#define NETINFO_LINK 0x1 /* the card's link is up */
#define NETINFO_UP 0x2   /* the interface has an address */
#define NETINFO_DHCP 0x4 /* which DHCP gave it */

/* What ping stores about the echo reply. */
struct ping_reply {
    u32 from;   /* who answered */
    u32 ttl;    /* the reply's time to live */
    u32 len;    /* its ICMP message's length */
    u32 rtt_us; /* the round trip, in microseconds of mtime */
};

/* Sockets (docs/camelot-syscalls.md, "Sockets"): Linux's values. */
#define AF_INET 2     /* IPv4: the only address family */
#define SOCK_STREAM 1 /* TCP */
#define SOCK_DGRAM 2  /* UDP */
#define SHUT_RD 0     /* shutdown: no more reading */
#define SHUT_WR 1     /* no more sending */
#define SHUT_RDWR 2   /* neither */
#define SOL_SOCKET 1  /* setsockopt: the socket's own options */
#define SO_RCVTIMEO 20 /* how long recv and accept wait: a struct timeval */

/* A time: seconds and microseconds (Linux's 32-bit layout). */
struct timeval {
    i32 tv_sec;
    i32 tv_usec;
};

/* An IPv4 address and port, as Linux lays it out (16 bytes): the port and
 * the address in network byte order (big-endian), as on the wire. */
struct in_addr {
    u32 s_addr;
};

struct sockaddr_in {
    u16 sin_family; /* AF_INET */
    u16 sin_port;
    struct in_addr sin_addr;
    u8 sin_zero[8];
};

/* spawn's and exec's limits: at most MAXARG arguments (argv[0] included), whose strings
 * take at most ARG_MAX bytes with their NULs. They go on the new process's
 * stack, above its first stack frame. */
#define MAXARG 16
#define ARG_MAX 512

/* Handles the ecall in `f` (from U-mode, sepc already past it): runs the
 * call, puts the result in the frame's a0. Unknown numbers return -ENOSYS. */
void syscall(struct trap_frame *f);

#endif
