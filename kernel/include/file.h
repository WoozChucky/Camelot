/* Open files (spec section 10, "Kernel files"), after xv6's file.c. A file
 * descriptor is an index into the process's `ofile` table (proc.h), whose
 * entries point into the kernel's table of open files: what open(), pipe()
 * or the first process's console set up, with the access it was opened for
 * and, for a file on the disk, the offset the next read or write uses.
 * Descriptors that dup() or spawn() copy share one open file, offset and
 * all; it goes when the last of them is closed.
 *
 * Three kinds:
 *
 *   FD_INODE   a file or directory of the filesystem (fs.h): reads and
 *              writes go through readi/writei at `off`, each write in
 *              transactions (log.h)
 *   FD_DEVICE  a device: the console. Opening a device node (an inode of
 *              type T_DEVICE) gives one, through the driver `devsw` names
 *              by its major number
 *   FD_PIPE    one end of a pipe (pipe.h)
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
#define NFILE 64

/* Descriptors per process. */
#define NOFILE 16

/* Device numbers (the major of a device inode). */
#define CONSOLE 1
#define NDEV 2

#define FD_NONE 0
#define FD_PIPE 1
#define FD_INODE 2
#define FD_DEVICE 3

struct pipe;

struct file {
    u32 type;
    u32 ref; /* descriptors pointing at it; 0: a free slot */
    bool readable;
    bool writable;
    struct pipe *pipe;  /* FD_PIPE */
    struct inode *ip;   /* FD_INODE, and FD_DEVICE opened by its path */
    u32 off;            /* FD_INODE */
    u16 major;          /* FD_DEVICE */
};

/* A device's driver: read and write on user memory, as console_read() and
 * console_write() do. */
struct devsw {
    int (*read)(pagetable_t pt, u32 dst, u32 n);
    int (*write)(pagetable_t pt, u32 src, u32 n);
};

extern const struct devsw devsw[NDEV];

/* A free slot of the table, with one reference and nothing else set; NULL
 * if every slot is taken. */
struct file *filealloc(void);

/* One more reference to `f`; returns it. */
struct file *filedup(struct file *f);

/* Drops a reference; the last one closes the file: a pipe end is closed
 * (pipeclose), an inode let go (iput, in a transaction of its own, which
 * frees it if it was unlinked meanwhile). */
void fileclose(struct file *f);

/* A console file: FD_DEVICE with major CONSOLE, readable or writable as
 * asked (the first process's descriptors 0, 1 and 2). NULL if the table is
 * full. */
struct file *file_console(bool readable, bool writable);

/* read(): up to `n` bytes into user address `dst`. Returns the bytes read
 * (0 at the end of the file or of the input), -EBADF if `f` isn't open for
 * reading, -EFAULT (having read nothing) unless [dst, dst + n) is writable
 * user memory, -ENOMEM if no page is free for copying through. */
int fileread(struct file *f, pagetable_t pt, u32 dst, u32 n);

/* write(): the `n` bytes at user address `src`. Returns how many were
 * written: all of them, or fewer if the disk filled up or a pipe's readers
 * went away part way; -EBADF, -EFAULT (nothing written), -ENOMEM, or what
 * writing the first byte failed with (-ENOSPC, -EFBIG, -EPIPE...). */
int filewrite(struct file *f, pagetable_t pt, u32 src, u32 n);

/* lseek(): sets an FD_INODE file's offset to `offset` from the start
 * (SEEK_SET), the current offset (SEEK_CUR) or the end (SEEK_END) and
 * returns it. -ESPIPE for a pipe or a device, -EINVAL for another `whence`
 * or an offset below 0 or past the end of the file (it has no holes). */
int filelseek(struct file *f, i32 offset, u32 whence);

/* fstat(): fills *st. */
void filestat(struct file *f, struct stat *st);

#endif
