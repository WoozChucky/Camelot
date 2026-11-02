/* The M-mode layer (spec section 10, "Kernel traps"): start() sets the machine
 * up and drops to S-mode; afterwards machine_vec (machine.s) handles the only
 * traps that still reach M-mode, the machine timer and machine calls. */
#ifndef CAMELOT_MACHINE_H
#define CAMELOT_MACHINE_H

#include "types.h"

/* Machine calls: `ecall` from S-mode with the function number in a7 and the
 * arguments in a0-a6. Unknown functions return -1 in a0. */
#define MCALL_HALT 0 /* stop the machine, exit code a0 (xcal.halt is M-only) */

/* Layout of machine_scratch, which mscratch points at. machine.s uses the same
 * offsets (as .equ constants): keep them in sync. */
#define MSCRATCH_SAVE0 0    /* words 0-2: a1-a3 while machine_vec runs */
#define MSCRATCH_INTERVAL 3 /* timer interval in mtime ticks */
#define MSCRATCH_WORDS 4

extern u32 machine_scratch[MSCRATCH_WORDS];

/* M-mode setup, called from _start (entry.s). Returns with mepc, mstatus.MPP
 * and the trap CSRs set up; _start's `mret` then enters S-mode. */
void start(void);

/* Where _start's mret lands (entry.s): calls kmain() in S-mode, then
 * machine_halt() with its return value. */
void supervisor_entry(void);

/* M-mode trap vector (machine.s). */
void machine_vec(void);

/* S-mode: MCALL_HALT. Never returns. */
void machine_halt(u32 code);

#endif
