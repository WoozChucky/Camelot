/* The framebuffer device, /dev/fb0 (fb.h): its memory, for mmap. */
#include "fb.h"
#include "memlayout.h"
#include "syscall.h"
#include "sysinfo.h"

long fb_ioctl(struct file *f, u32 request, uintptr arg) {
    (void)f;
    (void)request;
    (void)arg;
    return -ENOTTY;
}

int fb_mmap(struct file *f, u64 offset, usize size, phys_addr_t *pa) {
    (void)f;
    /* The VRAM, as the device table lists it (spec section 9.5). */
    u32 n = sysinfo_device_count();
    for (u32 i = 0; i < n; i++) {
        struct sysinfo_device dev;
        sysinfo_device(i, &dev);
        if (dev.type != DEV_FB_VRAM) {
            continue;
        }
        if (dev.base % PGSIZE != 0) {
            return -ENODEV; /* not something pages can map */
        }
        /* Whole pages: the VRAM starts on one, and its last is mapped
         * whole, as Linux's fb_mmap rounds smem_len up. */
        u64 len = PGROUNDUP(dev.size);
        if (offset > len || size > len - offset) {
            return -EINVAL;
        }
        *pa = dev.base + offset;
        return 0;
    }
    return -ENODEV;
}
