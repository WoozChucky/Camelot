/* Calls from U-mode: `ecall` with the function number in a7, the arguments
 * in a0 and a1, the result in a0 (spec section 10, "Kernel processes").
 *
 * These are provisional, enough for the first user programs to print and
 * exit; the system call ABI (#21) replaces them. kernel/user/lib.s uses the
 * same numbers. */
#ifndef CAMELOT_SYSCALL_H
#define CAMELOT_SYSCALL_H

#include "trap.h"

#define ECALL_EXIT 1        /* exit(status a0); never returns */
#define ECALL_DEBUG_WRITE 2 /* print a1 bytes at user address a0; returns a1, or -1 */

/* Handles the ecall in `f` (from U-mode, sepc already past it): runs the
 * function, puts the result in the frame's a0. Unknown numbers return -1. */
void syscall(struct trap_frame *f);

#endif
