/* A kernel thread's saved registers, for swtch() (swtch.S): what the
 * scheduler loop of each hart (struct cpu, spinlock.h) and each thread's
 * kernel side (struct thread, proc.h) keep while another thread runs. */
#ifndef CAMELOT_CONTEXT_H
#define CAMELOT_CONTEXT_H

#include "types.h"

/* What swtch() saves of a kernel thread: the return address, the stack
 * pointer and the callee-saved registers, which is all a function call has
 * to preserve, each a register wide (XLEN bits). swtch.S uses the same
 * offsets, in registers (ra at 0, sp at 1, s0-s11 from 2, times SZREG):
 * keep them in sync. */
struct context {
    uintptr ra;
    uintptr sp;
    uintptr s[12];
};

/* Saves the callee-saved registers in `old` and resumes `new` (swtch.S). */
void swtch(struct context *old, struct context *new);

#endif
