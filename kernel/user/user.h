/* What user programs get: the system calls (lib/usys.s, camelot-syscalls.md)
 * and a small C library (lib/, described in lib/README.md). No libc:
 * programs are compiled freestanding, with only this header. */
#ifndef CAMELOT_USER_H
#define CAMELOT_USER_H

typedef unsigned char uchar;
typedef unsigned short ushort;
typedef unsigned int uint;

/* The system call ABI's types, Linux's: a `long` wide, 64 bits (LP64). A
 * size, a count of bytes or an error, a file offset, a time in seconds, an
 * integer that holds a pointer. */
typedef unsigned long size_t;
typedef long ssize_t;
typedef long off_t;
typedef long time_t;
typedef long intptr_t;
typedef unsigned long uintptr_t;

/* Integers of a given width, as <stdint.h> has them. The 64-bit ones are
 * long long, as Linux's are. */
typedef signed char int8_t;
typedef unsigned char uint8_t;
typedef short int16_t;
typedef unsigned short uint16_t;
typedef int int32_t;
typedef unsigned int uint32_t;
typedef long long int64_t;
typedef unsigned long long uint64_t;

/* The limits of the integer types, as <limits.h> and <stdint.h> give
 * them. A long is a register: 64 bits. */
#define INT_MAX 2147483647
#define INT_MIN (-INT_MAX - 1)
#define UINT_MAX 4294967295u
#define LONG_MAX 0x7fffffffffffffffL
#define ULONG_MAX 0xffffffffffffffffUL
#define LONG_MIN (-LONG_MAX - 1)
#define LLONG_MAX 0x7fffffffffffffffLL
#define LLONG_MIN (-LLONG_MAX - 1)
#define ULLONG_MAX 0xffffffffffffffffULL
#define INT32_MAX INT_MAX
#define INT32_MIN INT_MIN
#define UINT32_MAX UINT_MAX
#define INT64_MAX LLONG_MAX
#define INT64_MIN LLONG_MIN
#define UINT64_MAX ULLONG_MAX
#define SIZE_MAX ULONG_MAX
#define SSIZE_MAX LONG_MAX

#define NULL ((void *)0)

/* Error numbers: a system call that fails returns minus one of them. They
 * are kernel/include/syscall.h's: a test checks they agree. */
