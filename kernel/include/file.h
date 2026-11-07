/* Open files (spec section 10, "Kernel files"), after xv6's file.c. A file
 * descriptor is an index into the process's `ofile` table (proc.h), whose
 * entries point into the kernel's table of open files: what open(), pipe()
 * or the first process's console set up, with the access it was opened for
 * and, for a file on the disk, the offset the next read or write uses.
 * Descriptors that dup() or spawn() copy share one open file, offset and
 * all; it goes when the last of them is closed.
 *
 * The kinds:
 *
 *   FD_INODE   a file or directory of the filesystem (fs.h): reads and
 *              writes go through readi/writei at `off`, each write in
 *              transactions (log.h)
 *   FD_DEVICE  a device: the console. Opening a device node (an inode of
 *              type T_DEVICE) gives one, through the driver `devsw` names
 *              by its major number
 *   FD_PIPE    one end of a pipe (pipe.h)
 *   FD_SOCKET  a TCP or UDP socket (net.h, net/socket.c): reads and writes
 *              are recv() and send()
 *   FD_SHM     a shared-memory object (memfd_create, shm.h): mapped with
 *              mmap() and sized with ftruncate(), not read or written
 *   FD_UNIX    a local socket (unix.h, net/unix.c): reads and writes are
 *              its recv() and send()
 *
 * Every function here runs in a process's kernel thread (they may sleep,
 * for the disk or a pipe) and takes user addresses in that process's page
 * table. */
#ifndef CAMELOT_FILE_H
#define CAMELOT_FILE_H

#include "fs.h"
#include "syscall.h"
#include "types.h"
#include "vm.h"

/* Open files in the kernel: enough for every process to use most of its
 * descriptors on files of its own. */
#define NFILE 256

/* Descriptors per process. */
#define NOFILE 64

/* Device numbers (the major of a device inode): the console, the
 * framebuffer (fb.h) and the input devices (input.h), whose minor says
 * which: 0 the keyboard, 1 the pointer, as Linux's /dev/input/eventN share
 * one major; /dev/ptmx and the pseudo-terminals' slaves (pty.h), whose
 * minor is N of /dev/pts/N. */
#define CONSOLE 1
#define FB 2
#define INPUT 3
#define PTMX 4
#define PTS 5
#define NDEV 6

#define FD_NONE 0
#define FD_PIPE 1
#define FD_INODE 2
#define FD_DEVICE 3
#define FD_SOCKET 4
#define FD_SHM 5
#define FD_UNIX 6

struct pipe;
struct poll_table;
struct socket;
struct shm_object;
struct unix_sock;

struct file {
    u32 type;
    u32 ref; /* descriptors pointing at it; 0: a free slot */
    bool readable;
    bool writable;
    struct pipe *pipe;  /* FD_PIPE */
    struct socket *sock; /* FD_SOCKET */
    struct shm_object *shm; /* FD_SHM, with a reference the file holds */
    struct unix_sock *usock; /* FD_UNIX */
    struct inode *ip;   /* FD_INODE, and FD_DEVICE opened by its path */
    u64 off;            /* FD_INODE: at most MAXFILE_BYTES (fs.h) */
    u16 major;          /* FD_DEVICE */
    u16 minor;          /* FD_DEVICE */
    /* FD_DEVICE: the driver's own, for this open file (Linux's
     * private_data): an input device's reader, say. Set by its open. */
    void *priv;
    /* The effective ids of the process that made it (filealloc), which
     * fstat gives as the owner of what has no inode: a pipe, a socket, a
     * memfd, the first process's console. */
    u32 uid;
    u32 gid;
};

/* A device's driver, Linux's file_operations for the files of its major,
 * each function given the open file (whose `minor` and `priv` it may use).
 * Any may be NULL:
 *
 *   open      as open() makes the file, before it has a descriptor: 0, or
 *             the error open() returns (the file then goes, without a
 *             close). The first process's console files have none
 *   close     as the file's last reference goes
 *   read      read() on user memory, as console_read() does; NULL: -EINVAL
 *   write     write(), the same way
 *   ioctl     ioctl(fd, request, arg): its result, or -ENOTTY for a
 *             request the device doesn't know (and when NULL)
 *   poll      what the file is ready for now, poll.h's bits, having named
 *             the channels its waiters sleep on with poll_wait() first;
 *             NULL: always ready to read and write, as on Linux
 *   mmap      for a device with memory a program may map (Linux's
 *             f_op->mmap): given the mapping's offset into that memory and
 *             its size (whole pages), 0 with the physical address of the
 *             first page in *pa, the rest following it, or an error
 *             (-EINVAL past the memory). NULL: -ENODEV. A mapping it
 *             gives counts as a vm_open's, taken with the address, so
 *             that the memory can't change between the two (mmap.c lets
 *             go of it with a vm_close if the mapping isn't made)
 *   vm_open   a mapping of the device's memory more, or one fewer: an area
 *   vm_close  split in two, merged or unmapped (Linux's vm_operations
 *             open and close), so that the driver knows while its memory
 *             is mapped. Called holding spinlocks (a process's vm_lock, or
 *             proc_lock when a process is reaped): they may take only a
 *             lock under which nothing sleeps or wakes a sleeper (either
 *             takes proc_lock), and should leave slow work, such as
 *             freeing memory, for later (fb.c's fb_reclaim) */
