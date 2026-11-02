/* The timer tick (spec section 10, "Kernel traps"). The machine timer fires
 * TICK_HZ times a second; machine_vec forwards each interrupt to S-mode as a
 * software interrupt and kernel_trap counts it in `ticks`. */
#ifndef CAMELOT_TIMER_H
#define CAMELOT_TIMER_H

#include "types.h"

/* Fallback frequency of mtime, should sysinfo be missing: the nominal clock
 * Grail reports when it runs at full speed (spec section 7). */
#define TIMEBASE_HZ_NOMINAL 10000000

/* Timer interrupts per second. */
#define TICK_HZ 10

/* The mtime frequency the kernel derives every interval from: SYS_TIMEBASE_HZ
 * from sysinfo (spec section 9.5), read on the first call (timer_init, in
 * M-mode) and remembered, or TIMEBASE_HZ_NOMINAL if SYS_MAGIC is wrong or
 * SYS_TIMEBASE_HZ reads 0. */
u32 timebase_hz(void);

/* mtime ticks between two timer interrupts: timebase_hz() / TICK_HZ, at
 * least 1. */
u32 timer_interval(void);

/* M-mode, on each hart: arms the hart's first timer interrupt one interval
 * from now and stores the interval and the address of the hart's mtimecmp
 * in its block of machine_scratch, where machine_vec rearms from. */
void timer_init(void);

#endif
