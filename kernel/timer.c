/* The timer tick: the timebase, the tick interval, and each hart's next
 * deadline, which the SBI's set_timer programs (include/sbi.h). */
#include "timer.h"
#include "riscv.h"
#include "sbi.h"
#include "spinlock.h"
#include "sysinfo.h"

/* The timebase, read once from sysinfo (0 until then), by hart 0's first
 * timer_start() in trap_init(). The other harts find it here. */
static u32 timebase;

/* Each hart's next tick, in `time` units: one interval after the one
 * before, rather than after whenever the tick was handled, so that the
 * ticks don't drift however long the handler takes to get there. Only the
 * hart itself touches its own. */
static u64 next_tick[NCPU];

u32 timebase_hz(void) {
    if (timebase == 0) {
        u32 hz = 0;
        if (sysinfo_read(SYS_MAGIC) == SYSINFO_MAGIC) {
            hz = sysinfo_read(SYS_TIMEBASE_HZ);
        }
        timebase = hz != 0 ? hz : TIMEBASE_HZ_NOMINAL;
    }
    return timebase;
}

u32 timer_interval(void) {
    u32 interval = timebase_hz() / TICK_HZ;
    /* A timebase below TICK_HZ: tick as often as the timer can rather than
     * set the next deadline to now, which would fire continuously. */
    return interval != 0 ? interval : 1;
}

void timer_start(void) {
    u32 hart = cpuid();
    next_tick[hart] = r_time() + timer_interval();
    sbi_set_timer(next_tick[hart]);
}

u32 timer_next(void) {
    u32 hart = cpuid();
    u32 interval = timer_interval();
    u64 now = r_time();
    /* Past every deadline that has gone by: those ticks were missed (the
     * hart had interrupts off that long, or a debugger stopped it), and
     * are counted, as Linux's periodic tick counts them; a deadline left
     * in the past would fire again at once, and never let the hart do
     * anything else. */
    u32 ticks = 0;
    do {
        next_tick[hart] += interval;
        ticks++;
    } while (next_tick[hart] <= now);
    sbi_set_timer(next_tick[hart]);
    return ticks;
}
