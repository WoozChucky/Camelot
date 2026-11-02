/* What user programs get: the system calls (usys.s, docs/camelot-syscalls.md)
 * and a tiny C library (ulib.c). No libc: programs are compiled freestanding,
 * with only this header. */
#ifndef CAMELOT_USER_H
#define CAMELOT_USER_H

typedef unsigned char uchar;
typedef unsigned int uint;
typedef unsigned long size_t;

#define NULL ((void *)0)

/* Error numbers: a system call that fails returns minus one of them. They
 * are kernel/include/syscall.h's: a test checks they agree. */
#define ENOENT 2   /* spawn: no program by that name */
#define E2BIG 7    /* spawn: argument list too long */
#define ENOEXEC 8  /* spawn: the program's image is not a valid executable */
#define EBADF 9    /* read, write: not an open file descriptor */
#define ECHILD 10  /* wait: no such child */
#define EAGAIN 11  /* spawn: every process slot is taken */
#define ENOMEM 12  /* spawn, sbrk: out of memory, or the heap would reach the stack */
#define EFAULT 14  /* a pointer argument is not valid user memory */
#define EINVAL 22  /* an argument is out of range */
#define ENOSYS 38  /* no such system call */

/* The console's file descriptors. */
#define STDIN 0
#define STDOUT 1
#define STDERR 2

/* spawn's limits: arguments, argv[0] included, and the bytes their strings
 * take with their NULs. */
#define MAXARG 16
#define ARG_MAX 512

/* The system calls. */
void exit(int status);
int write(int fd, const void *buf, uint len);
int read(int fd, void *buf, uint len);
int getpid(void);
int yield(void);
int sleep(uint ticks);
int spawn(const char *name, char *const argv[]);
int wait(int pid, int *status);
int sbrk(int increment);
uint uptime(void);
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

/* What error number `err` (positive) means, e.g. "no such program". */
const char *strerror(int err);

/* Busy-waits for about `n` iterations of a short loop, a few instructions
 * each: CPU time, as opposed to sleep(). */
void spin(uint n);

#endif