struct devsw {
    int (*open)(struct file *f);
    void (*close)(struct file *f);
    isize (*read)(struct file *f, pagetable_t pt, uintptr dst, usize n);
    isize (*write)(struct file *f, pagetable_t pt, uintptr src, usize n);
    long (*ioctl)(struct file *f, u32 request, uintptr arg);
    u32 (*poll)(struct file *f, struct poll_table *pt);
    int (*mmap)(struct file *f, u64 offset, usize size, phys_addr_t *pa);
    void (*vm_open)(void);
    void (*vm_close)(void);
};

extern const struct devsw devsw[NDEV];

/* Whether `devsw` has a driver for `major`: open() fails with -ENODEV
 * on a device node without one. */
bool device_has_driver(u32 major);

/* A device file's open, through its driver's hook (devsw), once the file
 * is set up: 0, or the hook's error. */
int device_open(struct file *f);

/* devsw's vm_open and vm_close for an area mapping device `major`'s
 * memory (mmap.c), when the driver has them. */
void device_vm_open(u32 major);
void device_vm_close(u32 major);

/* ioctl(): `request` with `arg` on `f`, through its device's driver;
 * -ENOTTY for anything but a device, or a request it doesn't know. */
long fileioctl(struct file *f, u32 request, uintptr arg);

/* poll() on `f` (poll.h): the POLL bits it is ready for, its channels
 * added to `pt`. A pipe, a socket (local or not) or a device with a poll
 * hook says; a
 * file on the disk, a shared-memory object and any other device are
 * always ready to read and write (Linux's DEFAULT_POLLMASK). */
u32 filepoll(struct file *f, struct poll_table *pt);

/* Sets up the table's lock. kmain calls it at boot. */
void file_init(void);

/* A free slot of the table, with one reference and nothing else set; NULL
 * if every slot is taken. fileclose() gives it back. */
struct file *filealloc(void);

/* One more reference to `f`; returns it. */
struct file *filedup(struct file *f);

/* Drops a reference; the last one closes the file: a pipe end is closed
 * (pipeclose), a socket closed (socket_close, unix_close), an inode let go
 * (iput, in a transaction of its own, which frees it if it was unlinked
 * meanwhile). */
void fileclose(struct file *f);

/* A console file: FD_DEVICE with major CONSOLE, readable or writable as
 * asked (the first process's descriptors 0, 1 and 2). NULL if the table is
 * full. */
struct file *file_console(bool readable, bool writable);

/* The most one read() or write() moves, as Linux's MAX_RW_COUNT: INT_MAX
 * rounded down to a page. A bigger count is taken as this, so a result
 * always fits an int. */
#define MAX_RW_COUNT (0x7fffffffu & ~(u32)(PGSIZE - 1))

/* read(): up to `n` bytes into user address `dst`. Returns the bytes read
 * (0 at the end of the file or of the input), -EBADF if `f` isn't open for
 * reading, -EFAULT (having read nothing) unless [dst, dst + n) is writable
 * user memory, -ENOMEM if no page is free for copying through. */
isize fileread(struct file *f, pagetable_t pt, uintptr dst, usize n);

/* write(): the `n` bytes at user address `src`. Returns how many were
 * written: all of them, or fewer if the disk filled up or a pipe's readers
 * went away part way; -EBADF, -EFAULT (nothing written), -ENOMEM, or what
 * writing the first byte failed with (-ENOSPC, -EFBIG, -EPIPE...). */
isize filewrite(struct file *f, pagetable_t pt, uintptr src, usize n);

/* lseek(): sets an FD_INODE file's offset to `offset` from the start
 * (SEEK_SET), the current offset (SEEK_CUR) or the end (SEEK_END) and
 * returns it. -ESPIPE for a pipe, a socket or a device, -EINVAL for another
 * `whence` or an offset below 0 or past the end of the file (it has no
 * holes), which an offset past the largest file is too: whatever its
 * value, never a part of it. */
off_t filelseek(struct file *f, off_t offset, u32 whence);

/* fstat(): fills *st. */
void filestat(struct file *f, struct stat *st);

/* What fstat() and stat() say of the locked inode `ip`, into *st. */
void inode_stat(struct inode *ip, struct stat *st);

/* memfd_create(): a new open file, readable and writable, for a new empty
 * shared-memory object (FD_SHM): -ENFILE if the table of files or of
 * objects is full. */
struct file *file_shm(bool sealing, int *err);

/* ftruncate(): sets the size of the shared-memory object behind `f`. 0, or
 * -EINVAL (not a shared-memory object, or not open for writing; `length`
 * below 0), -EFBIG (past the largest object). */
int filetruncate(struct file *f, off_t length);

struct vm_area;

/* What a mapping of `f` (mmap() of a descriptor) maps: fills in the
 * backing of `a` (its kind, its object and offset, or its device's
 * physical address, and whether mprotect() may make it writable) for
 * `size` bytes from `offset`, a mapping of type `type` (MAP_SHARED or
 * MAP_PRIVATE) with protections `prot` (mmap.h). For a shared-memory
 * object, `a` holds a reference to it from then on, which the area takes
 * over (or mmap lets go of). 0, or -EACCES (`f` isn't open for reading, or
 * for writing a writable shared mapping), -EINVAL (a private mapping of a
 * shared-memory object, or a device range past its memory), -ENODEV (a
 * file that can't be mapped: a regular file, a directory, a pipe, a
 * socket, the console). */
int filemmap(struct file *f, u32 type, u32 prot, u64 offset, usize size, struct vm_area *a);

#endif
