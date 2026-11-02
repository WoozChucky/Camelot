/* Driver for Grail's block devices (spec section 9.6; section 10, "Kernel
 * disks"). Disks are found in the sysinfo device table. Each takes one DMA
 * request at a time: the calling process sleeps until the disk's completion
 * interrupt wakes it, and gets the device's BLK_ERROR code back. */
#ifndef CAMELOT_BLK_H
#define CAMELOT_BLK_H

#include "types.h"

#define BLK_SECTOR_SIZE 512

/* The most disks a machine has (spec section 9.6). */
#define BLK_MAX_DISKS 8

/* Register offsets in a disk's page. */
#define BLK_CAPACITY 0x00 /* size in sectors (read-only) */
#define BLK_FLAGS 0x04    /* bit 0: the disk is read-only */
#define BLK_SECTOR 0x08   /* first sector of the transfer */
#define BLK_DMA 0x0c      /* physical address of the buffer */
#define BLK_COUNT 0x10    /* sectors to transfer */
#define BLK_CMD 0x14      /* writing a command runs it */
#define BLK_STATUS 0x18   /* bit 0 BUSY, bit 1 DONE (write 1 to clear) */
#define BLK_ERROR 0x1c    /* outcome of the last command */
#define BLK_CTRL 0x20     /* bit 0: completion interrupt enable */

#define BLK_FLAG_READ_ONLY 0x1
#define BLK_CMD_READ 1
#define BLK_CMD_WRITE 2
#define BLK_CMD_FLUSH 3
#define BLK_STATUS_DONE 0x2
#define BLK_CTRL_IRQ 0x1

/* BLK_ERROR codes: what the blk_ and bio functions return. */
#define BLK_OK 0
#define BLK_ERR_BAD_COMMAND 1
#define BLK_ERR_OUT_OF_RANGE 2 /* past the end of the disk */
#define BLK_ERR_BAD_DMA 3      /* the buffer is not all in RAM */
#define BLK_ERR_READ_ONLY 4    /* a write to a read-only disk */
#define BLK_ERR_IO 5           /* the host's backend failed */

/* Finds the disks (sysinfo entries of type DEV_BLOCK, in table order: disk 0
 * is the first), turns their completion interrupts on and enables their
 * PLIC sources. Call after trap_init(). */
void blk_init(void);

/* Number of disks found. Disks are numbered from 0. */
u32 blk_count(void);

/* Disk `dev`'s size in sectors, and whether it is read-only. */
u32 blk_capacity(u32 dev);
bool blk_read_only(u32 dev);

/* Disk `dev`'s PLIC source. */
u32 blk_irq(u32 dev);

/* Transfers `count` sectors from sector `sector` of disk `dev` into `buf`
 * (blk_read) or from `buf` to the disk (blk_write). `buf` is a kernel
 * address, which is also its physical address (the kernel's mappings are an
 * identity map). Must be called by a process: it sleeps until the request is
 * done, and until any earlier one on the same disk is. Returns BLK_OK or the
 * device's BLK_ERR_ code; a failed request moves no data. Panics if `dev` is
 * not a disk. */
u32 blk_read(u32 dev, u32 sector, void *buf, u32 count);
u32 blk_write(u32 dev, u32 sector, const void *buf, u32 count);

/* Waits until every earlier write to disk `dev` is durable on the host. */
u32 blk_flush(u32 dev);

/* The interrupt handler, called by kernel_trap and user_trap for each PLIC
 * source they claim: if `irq` is a disk's, takes its result, acknowledges
 * BLK_STATUS.DONE (which lowers the line), wakes the process waiting for it
 * and returns true. Returns false for any other source. */
bool blk_intr(u32 irq);

#endif