#define EPERM 1         /* link: not for a directory; kill: a kernel process, or the shell */
#define ENOENT 2        /* no such file or directory */
#define ESRCH 3         /* kill: no such process; thread_join: no such thread */
#define EINTR 4         /* a wait ended because the process was killed */
#define EIO 5           /* a pseudo-terminal whose other end is gone, or locked */
#define ENXIO 6         /* open: a socket's inode, which only connect() reaches */
#define E2BIG 7         /* spawn, exec: argument list too long */
#define ENOEXEC 8       /* spawn, exec: the file is not a valid executable */
#define EBADF 9         /* not an open file descriptor, or not open for that */
#define ECHILD 10       /* wait: no such child */
#define EAGAIN 11       /* spawn, thread_create: every slot is taken; futex_wait: not the value; a socket's receive timeout ran out */
#define ENOMEM 12       /* out of memory, or the heap would reach the stack or a mapping; mmap: no room; mprotect: not a mapping */
#define EACCES 13       /* spawn, exec: not a regular file; mmap, mprotect: the descriptor isn't open for that */
#define EFAULT 14       /* a pointer argument is not valid user memory */
#define EBUSY 16        /* ioctl: the device's memory is mapped, and the request would change it */
#define EEXIST 17       /* the name is taken; mmap: MAP_FIXED_NOREPLACE over a mapping */
#define ENODEV 19       /* open: a device node with no driver; mmap: a file that can't be mapped */
#define ENOTDIR 20      /* a name on the way is not a directory */
#define EISDIR 21       /* open: a directory, for writing */
#define EINVAL 22       /* an argument is out of range */
#define ENFILE 23       /* every slot of the kernel's open file table is taken */
#define EMFILE 24       /* every descriptor of the process is taken */
#define ENOTTY 25       /* ioctl: not a device, or a request the device doesn't know */
#define EFBIG 27        /* write: past the largest file */
#define ENOSPC 28       /* no free block or inode on the disk */
#define ESPIPE 29       /* lseek: not on a pipe or a device */
#define EROFS 30        /* the filesystem is read-only */
#define EPIPE 32        /* write: a pipe nobody reads any more */
#define ERANGE 34       /* getcwd: the buffer is too small for the path */
#define ENAMETOOLONG 36 /* a name longer than DIRSIZ, or a path longer than MAXPATH */
#define ENOSYS 38       /* no such system call */
#define ENOTEMPTY 39    /* unlink: a directory with entries */
#define EOVERFLOW 75    /* open: a file bigger than the kernel's offsets reach; mmap: past the largest object */
#define ENOTSOCK 88     /* a socket call on a descriptor that is not a socket */
#define EDESTADDRREQ 89 /* send: a UDP socket with no peer, and no address given */
#define EMSGSIZE 90     /* a datagram too big for one packet */
#define ENOPROTOOPT 92  /* setsockopt: an option sockets don't have */
#define EPROTONOSUPPORT 93 /* socket: a protocol the type doesn't have */
#define ESOCKTNOSUPPORT 94 /* socket, socketpair: a local socket of a type other than SOCK_STREAM */
#define EOPNOTSUPP 95   /* listen, accept: not on a UDP socket; mmap: a flag MAP_SHARED_VALIDATE refuses */
#define EAFNOSUPPORT 97 /* socket: neither AF_INET nor AF_UNIX; a TCP or UDP socket's address not AF_INET */
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

/* spawn's and exec's limits: at most MAXARG strings in argv (argv[0]
 * included) and as many in envp, and the bytes the strings of both take
 * with their NULs together. */
#define MAXARG 16
#define ARG_MAX 1024

/* A program starts as Linux starts one (docs/camelot-syscalls.md,
 * "Starting a program"): crt0 calls main(argc, argv, envp), and after
 * envp's NULL comes the auxiliary vector, pairs of a type and a value (a
 * long each), ending with AT_NULL. Its types, Linux's values
 * (kernel/include/syscall.h's: a test checks they agree). */
#define AT_NULL 0    /* the end of the vector */
#define AT_PHDR 3    /* where the program headers are, 0 if not loaded */
#define AT_PHENT 4   /* a program header's size */
#define AT_PHNUM 5   /* how many there are */
#define AT_PAGESZ 6  /* the page size: 4096 */
#define AT_BASE 7    /* the interpreter's base: 0, there is none */
#define AT_FLAGS 8   /* flags: 0 */
#define AT_ENTRY 9   /* the program's entry point, crt0's _start */
#define AT_UID 11    /* the user and group ids: 0, Camelot has one user */
#define AT_EUID 12
#define AT_GID 13
#define AT_EGID 14
#define AT_HWCAP 16  /* the ISA's letters, bit 'X' - 'A' for each: I, M and A */
#define AT_CLKTCK 17 /* how often uptime() ticks a second */
#define AT_SECURE 23 /* 0: not a setuid program */
#define AT_RANDOM 25 /* the address of 16 random bytes */
#define AT_EXECFN 31 /* the path the program was started with */

/* open's flags: one access mode, plus O_CREAT (make the file if it is
 * missing), O_TRUNC (empty it, when opened for writing) and O_CLOEXEC (the
 * descriptor closes on exec). */
#define O_RDONLY 0x000
#define O_WRONLY 0x001
#define O_RDWR 0x002
#define O_CREAT 0x040
#define O_TRUNC 0x200
#define O_CLOEXEC 0x80000

