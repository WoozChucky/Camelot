/* Grail's sysinfo block (spec section 9.5): read-only 32-bit registers that
 * describe the machine, so the kernel discovers RAM and its devices instead of
 * hard-coding them. */
#ifndef CAMELOT_SYSINFO_H
#define CAMELOT_SYSINFO_H

#include "types.h"

#define SYSINFO_BASE 0x30002000

/* Register offsets. */
#define SYS_MAGIC 0x00
#define SYS_VERSION 0x04
#define SYS_RAM_BASE 0x08
#define SYS_RAM_SIZE 0x0c
#define SYS_TIMEBASE_HZ 0x10
#define SYS_CPU_HZ 0x14
#define SYS_FLAGS 0x18
#define SYS_DEV_COUNT 0x1c
#define SYS_DEV_TABLE 0x40

#define SYSINFO_MAGIC 0x53595349 /* "SYSI" */
/* The layout this kernel knows (SYS_VERSION). */
#define SYSINFO_VERSION 1

/* Device table entry i: four words at SYS_DEV_TABLE + 16 * i. */
#define SYS_DEV_ENTRY_SIZE 16
#define DEV_TYPE 0x0
#define DEV_BASE 0x4
#define DEV_SIZE 0x8
#define DEV_IRQ 0xc

/* Device type codes. */
#define DEV_CLINT 1
#define DEV_PLIC 2
#define DEV_UART 3
#define DEV_TEST 4
#define DEV_FB_CTRL 5
#define DEV_FB_VRAM 6
#define DEV_SYSINFO 7
#define DEV_KEYBOARD 8

struct sysinfo_device {
    u32 type;
    u32 base;
    u32 size;
    u32 irq;
};

/* Reads register `off`. */
u32 sysinfo_read(u32 off);

/* Panics unless the block is present (SYS_MAGIC) with the layout this kernel
 * knows (SYS_VERSION). */
void sysinfo_check(void);

/* Number of entries in the device table. */
u32 sysinfo_device_count(void);

/* Reads device table entry `i` (< sysinfo_device_count()) into `dev`. */
void sysinfo_device(u32 i, struct sysinfo_device *dev);

#endif
