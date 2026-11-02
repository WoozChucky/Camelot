/* Reading Grail's sysinfo block (spec section 9.5). Works with paging off and
 * on: the kernel page table maps the block at its physical address. */
#include "sysinfo.h"
#include "mmio.h"
#include "trap.h"

u32 sysinfo_read(u32 off) {
    return mmio_read32(SYSINFO_BASE + off);
}

void sysinfo_check(void) {
    if (sysinfo_read(SYS_MAGIC) != SYSINFO_MAGIC) {
        panic("no sysinfo block at 0x30002000");
    }
    if (sysinfo_read(SYS_VERSION) != SYSINFO_VERSION) {
        panic("sysinfo: unknown layout version");
    }
}

u32 sysinfo_device_count(void) {
    return sysinfo_read(SYS_DEV_COUNT);
}

void sysinfo_device(u32 i, struct sysinfo_device *dev) {
    u32 entry = SYS_DEV_TABLE + i * SYS_DEV_ENTRY_SIZE;
    dev->type = sysinfo_read(entry + DEV_TYPE);
    dev->base = sysinfo_read(entry + DEV_BASE);
    dev->size = sysinfo_read(entry + DEV_SIZE);
    dev->irq = sysinfo_read(entry + DEV_IRQ);
}
