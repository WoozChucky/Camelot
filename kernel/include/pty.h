/* Pseudo-terminals (spec section 10, "Terminals"; after Linux's
 * drivers/tty/pty.c and devpts): a pair of devices, a master and a slave,
 * where the slave is a terminal (tty.h) and the master the other end of
 * its line, as a terminal emulator holds it. Opening /dev/ptmx (major
 * PTMX) makes a pair and gives its master; the slave is /dev/pts/N (major
 * PTS, minor N), N being what TIOCGPTN says, and opens once TIOCSPTLCK
 * has unlocked it (glibc's unlockpt), as on Linux.
 *
 * What the master writes is the slave's input, through the line
 * discipline: edited, echoed back to the master, its ^C acted on. What the
 * slave writes, through the output processing (NL to CR NL), is what the
 * master reads, from a 2 KiB ring; a slave writer waits for room, and the
 * echo that finds none is dropped. The pair has one termios, window size
 * and foreground, which the requests on either end reach.
 *
 * Closing the master hangs the slave up: its reads return what was left,
 * then 0, its writes -EIO. Once every slave file has closed (one was open),
 * the master's reads return what was left, then -EIO, and its writes -EIO,
 * until a slave opens again. A pair goes when both ends are closed.
 *
 * The root image has /dev/ptmx and NPTY slaves' nodes, /dev/pts/0 to 15,
 * since Camelot has no devpts to make them as pairs come and go: a slave
 * whose pair doesn't exist opens with -ENXIO, one whose master is gone or
 * that is locked with -EIO.
 *
 * Locking: pty_lock, a spinlock, guards every pair and is their
 * terminals' lock. A pair's channels: its terminal (the slave's readers),
 * its terminal's input (the master's writers, waiting for room),
 * `out_head` (the master's readers: output came) and `out_tail` (the
 * slave's writers: output was read). */
#ifndef CAMELOT_PTY_H
#define CAMELOT_PTY_H

#include "types.h"
#include "vm.h"

struct file;
struct poll_table;

/* Pairs at once: /dev/pts/0 to /dev/pts/15. */
#define NPTY 16

void pty_init(void);

/* /dev/ptmx: open makes a pair (-ENOSPC if there are NPTY, -ENOMEM). */
int ptmx_open(struct file *f);
void ptmx_close(struct file *f);
isize ptmx_read(struct file *f, pagetable_t pt, uintptr dst, usize n);
isize ptmx_write(struct file *f, pagetable_t pt, uintptr src, usize n);
long ptmx_ioctl(struct file *f, u32 request, uintptr arg);
u32 ptmx_poll(struct file *f, struct poll_table *pt);

/* /dev/pts/N. */
int pts_open(struct file *f);
void pts_close(struct file *f);
isize pts_read(struct file *f, pagetable_t pt, uintptr dst, usize n);
isize pts_write(struct file *f, pagetable_t pt, uintptr src, usize n);
long pts_ioctl(struct file *f, u32 request, uintptr arg);
u32 pts_poll(struct file *f, struct poll_table *pt);

#endif
