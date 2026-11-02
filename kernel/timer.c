/* The timer tick: the timebase, the tick interval and arming the CLINT. */
#include "timer.h"
#include "clint.h"
#include "machine.h"
#include "mmio.h"
#include "spinlock.h"
#include "sysinfo.h"

/* The timebase, read once from sysinfo (0 until then). */
static u32 timebase;

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
     * rearming with 0, which would fire continuously. */
    return interval != 0 ? interval : 1;
}

void timer_init(void) {
    u32 hart = r_tp();
    u32 interval = timer_interval();
    u32 *scratch = &machine_scratch[hart * MSCRATCH_WORDS];
    scratch[MSCRATCH_INTERVAL] = interval;
    scratch[MSCRATCH_MTIMECMP] = CLINT_MTIMECMP_LO(hart);

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
    mmio_write32(CLINT_MTIMECMP_HI(hart), 0xffffffffu);
    mmio_write32(CLINT_MTIMECMP_LO(hart), cmp_lo);
    mmio_write32(CLINT_MTIMECMP_HI(hart), cmp_hi);
}
