/* The timer tick: the timebase, the tick interval and arming the CLINT. */
#include "timer.h"
#include "clint.h"
#include "machine.h"
#include "mmio.h"

u32 timebase_hz(void) {
    /* Nominal for now: switch to Grail's sysinfo device once it exists (#31). */
    return TIMEBASE_HZ_NOMINAL;
}

u32 timer_interval(void) {
    return timebase_hz() / TICK_HZ;
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
