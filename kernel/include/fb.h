/* The framebuffer device, /dev/fb0 (major FB, file.h; spec section 10,
 * "Kernel memory mappings"): so far the memory a program maps to draw
 * into, Linux's fbdev mmap of its screen memory. It maps the display's
 * built-in VRAM (Grail's framebuffer, spec section 9.3: 640x480, 32 bits a
 * pixel), the buffer the display scans out after reset; reads and writes
 * of the device itself are -EINVAL. */
#ifndef CAMELOT_FB_H
#define CAMELOT_FB_H

#include "types.h"
#include "vm.h"

/* devsw's functions for FB. fb_mmap() gives the VRAM's pages from
 * `offset`: -EINVAL if [offset, offset + size) goes past it, -ENODEV on a
 * machine without one. */
isize fb_read(pagetable_t pt, uintptr dst, usize n);
isize fb_write(pagetable_t pt, uintptr src, usize n);
int fb_mmap(u64 offset, usize size, phys_addr_t *pa);

#endif
