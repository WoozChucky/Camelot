/* Reading Grail's sysinfo block (spec section 9.5) through sysinfo_regs: its
 * physical address with paging off, the fixmap's page from setup_vm() on,
 * then the kernel's ioremap() of it (sysinfo_map()). */
#include "sysinfo.h"
#include "mmio.h"
#include "trap.h"
#include "vm.h"

uintptr sysinfo_regs = SYSINFO_BASE;

void sysinfo_map(void) {
    sysinfo_regs = ioremap(SYSINFO_BASE, SYSINFO_SIZE);
}

u32 sysinfo_read(u32 off) {
    return mmio_read32(sysinfo_regs + off);
}

usize sysinfo_read_reg(u32 off) {
    return mmio_read64(sysinfo_regs + off);
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
    dev->base = sysinfo_read_reg(entry + DEV_BASE);
    dev->size = sysinfo_read_reg(entry + DEV_SIZE);
    dev->irq = sysinfo_read(entry + DEV_IRQ);
}
