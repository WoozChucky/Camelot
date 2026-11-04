/* The M-mode layer (spec section 10, "Kernel traps"): start() sets the machine
 * up and drops to S-mode; afterwards machine_vec (machine.s) handles the only
 * traps that still reach M-mode: the machine timer, inter-processor
 * interrupts and machine calls. */
#ifndef CAMELOT_MACHINE_H
#define CAMELOT_MACHINE_H

/* Assembly includes this file too (machine.S), for the machine calls and the
 * scratch block's layout: the C is under #ifndef __ASSEMBLER__ (const.h). */
#ifndef __ASSEMBLER__
#include "types.h"
#endif

/* Machine calls: `ecall` from S-mode with the function number in a7 and the
 * arguments in a0-a6. Unknown functions return -1 in a0. */
#define MCALL_HALT 0 /* stop the machine, exit code a0 (xcal.halt is M-only) */
#define MCALL_IPI 1  /* interrupt the harts whose bits are set in a0 (msip = 1); returns 0 */

/* Layout of each hart's block of machine_scratch, which its mscratch points
 * at: slots a register wide (XLEN bits), since machine_vec saves registers
 * in them and holds an address. machine.S uses these slot numbers, times
 * SZREG. The tick slot is read and swapped as a u32, its low half on
 * RV64. */
#define MSCRATCH_SAVE0 0    /* slots 0-2: a1-a3 while machine_vec runs */
#define MSCRATCH_INTERVAL 3 /* timer interval in mtime ticks */
#define MSCRATCH_MTIMECMP 4 /* the physical address of the hart's mtimecmp in the CLINT */
#define MSCRATCH_TICK 5     /* 1 when a timer tick came with the last SSIP; S-mode swaps in 0 */
#define MSCRATCH_WORDS 8

/* NCPU blocks of MSCRATCH_WORDS slots: hart i's at i * MSCRATCH_WORDS (so
 * hart 0's comes first, at the symbol). M-mode does not translate addresses:
 * mscratch holds the block's physical address, and machine_vec reaches the
 * CLINT at its physical address too. S-mode reaches the same memory at the
 * symbol's (virtual) address. */
#ifndef __ASSEMBLER__
extern uintptr machine_scratch[];

/* M-mode setup, called from _start (entry.s) on every hart, on its own
 * stack. Returns with mepc, mstatus.MPP and the trap CSRs set up; _start's
 * `mret` then enters S-mode. */
void start(void);

/* Where _start's mret lands (entry.S), in S-mode with paging off: turns
 * paging on (on RV64: with the early page table, then jumps to the kernel's
 * high addresses), then on hart 0 calls kmain(), then machine_halt() with
 * its return value; on the others, hart_main() (smp.h). */
void supervisor_entry(void);

/* M-mode trap vector (machine.S). */
void machine_vec(void);

/* S-mode: MCALL_HALT. Never returns. */
void machine_halt(u32 code);

/* S-mode: MCALL_IPI, a machine-mode software interrupt to every hart whose
 * bit is set in `harts`, which machine_vec there turns into a supervisor
 * software interrupt (spec section 10, "Kernel SMP"). */
void machine_ipi(u32 harts);
#endif

#endif
