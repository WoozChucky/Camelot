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
int piperead(struct pipe *p, pagetable_t pt, u32 dst, u32 n);

/* Writes the `n` bytes at user address `src`, sleeping whenever the pipe
 * is full. Returns `n`; or, once no read end is open, the bytes written so
 * far, or -EPIPE if there were none. `src` has been checked. */
int pipewrite(struct pipe *p, pagetable_t pt, u32 src, u32 n);

/* The bytes waiting in `p` (fstat's size). */
u32 pipe_bytes(const struct pipe *p);

#endif
