/* Block device driver (spec section 10, "Kernel disks"), for Grail's disks
 * (spec section 9.6): one DMA request at a time per disk, completion by
 * interrupt.
 *
 * A request is made by a process (a user process in a system call, or a
 * kernel process such as the boot self-test): it writes the sector, the
 * buffer's address and the count, then the command, and sleeps. The disk
 * raises its PLIC source when the command is done; blk_intr() takes the
 * result, acknowledges it and wakes the process. Nothing polls the device.
 *
 * Locking: each disk has a spinlock for its request state (busy, done,
 * error) and its registers; the process waiting for the disk sleeps on the
 * disk with it, and blk_intr() takes it to post the result. The fields set
 * up by blk_init() never change afterwards and are read without it. */
#include "blk.h"
#include "mmio.h"
#include "plic.h"
#include "proc.h"
#include "spinlock.h"
#include "sysinfo.h"
#include "trap.h"
#include "vm.h"

/* The first field is the request state's lock, so the disk's address (what
 * the processes waiting for it sleep on) is also the lock's. */
struct disk {
    struct spinlock lock;
    uintptr base; /* its page of registers */
    u32 irq;      /* its PLIC source */
    u32 capacity; /* in sectors */
    bool read_only;
    /* A request is in flight: set by the process that made it until it has
     * taken the result. Anyone else wanting the disk sleeps until it clears. */
    bool busy;
    /* The completion interrupt for the request in flight has come, and
     * `error` holds its BLK_ERROR. Set by blk_intr(). */
    bool done;
    u32 error;
};

static struct disk disks[BLK_MAX_DISKS];
static u32 ndisks;

void blk_init(void) {
    ndisks = 0;
    u32 n = sysinfo_device_count();
    for (u32 i = 0; i < n && ndisks < BLK_MAX_DISKS; i++) {
        struct sysinfo_device dev;
        sysinfo_device(i, &dev);
        /* A disk without an interrupt source would have to be polled, which
         * this driver doesn't do. Grail gives every disk one. */
        if (dev.type != DEV_BLOCK || dev.irq == 0 || dev.irq > 31) {
            continue;
        }
        struct disk *d = &disks[ndisks];
        initlock(&d->lock, "disk");
        d->base = ioremap(dev.base, dev.size);
        d->irq = dev.irq;
        /* The kernel's sector numbers are 32-bit (the filesystem's block
         * numbers are): it uses at most 2^32 - 1 sectors of a larger disk. */
        u64 capacity = mmio_read64(d->base + BLK_CAPACITY);
        d->capacity = capacity > 0xffffffffull ? 0xffffffffu : (u32)capacity;
        d->read_only = (mmio_read32(d->base + BLK_FLAGS) & BLK_FLAG_READ_ONLY) != 0;
        d->busy = false;
        d->done = false;
        d->error = BLK_OK;
        /* Acknowledge a result nobody asked for (left by whatever ran before
         * the kernel), so the line is low when the interrupt is turned on. */
        mmio_write32(d->base + BLK_STATUS, BLK_STATUS_DONE);
        mmio_write32(d->base + BLK_CTRL, BLK_CTRL_IRQ);
        plic_enable(d->irq);
        ndisks++;
    }
}

u32 blk_count(void) {
    return ndisks;
}

static struct disk *disk(u32 dev) {
    if (dev >= ndisks) {
        panic("blk: no such disk");
    }
    return &disks[dev];
}

u32 blk_capacity(u32 dev) {
    return disk(dev)->capacity;
}

bool blk_read_only(u32 dev) {
    return disk(dev)->read_only;
}

u32 blk_irq(u32 dev) {
    return disk(dev)->irq;
}

/* Runs command `cmd` on disk `dev` and sleeps until its interrupt. */
static u32 request(u32 dev, u32 cmd, u32 sector, uintptr buf, u32 count) {
    struct disk *d = disk(dev);
    if (myproc() == NULL) {
        panic("blk: request outside a process");
    }
    acquire(&d->lock);
    /* One request at a time: wait for the one in flight to be taken. */
    while (d->busy) {
        sleep(d, &d->lock);
    }
    d->busy = true;
    d->done = false;
    /* The device wants the buffer's physical address (it does not use the
     * MMU): pa() of the kernel pointer. A command without data (a flush) has
     * no buffer, and 0 there. */
    phys_addr_t dma = buf != 0 ? pa(buf) : 0;
    mmio_write64(d->base + BLK_SECTOR, sector);
    mmio_write64(d->base + BLK_DMA, dma);
    mmio_write32(d->base + BLK_COUNT, count);
    mmio_write32(d->base + BLK_CMD, cmd);
    /* Grail finishes the command during that store, so its interrupt is
     * already pending; a real disk would take a while. Either way the process
     * sleeps, and the scheduler runs something else or idles in wfi until
     * blk_intr() wakes it. */
    while (!d->done) {
        sleep(d, &d->lock);
    }
    u32 err = d->error;
    d->busy = false;
    /* The next process waiting for the disk, if any. */
    wakeup(d);
    release(&d->lock);
    return err;
}

u32 blk_read(u32 dev, u32 sector, void *buf, u32 count) {
    return request(dev, BLK_CMD_READ, sector, (uintptr)buf, count);
}

u32 blk_write(u32 dev, u32 sector, const void *buf, u32 count) {
    return request(dev, BLK_CMD_WRITE, sector, (uintptr)buf, count);
}

u32 blk_flush(u32 dev) {
    return request(dev, BLK_CMD_FLUSH, 0, 0, 0);
}

bool blk_intr(u32 irq) {
    for (u32 i = 0; i < ndisks; i++) {
        struct disk *d = &disks[i];
        if (d->irq != irq) {
            continue;
        }
        acquire(&d->lock);
        d->error = mmio_read32(d->base + BLK_ERROR);
        /* Clearing DONE lowers the line, so the source isn't pending again
         * once the trap code completes it. */
        mmio_write32(d->base + BLK_STATUS, BLK_STATUS_DONE);
        d->done = true;
        wakeup(d);
        release(&d->lock);
        return true;
    }
    return false;
}