/* fcntl's commands, and the descriptor flag F_GETFD and F_SETFD read and
 * set. */
#define F_DUPFD 0            /* a copy at the lowest free descriptor >= arg */
#define F_GETFD 1            /* the descriptor's flags */
#define F_SETFD 2            /* sets them to arg */
#define F_DUPFD_CLOEXEC 1030 /* F_DUPFD, the copy closing on exec */
#define FD_CLOEXEC 1         /* the descriptor closes on exec */

/* mmap's and mprotect's protections, and mmap's flags (Linux's values;
 * kernel/include/mmap.h). */
#define PROT_NONE 0x0
#define PROT_READ 0x1
#define PROT_WRITE 0x2
#define PROT_EXEC 0x4
#define PROT_SEM 0x8
#define MAP_SHARED 0x01
#define MAP_PRIVATE 0x02
#define MAP_SHARED_VALIDATE 0x03
#define MAP_FIXED 0x10
#define MAP_ANONYMOUS 0x20
#define MAP_NORESERVE 0x4000
#define MAP_POPULATE 0x8000
#define MAP_FIXED_NOREPLACE 0x100000

/* memfd_create's flags (Linux's values; kernel/include/shm.h). */
#define MFD_CLOEXEC 0x1
#define MFD_ALLOW_SEALING 0x2

/* /dev/fb0's ioctl requests and structures, Linux's fbdev
 * (<linux/fb.h>; kernel/include/fb.h, docs/camelot-syscalls.md "The
 * framebuffer"). */
#define FBIOGET_VSCREENINFO 0x4600
#define FBIOPUT_VSCREENINFO 0x4601
#define FBIOGET_FSCREENINFO 0x4602
#define FBIOPAN_DISPLAY 0x4606
#define FBIOBLANK 0x4611
#define FBIO_WAITFORVSYNC 0x40044620
#define FB_ACTIVATE_NOW 0
#define FB_ACTIVATE_NXTOPEN 1
#define FB_ACTIVATE_TEST 2
#define FB_ACTIVATE_MASK 15
#define FB_ACTIVATE_VBL 16
#define FB_TYPE_PACKED_PIXELS 0
#define FB_VISUAL_TRUECOLOR 2
#define FB_BLANK_UNBLANK 0
#define FB_BLANK_POWERDOWN 4

struct fb_bitfield {
    uint offset;
    uint length;
    uint msb_right;
};

/* 160 bytes: the mode, the buffer's size and the row shown from. */
struct fb_var_screeninfo {
    uint xres;
    uint yres;
    uint xres_virtual;
    uint yres_virtual;
    uint xoffset;
    uint yoffset;
    uint bits_per_pixel;
    uint grayscale;
    struct fb_bitfield red;
    struct fb_bitfield green;
    struct fb_bitfield blue;
    struct fb_bitfield transp;
    uint nonstd;
    uint activate;
    uint height;
    uint width;
    uint accel_flags;
    uint pixclock;
    uint left_margin;
    uint right_margin;
    uint upper_margin;
    uint lower_margin;
    uint hsync_len;
    uint vsync_len;
    uint sync;
    uint vmode;
    uint rotate;
    uint colorspace;
    uint reserved[4];
};

/* 80 bytes: the buffer. */
struct fb_fix_screeninfo {
    char id[16];
    unsigned long smem_start;
    uint smem_len;
    uint type;
    uint type_aux;
    uint visual;
    ushort xpanstep;
    ushort ypanstep;
    ushort ywrapstep;
    uint line_length;
    unsigned long mmio_start;
    uint mmio_len;
    uint accel;
    ushort capabilities;
    ushort reserved[2];
};

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

/* 24 bytes (the size a 64-bit off_t). */
struct stat {
    uint ino;     /* the inode; 0 for a pipe or the shell's console */
    ushort type;  /* T_DIR, T_FILE, T_DEVICE, T_PIPE or T_SOCKET */
    ushort nlink; /* directory entries naming it; a socket: 0 once its other end hung up */
    off_t size;   /* bytes; for a pipe, those waiting to be read */
    ushort major; /* a device's numbers */
    ushort minor;
};

