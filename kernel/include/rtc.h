/* The clocks (spec section 9.9, docs/camelot-syscalls.md "clock_gettime"):
 * CLOCK_MONOTONIC, the time since boot from `time` (mtime) at the
 * timebase, and CLOCK_REALTIME, the date, which the real-time clock gives
 * once at boot and the monotonic clock keeps from then on, as Linux sets
 * its system time from the RTC at boot and then keeps it with its
 * clocksource. */
#ifndef CAMELOT_RTC_H
#define CAMELOT_RTC_H

#include "types.h"

#define NSEC_PER_SEC 1000000000ull

/* Finds the real-time clock in the sysinfo table and reads the date from
 * it, printing it (`rtc: 2026-01-01 00:00 UTC`). Without one the date is
 * the time since boot, from 1970-01-01, as on Linux with no RTC. */
void rtc_init(void);

/* Nanoseconds since boot. */
u64 clock_monotonic_ns(void);

/* Nanoseconds since 1970-01-01 00:00:00 UTC. */
u64 clock_realtime_ns(void);

/* A date in days since 1970-01-01 as its year, month (1 to 12) and day (1
 * to 31): Howard Hinnant's civil_from_days, for days from 1970 on. */
void civil_from_days(u64 days, u32 *year, u32 *month, u32 *day);

#endif
