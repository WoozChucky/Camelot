/* A kernel thread's saved registers, for swtch() (swtch.s): what the
 * scheduler loop of each hart (struct cpu, spinlock.h) and each process's
 * kernel thread (struct proc, proc.h) keep while another thread runs. */
#ifndef CAMELOT_CONTEXT_H
#define CAMELOT_CONTEXT_H

#include "types.h"

/* What swtch() saves of a kernel thread: the return address, the stack
 * pointer and the callee-saved registers, which is all a function call has
 * to preserve. swtch.s uses the same offsets (ra at 0, sp at 4, s0-s11 from
 * 8): keep them in sync. */
struct context {
    u32 ra;
    u32 sp;
    u32 s[12];
};

/* Saves the callee-saved registers in `old` and resumes `new` (swtch.s). */
void swtch(struct context *old, struct context *new);

#endif
