/* S-mode trap handling (spec section 10, "Kernel traps"). */
#ifndef CAMELOT_TRAP_H
#define CAMELOT_TRAP_H

#include "types.h"

/* What kernel_vec (kernelvec.s) saves on the stack. The offsets are a contract
 * with kernelvec.s: regs[i] at 4*i, sepc at 128, scause at 132, stval at 136,
 * sstatus at 140. kernel_vec writes sepc and sstatus back before sret, so a
 * handler may change where execution resumes. */
struct trap_frame {
    u32 regs[32]; /* x0-x31 as they were at the trap; regs[0] is always 0 */
    u32 sepc;
    u32 scause;
    u32 stval;
    u32 sstatus;
};

#define TRAP_FRAME_SIZE 144

/* Exit code after a kernel panic. (0xff means an unexpected M-mode trap.) */
#define PANIC_EXIT_CODE 0xfe

/* Timer ticks since boot, counted by kernel_trap. */
extern volatile u32 ticks;

/* S-mode trap vector; start() points stvec at it. */
void kernel_vec(void);

/* Called by kernel_vec with the saved registers. */
void kernel_trap(struct trap_frame *f);

/* Enables the interrupts the kernel handles (the forwarded timer tick) and
 * turns interrupts on in S-mode. */
void trap_init(void);

/* Name of a trap cause, e.g. "illegal instruction". `cause` is a raw scause
 * value, interrupt bit included. */
const char *trap_cause_name(u32 cause);

/* Prints "panic: <msg>" and stops the machine with PANIC_EXIT_CODE. */
void panic(const char *msg);

/* Executes an illegal instruction (unimp) after setting every register but sp
 * to its own number in each byte (x5 = 0x05050505), so a test can check the
 * panic's register dump. Never returns. */
void illegal_instruction(void);

#endif
