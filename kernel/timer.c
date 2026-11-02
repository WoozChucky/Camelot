/* The timer tick: the timebase, the tick interval and arming the CLINT. */
#include "timer.h"
#include "clint.h"
#include "machine.h"
#include "mmio.h"
#include "sysinfo.h"

/* The timebase, read once from sysinfo (0 until then). */
static u32 timebase;

u32 timebase_hz(void) {
    if (timebase == 0) {
        u32 hz = 0;
        if (mmio_read32(SYS_MAGIC) == SYSINFO_MAGIC) {
            hz = mmio_read32(SYS_TIMEBASE_HZ);
        }
        timebase = hz != 0 ? hz : TIMEBASE_HZ_NOMINAL;
    }
    return timebase;
}

u32 timer_interval(void) {
    u32 interval = timebase_hz() / TICK_HZ;
    /* A timebase below TICK_HZ: tick as often as the timer can rather than
     * rearming with 0, which would fire continuously. */
    return interval != 0 ? interval : 1;
}

void timer_init(void) {
    u32 interval = timer_interval();
    machine_scratch[MSCRATCH_INTERVAL] = interval;

    /* Read the 64-bit mtime as two halves: retry if the low word wrapped
     * between the reads. */
    u32 hi;
    u32 lo;
    do {
        hi = mmio_read32(CLINT_MTIME_HI);
        lo = mmio_read32(CLINT_MTIME_LO);
    } while (mmio_read32(CLINT_MTIME_HI) != hi);

    /* mtimecmp = mtime + interval. Park the high word at its maximum while
     * the low one changes, so no intermediate value can be in the past. */
    u32 cmp_lo = lo + interval;
    u32 cmp_hi = hi + (cmp_lo < lo ? 1 : 0);
    mmio_write32(CLINT_MTIMECMP_HI, 0xffffffffu);
    mmio_write32(CLINT_MTIMECMP_LO, cmp_lo);
    mmio_write32(CLINT_MTIMECMP_HI, cmp_hi);
}