/* What reading a directory returns: one of these per slot, inum 0 for a
 * free one. A name of DIRSIZ bytes has no NUL. */
struct dirent {
    ushort inum;
    char name[DIRSIZ];
};

/* What procs stores for each process: 40 bytes. */
struct procinfo {
    uint pid;
    uint ppid;   /* its parent's pid, 0 if it has none */
    uint state;  /* PROC_RUNNABLE, PROC_RUNNING, PROC_ZOMBIE or PROC_SLEEPING */
    size_t size; /* the end of its heap: program, data and heap, from 0 */
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

/* Sockets (docs/camelot-syscalls.md, "Sockets" and "Local sockets"):
 * IPv4's TCP (SOCK_STREAM) and UDP (SOCK_DGRAM), and local streams
 * (AF_UNIX, SOCK_STREAM). Linux's values and layout. */
#define AF_UNIX 1
#define AF_LOCAL AF_UNIX
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
#define SCM_RIGHTS 1   /* a control message of descriptors (ints) */

/* sendmsg's and recvmsg's flags. */
#define MSG_CTRUNC 0x8              /* recvmsg: descriptors that did not fit were closed */
#define MSG_TRUNC 0x20              /* recvmsg: a datagram's end did not fit */
#define MSG_DONTWAIT 0x40           /* -EAGAIN rather than wait (local sockets) */
#define MSG_NOSIGNAL 0x4000         /* nothing to do: Camelot has no SIGPIPE */
#define MSG_CMSG_CLOEXEC 0x40000000 /* recvmsg: the descriptors close on exec */

/* poll(): the descriptors and what to wait for (kernel/include/poll.h,
 * docs/camelot-syscalls.md "Polling"). Linux's values and layout. */
#define POLLIN 0x001
#define POLLPRI 0x002
#define POLLOUT 0x004
#define POLLERR 0x008
#define POLLHUP 0x010
#define POLLNVAL 0x020
#define POLLRDNORM 0x040
#define POLLRDBAND 0x080
#define POLLWRNORM 0x100
#define POLLWRBAND 0x200
#define POLLRDHUP 0x2000

struct pollfd {
    int fd;        /* a negative one is skipped */
    short events;  /* what to wait for */
    short revents; /* what came: of those, and POLLERR, POLLHUP, POLLNVAL */
};

/* A time: seconds and microseconds, two longs, as Linux's. */
struct timeval {
    time_t tv_sec;
    long tv_usec;
};

/* Terminals: the console and the pseudo-terminals, /dev/ptmx and
 * /dev/pts/N (kernel/include/tty.h, pty.h; docs/camelot-syscalls.md
 * "Terminals"). Linux's requests, and its struct termios for TCGETS, 36
 * bytes; the flags have Linux's values, written in hex. */
#define TCGETS 0x5401
#define TCSETS 0x5402
#define TCSETSW 0x5403
#define TCSETSF 0x5404
#define TCFLSH 0x540B
#define TIOCGPGRP 0x540F
#define TIOCSPGRP 0x5410
#define TIOCGWINSZ 0x5413
#define TIOCSWINSZ 0x5414
#define FIONREAD 0x541B
#define TIOCGPTN 0x80045430
#define TIOCSPTLCK 0x40045431

#define TCIFLUSH 0
#define TCOFLUSH 1
#define TCIOFLUSH 2

/* tcsetattr's `when`. */
#define TCSANOW 0
#define TCSADRAIN 1
#define TCSAFLUSH 2

#define NCCS 19
struct termios {
    uint c_iflag;
    uint c_oflag;
    uint c_cflag;
    uint c_lflag;
    unsigned char c_line;
    unsigned char c_cc[NCCS];
};

#define VINTR 0
#define VQUIT 1
#define VERASE 2
#define VKILL 3
#define VEOF 4
#define VTIME 5
#define VMIN 6
#define VSWTC 7
#define VSTART 8
#define VSTOP 9
#define VSUSP 10
#define VEOL 11
#define VREPRINT 12
#define VDISCARD 13
#define VWERASE 14
#define VLNEXT 15
#define VEOL2 16

/* c_iflag. */
#define IGNBRK 0x1
#define BRKINT 0x2
#define IGNPAR 0x4
#define PARMRK 0x8
#define INPCK 0x10
#define ISTRIP 0x20
#define INLCR 0x40
#define IGNCR 0x80
#define ICRNL 0x100
#define IXON 0x400
#define IXOFF 0x1000

/* c_oflag. */
#define OPOST 0x1
#define ONLCR 0x4

/* c_cflag. */
#define B38400 0xf
#define CSIZE 0x30
#define CS8 0x30
#define CREAD 0x80
#define PARENB 0x100
#define HUPCL 0x400

/* c_lflag. */
#define ISIG 0x1
#define ICANON 0x2
#define ECHO 0x8
#define ECHOE 0x10
#define ECHOK 0x20
#define ECHONL 0x40
#define NOFLSH 0x80
#define ECHOCTL 0x200
#define ECHOKE 0x800
#define IEXTEN 0x8000

struct winsize {
    ushort ws_row;
    ushort ws_col;
    ushort ws_xpixel;
    ushort ws_ypixel;
};

/* /dev/input/event0 (the keyboard) and event1 (the pointer): Linux's evdev
 * (<linux/input.h>, <linux/input-event-codes.h>; kernel/include/input.h,
 * docs/camelot-syscalls.md "Input devices"). A read gives whole events. */
struct input_event {
    struct timeval time;
    ushort type;
    ushort code;
    int value;
};

struct input_id {
    ushort bustype;
    ushort vendor;
    ushort product;
    ushort version;
};

struct input_absinfo {
    int value;
    int minimum;
    int maximum;
    int fuzz;
    int flat;
    int resolution;
};

#define EV_VERSION 0x010001
#define EVIOCGVERSION 0x80044501
#define EVIOCGID 0x80084502
#define EVIOCGRAB 0x40044590
#define EVIOCGNAME(len) (0x80004506 | (len) << 16)
#define EVIOCGBIT(ev, len) (0x80004520 + (ev) | (len) << 16)
#define EVIOCGABS(abs) (0x80184540 + (abs))
#define EVIOCGKEY(len) (0x80004518 | (len) << 16)

#define EV_SYN 0x00
#define EV_KEY 0x01
#define EV_REL 0x02
#define EV_ABS 0x03
#define EV_MSC 0x04
#define EV_CNT 0x20
#define SYN_REPORT 0
#define SYN_DROPPED 3
#define REL_X 0x00
#define REL_Y 0x01
#define REL_WHEEL 0x08
#define ABS_X 0x00
#define ABS_Y 0x01
#define BTN_LEFT 0x110
#define BTN_RIGHT 0x111
#define BTN_MIDDLE 0x112
#define KEY_CNT 0x300
#define BUS_HOST 0x19

/* The keys of a PC keyboard's main block and the keys around it. */
#define KEY_ESC 1
#define KEY_1 2
#define KEY_2 3
#define KEY_3 4
#define KEY_4 5
#define KEY_5 6
#define KEY_6 7
#define KEY_7 8
#define KEY_8 9
#define KEY_9 10
#define KEY_0 11
#define KEY_MINUS 12
#define KEY_EQUAL 13
#define KEY_BACKSPACE 14
#define KEY_TAB 15
#define KEY_Q 16
#define KEY_W 17
#define KEY_E 18
#define KEY_R 19
#define KEY_T 20
#define KEY_Y 21
#define KEY_U 22
#define KEY_I 23
#define KEY_O 24
#define KEY_P 25
#define KEY_LEFTBRACE 26
#define KEY_RIGHTBRACE 27
#define KEY_ENTER 28
#define KEY_LEFTCTRL 29
#define KEY_A 30
#define KEY_S 31
#define KEY_D 32
#define KEY_F 33
#define KEY_G 34
#define KEY_H 35
#define KEY_J 36
#define KEY_K 37
#define KEY_L 38
#define KEY_SEMICOLON 39
#define KEY_APOSTROPHE 40
#define KEY_GRAVE 41
#define KEY_LEFTSHIFT 42
#define KEY_BACKSLASH 43
#define KEY_Z 44
#define KEY_X 45
#define KEY_C 46
#define KEY_V 47
#define KEY_B 48
#define KEY_N 49
#define KEY_M 50
#define KEY_COMMA 51
#define KEY_DOT 52
#define KEY_SLASH 53
#define KEY_RIGHTSHIFT 54
#define KEY_LEFTALT 56
#define KEY_SPACE 57
#define KEY_CAPSLOCK 58
#define KEY_F1 59
#define KEY_F2 60
#define KEY_F3 61
#define KEY_F4 62
#define KEY_F5 63
#define KEY_F6 64
#define KEY_F7 65
#define KEY_F8 66
#define KEY_F9 67
#define KEY_F10 68
#define KEY_F11 87
#define KEY_F12 88
#define KEY_RIGHTCTRL 97
#define KEY_RIGHTALT 100
#define KEY_HOME 102
#define KEY_UP 103
#define KEY_PAGEUP 104
#define KEY_LEFT 105
#define KEY_RIGHT 106
#define KEY_END 107
#define KEY_DOWN 108
#define KEY_PAGEDOWN 109
#define KEY_INSERT 110
#define KEY_DELETE 111
#define KEY_LEFTMETA 125
#define KEY_RIGHTMETA 126

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

/* A local socket's address: 110 bytes, a path that ends with a NUL. */
#define UNIX_PATH_MAX 108

struct sockaddr_un {
    ushort sun_family; /* AF_UNIX */
    char sun_path[UNIX_PATH_MAX];
};

/* sendmsg and recvmsg's message (56 bytes): an address, the buffers to
 * gather from or scatter into (16 bytes each), and the control messages,
 * each a struct cmsghdr (16 bytes) and its data, 8-byte aligned. */
struct iovec {
    void *iov_base;
    size_t iov_len;
};

struct msghdr {
    void *msg_name;        /* an address, or NULL */
    uint msg_namelen;      /* its size; recvmsg: what it stored */
    struct iovec *msg_iov;
    size_t msg_iovlen;
    void *msg_control;     /* the control messages, or NULL */
    size_t msg_controllen; /* their size; recvmsg: what it stored */
    int msg_flags;         /* recvmsg: MSG_CTRUNC, MSG_TRUNC */
};

struct cmsghdr {
    size_t cmsg_len; /* CMSG_LEN of the data */
    int cmsg_level;  /* SOL_SOCKET */
    int cmsg_type;   /* SCM_RIGHTS */
};

/* Linux's: a message's length for `len` bytes of data, the room it takes
 * with its padding, its data, and the first and next messages of a struct
 * msghdr (NULL at the end; lib/cmsg.c). */
#define CMSG_ALIGN(len) (((len) + sizeof(size_t) - 1) & ~(sizeof(size_t) - 1))
#define CMSG_LEN(len) (sizeof(struct cmsghdr) + (len))
#define CMSG_SPACE(len) (sizeof(struct cmsghdr) + CMSG_ALIGN(len))
#define CMSG_DATA(cmsg) ((unsigned char *)((struct cmsghdr *)(cmsg) + 1))
#define CMSG_FIRSTHDR(msg)                                                                       \
    ((msg)->msg_controllen >= sizeof(struct cmsghdr) ? (struct cmsghdr *)(msg)->msg_control \
                                                      : (struct cmsghdr *)NULL)
#define CMSG_NXTHDR(msg, cmsg) cmsg_nxthdr((msg), (cmsg))
struct cmsghdr *cmsg_nxthdr(struct msghdr *msg, struct cmsghdr *cmsg);

/* The system calls, with Linux's types: a call that returns a count of
 * bytes returns an ssize_t, lseek an off_t; sbrk the old end of the heap
 * and mmap the new mapping's address, a pointer, or minus an error number
 * as a pointer ((long)p < 0). */
void exit(int status);
ssize_t write(int fd, const void *buf, size_t len);
ssize_t read(int fd, void *buf, size_t len);
int getpid(void);
int yield(void);
int sleep(uint ticks);
int spawn(const char *path, char *const argv[], char *const envp[]);
int wait(int pid, int *status);
void *sbrk(long increment);
uint uptime(void);
int open(const char *path, int flags);
int close(int fd);
off_t lseek(int fd, off_t offset, int whence);
int fstat(int fd, struct stat *st);
int mkdir(const char *path);
int unlink(const char *path);
int link(const char *old, const char *new);
int chdir(const char *path);
int dup(int fd);
int pipe(int *fds);
int exec(const char *path, char *const argv[], char *const envp[]);
int kill(int pid);
int procs(struct procinfo *buf, uint max);
int harts(struct hartinfo *buf, uint max);
int netinfo(struct netinfo *info);
int netconf(uint addr, uint mask, uint gateway);
int ping(uint addr, uint seq, struct ping_reply *reply);
int socket(int domain, int type, int protocol);
int bind(int fd, const void *addr);
int listen(int fd, int backlog);
int accept(int fd, void *addr);
int connect(int fd, const void *addr);
ssize_t send(int fd, const void *buf, size_t len);
ssize_t recv(int fd, void *buf, size_t len);
ssize_t sendto(int fd, const void *buf, size_t len, const void *to);
ssize_t recvfrom(int fd, void *buf, size_t len, void *from);
int shutdown(int fd, int how);
int getsockname(int fd, void *addr);
int getpeername(int fd, void *addr);
int setsockopt(int fd, int level, int option, const void *value, uint len);
int thread_create(void (*entry)(void *), void *arg, void *stack);
void thread_exit(int status);
int thread_join(int tid, int *status);
int gettid(void);
int futex_wait(int *addr, int val);
int futex_wake(int *addr, int n);
int fcntl(int fd, int cmd, int arg);
void *mmap(void *addr, size_t len, int prot, int flags, int fd, off_t offset);
int munmap(void *addr, size_t len);
int mprotect(void *addr, size_t len, int prot);
int memfd_create(const char *name, uint flags);
int ftruncate(int fd, off_t length);
int ioctl(int fd, unsigned long request, ...);
int poll(struct pollfd *fds, unsigned long nfds, int timeout);
int socketpair(int domain, int type, int protocol, int *fds);
ssize_t sendmsg(int fd, const struct msghdr *msg, int flags);
ssize_t recvmsg(int fd, struct msghdr *msg, int flags);
int stat(const char *path, struct stat *st);
/* The working directory's path, written to buf with its NUL (Linux's
 * getcwd system call): returns its length, the NUL counted, or -ERANGE if
 * size bytes don't hold it, -ENOENT if the directory was removed. Where
 * glibc's getcwd() returns buf and sets errno, this one returns the length
 * or the error, as the rest of this library does. */
long getcwd(char *buf, size_t size);
/* Any call by number (for tests): its result, a long, as the kernel left
 * it in a0. */
long syscall(long number, long a0, long a1, long a2);

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
int atoi(const char *s);
long strtol(const char *s, char **end, int base);

/* The environment (lib/environ.c, lib/getenv.c), as POSIX has it: environ is
 * the program's, a NULL-terminated array of NAME=value strings, which
 * crt0 sets to main's envp before calling it (as glibc's
 * __libc_start_main does), and which a program passes on to spawn or exec
 * (or another array of its own making). getenv: the value of the first
 * entry called `name` (what follows its first '='), or NULL if there is
 * none or `name` is empty or has a '=' in it. setenv: gives `name` the
 * value `value`, adding it at the end, or replacing it if it is there and
 * `overwrite` is not 0 (else it stays); returns 0, -EINVAL for a bad name
 * (as getenv's) or -ENOMEM. unsetenv: removes every entry called `name`;
 * 0, or -EINVAL. Errors come back as numbers, as from the kernel: there is
 * no errno. setenv changes environ (to a new array, from malloc, the first
 * time it adds a variable), unsetenv the array in place; neither frees a
 * string it replaces or removes, so that what getenv returned stays good,
 * as glibc keeps them. */
extern char **environ;
char *getenv(const char *name);
int setenv(const char *name, const char *value, int overwrite);
int unsetenv(const char *name);

/* The heap (lib/malloc.c), on top of sbrk: blocks aligned to the size of
 * malloc's header, two pointers' worth (16 bytes: the ABI's largest
 * alignment, as glibc's malloc gives), a free list with
 * neighbours merged. malloc(0) and a failure return NULL.
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
 * '-' (left align), '0' (zero padding) and width. d, u and x take an int
 * (or unsigned int), or with a length before them a wider one: 'l' a long,
 * 'll' a long long (int64_t), 'z' a size_t (or ssize_t, with d). %p prints
 * every digit of a pointer, 16. printf and fprintf return the
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

/* Kills every process `pid` started that has not ended, and theirs, down
 * to the last generation, from one procs() snapshot: the youngest first,
 * so that none is left an orphan. Not `pid` itself (which may be the
 * caller: kill_descendants(getpid()) ends what a program started). Returns
 * how many it killed. One thread at a time (lib/proc.c). */
int kill_descendants(int pid);

/* Terminals (lib/tty.c), as POSIX has them, but for returning a negative
 * error number rather than -1 and errno.
 *
 * isatty: 1 if `fd` is a terminal (TCGETS answers), else 0. tcgetattr and
 * tcsetattr: TCGETS, and TCSETS, TCSETSW or TCSETSF as `when` is TCSANOW,
 * TCSADRAIN or TCSAFLUSH; cfmakeraw makes `t` raw mode, as glibc's does.
 * tcgetpgrp and tcsetpgrp: the foreground process (a pid until Camelot has
 * process groups; 0, none) that Ctrl-C kills, through TIOCGPGRP and
 * TIOCSPGRP.
 *
 * posix_openpt opens /dev/ptmx with `flags` (O_RDWR) and returns the
 * master's descriptor; grantpt only checks that `fd` is one (Camelot has
 * no owners to set), and unlockpt unlocks its slave (TIOCSPTLCK), both 0
 * or an error (-ENOTTY for anything but a master); ptsname is
 * the slave's path, "/dev/pts/N", in a buffer of its own overwritten by
 * the next call, or NULL. ttyname is the path of the terminal `fd` is,
 * "/dev/console" or "/dev/pts/N", the same way, or NULL for anything else. */
int isatty(int fd);
int tcgetattr(int fd, struct termios *t);
int tcsetattr(int fd, int when, const struct termios *t);
void cfmakeraw(struct termios *t);
int tcgetpgrp(int fd);
int tcsetpgrp(int fd, int pid);
int posix_openpt(int flags);
int grantpt(int fd);
int unlockpt(int fd);
char *ptsname(int fd);
char *ttyname(int fd);

/* Busy-waits for about `n` iterations of a short loop, a few instructions
 * each: CPU time, as opposed to sleep() (lib/spin.c). */
void spin(uint n);

#endif
