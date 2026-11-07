/* The real-time clock driver and the clocks (rtc.h). The device is QEMU
 * virt's Goldfish RTC (spec section 9.9): a 64-bit count of nanoseconds
 * since the epoch, read low half first, which latches the high half. It is
 * read once, at boot; the date then runs on `time`, so the two clocks never
 * disagree about how much time has passed. */
#include "rtc.h"
#include "kprintf.h"
#include "mmio.h"
#include "riscv.h"
#include "sysinfo.h"
#include "timer.h"
#include "vm.h"

#define RTC_TIME_LOW 0x00
#define RTC_TIME_HIGH 0x04

/* The date at `time` 0, in nanoseconds since the epoch: 0 until
 * rtc_init() reads the clock, and without one. */
static u64 boot_realtime_ns;

u64 clock_monotonic_ns(void) {
    /* In two parts, so that nothing overflows: whole seconds, then the
     * rest of a second (below the timebase, a u32, so times 10^9 fits in
     * 64 bits). */
    u64 t = r_time();
    u64 hz = timebase_hz();
    return t / hz * NSEC_PER_SEC + t % hz * NSEC_PER_SEC / hz;
}

u64 clock_realtime_ns(void) {
    return boot_realtime_ns + clock_monotonic_ns();
}

void civil_from_days(u64 days, u32 *year, u32 *month, u32 *day) {
    /* Counted from 0000-03-01, so that a year ends with its leap day. */
    u64 z = days + 719468;
    u64 era = z / 146097;
    u64 doe = z - era * 146097;
    u64 yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    u64 doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    u64 mp = (5 * doy + 2) / 153;
    u64 m = mp < 10 ? mp + 3 : mp - 9;
    *year = (u32)(yoe + era * 400 + (m <= 2 ? 1 : 0));
    *month = (u32)m;
    *day = (u32)(doy - (153 * mp + 2) / 5 + 1);
}

void rtc_init(void) {
    u32 n = sysinfo_device_count();
    struct sysinfo_device dev;
    bool found = false;
    for (u32 i = 0; i < n && !found; i++) {
        sysinfo_device(i, &dev);
        found = dev.type == DEV_RTC;
    }
    if (!found) {
        kprintf("rtc: none\n");
        return;
    }
    uintptr regs = ioremap(dev.base, dev.size);
    u64 low = mmio_read32(regs + RTC_TIME_LOW);
    u64 high = mmio_read32(regs + RTC_TIME_HIGH);
    u64 now = high << 32 | low;
    boot_realtime_ns = now - clock_monotonic_ns();
    u64 secs = now / NSEC_PER_SEC;
    u32 year;
    u32 month;
    u32 day;
    civil_from_days(secs / 86400, &year, &month, &day);
    u32 minutes = (u32)(secs % 86400 / 60);
    kprintf("rtc: %u-%02u-%02u %02u:%02u UTC\n", year, month, day, minutes / 60, minutes % 60);
}
