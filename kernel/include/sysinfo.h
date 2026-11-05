/* Grail's sysinfo block (spec section 9.5): read-only registers that describe
 * the machine, so the kernel discovers RAM and its devices instead of
 * hard-coding them. Version 2 of its layout, whose addresses and sizes are
 * 64-bit (RAM's base and size, the device table's base and size). */
#ifndef CAMELOT_SYSINFO_H
#define CAMELOT_SYSINFO_H

#ifndef __ASSEMBLER__
#include "types.h"
#endif

#define SYSINFO_BASE 0x30002000

/* The block's size: one page. */
#define SYSINFO_SIZE 0x1000

/* Register offsets. SYS_RAM_BASE and SYS_RAM_SIZE are a register wide:
 * read them with sysinfo_read_reg(). */
#define SYS_MAGIC 0x00
#define SYS_VERSION 0x04
#define SYS_RAM_BASE 0x08
#define SYS_RAM_SIZE 0x10
#define SYS_TIMEBASE_HZ 0x18
#define SYS_CPU_HZ 0x1c
#define SYS_FLAGS 0x20
#define SYS_DEV_COUNT 0x24
#define SYS_HARTS 0x28 /* number of harts, 1 to 8 (spec section 7, "Harts") */
/* The RAM the firmware keeps for itself (spec section 7, "Firmware"; the
 * device tree's /reserved-memory): 0x8000_0000 and 2 MiB under Grail's
 * --bios, a size of 0 without firmware. */
#define SYS_RESERVED_BASE 0x30
#define SYS_RESERVED_SIZE 0x38
#define SYS_DEV_TABLE 0x40

#define SYSINFO_MAGIC 0x53595349 /* "SYSI" */
/* The layout this kernel knows (SYS_VERSION). */
#define SYSINFO_VERSION 2
/* Device table entry i, at SYS_DEV_TABLE + 24 * i: the type and the PLIC
 * source (32-bit), then the base and the size (64-bit). */
#define SYS_DEV_ENTRY_SIZE 24
#define DEV_TYPE 0x0
#define DEV_IRQ 0x4
#define DEV_BASE 0x8
#define DEV_SIZE 0x10

/* Device type codes. */
#define DEV_CLINT 1
#define DEV_PLIC 2
#define DEV_UART 3
#define DEV_TEST 4
#define DEV_FB_CTRL 5
#define DEV_FB_VRAM 6
#define DEV_SYSINFO 7
#define DEV_KEYBOARD 8
#define DEV_BLOCK 9 /* a disk, one entry each (spec section 9.6) */
#define DEV_POINTER 10
#define DEV_NET 11 /* a network card, one entry each (spec section 9.8) */

#ifndef __ASSEMBLER__
struct sysinfo_device {
    u32 type;
    phys_addr_t base; /* physical: ioremap() gives the kernel's address */
    usize size;
    u32 irq;
};

/* Where the block's registers are reached: SYSINFO_BASE, its physical
 * address (as setup_vm() reads it, with paging off); the fixmap's page from
 * setup_vm() on (vm.h); then its ioremap() mapping (sysinfo_map()). */
extern uintptr sysinfo_regs;

/* Points sysinfo_regs at the block's ioremap() mapping, once kvminit() has
 * made ioremap() work. */
void sysinfo_map(void);

/* Reads 32-bit register `off`. */
u32 sysinfo_read(u32 off);

/* Reads register `off`, one a register wide (SYS_RAM_BASE, SYS_RAM_SIZE): 64
 * bits in version 2, 32 in version 1. */
usize sysinfo_read_reg(u32 off);

/* Panics unless the block is present (SYS_MAGIC) with the layout this kernel
 * knows (SYS_VERSION). */
void sysinfo_check(void);

/* Number of entries in the device table. */
u32 sysinfo_device_count(void);

/* Reads device table entry `i` (< sysinfo_device_count()) into `dev`. */
void sysinfo_device(u32 i, struct sysinfo_device *dev);
#endif

#endif
