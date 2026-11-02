/* The timer tick (spec section 10, "Kernel traps"). The machine timer fires
 * TICK_HZ times a second; machine_vec forwards each interrupt to S-mode as a
 * software interrupt and kernel_trap counts it in `ticks`. */
#ifndef CAMELOT_TIMER_H
#define CAMELOT_TIMER_H

#include "types.h"

/* Nominal frequency of mtime (spec section 7). Grail advances mtime once per
 * instruction and doesn't pace execution by default, so this is a convention
 * software agrees on rather than wall-clock time. */
#define TIMEBASE_HZ_NOMINAL 10000000

/* Timer interrupts per second. */
#define TICK_HZ 10

/* The mtime frequency the kernel derives every interval from. It is the one
 * place that knows the timebase: once Grail's sysinfo device reports the real
 * frequency (#31), this reads it from there instead of returning the nominal
 * constant. */
u32 timebase_hz(void);

/* mtime ticks between two timer interrupts: timebase_hz() / TICK_HZ. */
u32 timer_interval(void);

/* M-mode: arms the first timer interrupt one interval from now and stores the
 * interval in machine_scratch, where machine_vec rearms from. */
void timer_init(void);

#endif
