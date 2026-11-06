/* The framebuffer device, /dev/fb0 (fb.h): its memory, for mmap. */
#include "fb.h"
#include "memlayout.h"
#include "syscall.h"
#include "sysinfo.h"

isize fb_read(pagetable_t pt, uintptr dst, usize n) {
    (void)pt;
    (void)dst;
    (void)n;
    return -EINVAL;
}

isize fb_write(pagetable_t pt, uintptr src, usize n) {
    (void)pt;
    (void)src;
    (void)n;
    return -EINVAL;
}

int fb_mmap(u64 offset, usize size, phys_addr_t *pa) {
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
