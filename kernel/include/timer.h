/* The timer tick (spec section 10, "Kernel traps"). Each hart has its own:
 * the supervisor timer interrupt, TICK_HZ times a second, which the hart
 * arms through the SBI's set_timer (include/sbi.h) and kernel_trap counts
 * (trap.c). */
#ifndef CAMELOT_TIMER_H
#define CAMELOT_TIMER_H

#include "types.h"

/* Fallback frequency of mtime, should sysinfo be missing: the nominal clock
 * Grail reports when it runs at full speed (spec section 7). */
#define TIMEBASE_HZ_NOMINAL 10000000

/* Timer interrupts per second. */
#define TICK_HZ 10

/* The `time` frequency the kernel derives every interval from:
 * SYS_TIMEBASE_HZ from sysinfo (spec section 9.5), read on the first call
 * and remembered, or TIMEBASE_HZ_NOMINAL if SYS_MAGIC is wrong or
 * SYS_TIMEBASE_HZ reads 0. */
u32 timebase_hz(void);

/* `time` ticks between two timer interrupts: timebase_hz() / TICK_HZ, at
 * least 1. */
u32 timer_interval(void);

/* This hart's first tick, one interval from now (trap_init() on hart 0,
 * hart_main() on the others, with sie.STIE set). */
void timer_start(void);

/* At a tick: this hart's next one, one interval after the deadline of the
 * one being handled. set_timer also clears the pending supervisor timer
 * interrupt, which S-mode cannot clear itself. */
void timer_next(void);

#endif
