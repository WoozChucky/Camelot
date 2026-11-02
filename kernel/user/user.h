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
#define ESRCH 3         /* kill: no such process */
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

/* What procs stores for each process: 32 bytes. */
struct procinfo {
    uint pid;
    uint ppid;  /* its parent's pid, 0 if it has none */
    uint state; /* PROC_RUNNABLE, PROC_RUNNING, PROC_ZOMBIE or PROC_SLEEPING */
    uint size;  /* the end of its heap: program, data and heap, from 0 */
    char name[16];
};

#define PROC_RUNNABLE 1 /* waiting for its turn */
#define PROC_RUNNING 2  /* on the CPU: the caller */
#define PROC_ZOMBIE 3   /* ended, not collected by its parent yet */
#define PROC_SLEEPING 4 /* waiting: for input, a child, the clock, a disk */

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

/* fstat() on the file at `path`: opens it for reading, then closes it.
 * Returns 0 or the error (lib/stat.c). */
int stat(const char *path, struct stat *st);

/* Busy-waits for about `n` iterations of a short loop, a few instructions
 * each: CPU time, as opposed to sleep() (lib/spin.c). */
void spin(uint n);

#endif
