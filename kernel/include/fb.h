/* The framebuffer device, /dev/fb0 (major FB, file.h; spec section 10,
 * "Kernel memory mappings"): so far the memory a program maps to draw
 * into, Linux's fbdev mmap of its screen memory. It maps the display's
 * built-in VRAM (Grail's framebuffer, spec section 9.3: 640x480, 32 bits a
 * pixel), the buffer the display scans out after reset; reads and writes
 * of the device itself are -EINVAL (devsw has neither). */
#ifndef CAMELOT_FB_H
#define CAMELOT_FB_H

#include "types.h"
#include "vm.h"

struct file;

/* devsw's functions for FB. fb_mmap() gives the VRAM's pages from
 * `offset`: -EINVAL if [offset, offset + size) goes past it, -ENODEV on a
 * machine without one. fb_ioctl() knows no request yet: -ENOTTY. */
long fb_ioctl(struct file *f, u32 request, uintptr arg);
int fb_mmap(struct file *f, u64 offset, usize size, phys_addr_t *pa);

#endif
