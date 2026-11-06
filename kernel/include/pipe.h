/* Pipes (spec section 10, "Kernel files"), after xv6's pipe.c: a bounded
 * buffer of PIPESIZE bytes between two open files, the read end and the
 * write end. A reader sleeps while the buffer is empty and a writer while
 * it is full; each wakes the other as it makes progress. When every write
 * end is closed, a read of an empty pipe returns 0 (the end of the input);
 * when every read end is closed, a write fails with -EPIPE. */
#ifndef CAMELOT_PIPE_H
#define CAMELOT_PIPE_H

#include "file.h"
#include "types.h"
#include "vm.h"

#define PIPESIZE 512

/* Makes a pipe and its two ends: *rf the read end, *wf the write end.
 * Returns 0, or -ENFILE (the open file table is full) or -ENOMEM, having
 * allocated nothing. */
int pipealloc(struct file **rf, struct file **wf);

/* Closes the read end (`writable` false) or the write end of `p`, waking
 * whoever waits on the other one; frees the pipe once both are closed. */
void pipeclose(struct pipe *p, bool writable);

/* Reads up to `n` bytes (n > 0) to user address `dst`, sleeping while the
 * pipe is empty and a write end is open: returns what was there, at most
 * `n` bytes, or 0 once it is empty with no writer left. `dst` has been
 * checked. */
isize piperead(struct pipe *p, pagetable_t pt, uintptr dst, usize n);

/* Writes the `n` bytes at user address `src`, sleeping whenever the pipe
 * is full. Returns `n`; or, once no read end is open, the bytes written so
 * far, or -EPIPE if there were none. `src` has been checked. */
isize pipewrite(struct pipe *p, pagetable_t pt, uintptr src, usize n);

/* The bytes waiting in `p` (fstat's size). */
u32 pipe_bytes(struct pipe *p);

struct poll_table;

/* poll() on an end of `p` (the write end if `writable`): the read end is
 * ready to read with bytes waiting and hung up (POLLHUP) once the write
 * end is closed; the write end ready to write with room, and in error
 * (POLLERR) once the read end is closed, as on Linux. */
u32 pipe_poll(struct pipe *p, bool writable, struct poll_table *pt);

#endif
