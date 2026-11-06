/* The framebuffer device, /dev/fb0 (major FB, file.h; spec section 10,
 * "Framebuffer driver"), Linux's fbdev over Grail's display controller
 * (spec section 9.3): a program maps the screen's memory to draw into
 * (mmap), reads and sets the mode with FBIOGET_VSCREENINFO and
 * FBIOPUT_VSCREENINFO, flips between the two halves of a double-height
 * buffer with FBIOPAN_DISPLAY and waits for the vertical blank with
 * FBIO_WAITFORVSYNC. Reads and writes of the device itself are -EINVAL
 * (devsw has neither), as fbdev's are on no driver of Camelot's: the memory
 * is mapped, not read.
 *
 * The buffer: the display's built-in VRAM for 640x480, the mode at reset, a
 * single frame; a run of pages of RAM (kalloc_contig) for a bigger mode or a
 * double-height buffer, which the display scans out, as Linux's
 * simple-framebuffer and DRM's dumb buffers live in system memory. While a
 * program has the device open or mapped, it is the program's; when the last
 * open file and mapping go, the driver goes back to 640x480 in VRAM,
 * freeing any buffer in RAM, and gives the screen back to the console
 * (fbcon.h). */
#ifndef CAMELOT_FB_H
#define CAMELOT_FB_H

#include "types.h"
#include "vm.h"

struct file;

/* Linux's fbdev requests (include/uapi/linux/fb.h), the same numbers. */
#define FBIOGET_VSCREENINFO 0x4600
#define FBIOPUT_VSCREENINFO 0x4601
#define FBIOGET_FSCREENINFO 0x4602
#define FBIOPAN_DISPLAY 0x4606
#define FBIOBLANK 0x4611
#define FBIO_WAITFORVSYNC 0x40044620 /* _IOW('F', 0x20, __u32) */

/* fb_var_screeninfo.activate */
#define FB_ACTIVATE_NOW 0
#define FB_ACTIVATE_NXTOPEN 1
#define FB_ACTIVATE_TEST 2
#define FB_ACTIVATE_MASK 15
#define FB_ACTIVATE_VBL 16

/* fb_fix_screeninfo.type, .visual */
#define FB_TYPE_PACKED_PIXELS 0
#define FB_VISUAL_TRUECOLOR 2

/* FBIOBLANK's argument: 0 shows the picture, any of the others blanks it. */
#define FB_BLANK_UNBLANK 0
#define FB_BLANK_POWERDOWN 4

/* Where a colour's bits are in a pixel. */
struct fb_bitfield {
    u32 offset;
    u32 length;
    u32 msb_right;
};

/* The mode and the part of the buffer shown: Linux's, 160 bytes. */
struct fb_var_screeninfo {
    u32 xres; /* the mode */
    u32 yres;
    u32 xres_virtual; /* the buffer: xres, by yres or 2 * yres rows */
    u32 yres_virtual;
    u32 xoffset; /* the first row shown: 0, or a flip's */
    u32 yoffset;
    u32 bits_per_pixel; /* 32 */
    u32 grayscale;
    struct fb_bitfield red; /* XRGB8888: 16, 8 and 0, 8 bits each */
    struct fb_bitfield green;
    struct fb_bitfield blue;
    struct fb_bitfield transp; /* none: the top byte is ignored */
    u32 nonstd;
    u32 activate; /* FB_ACTIVATE_ */
    u32 height; /* the picture's size in mm: not known, 0xffffffff */
    u32 width;
    u32 accel_flags;
    /* The timings: the display has none to give, 0. */
    u32 pixclock;
    u32 left_margin;
    u32 right_margin;
    u32 upper_margin;
    u32 lower_margin;
    u32 hsync_len;
    u32 vsync_len;
    u32 sync;
    u32 vmode;
    u32 rotate;
    u32 colorspace;
    u32 reserved[4];
};

/* What doesn't change with the mode's settings but the buffer: Linux's, 80
 * bytes on LP64. */
struct fb_fix_screeninfo {
    char id[16]; /* "grail-fb" */
    u64 smem_start; /* the buffer's physical address */
    u32 smem_len; /* and its size */
    u32 type; /* FB_TYPE_PACKED_PIXELS */
    u32 type_aux;
    u32 visual; /* FB_VISUAL_TRUECOLOR */
    u16 xpanstep;
    u16 ypanstep; /* rows a pan moves by: a page's worth (FB_SCANOUT) */
    u16 ywrapstep;
    u32 line_length; /* bytes a row: xres * 4 */
    u64 mmio_start;
    u32 mmio_len;
    u32 accel;
    u16 capabilities;
    u16 reserved[2];
};

_Static_assert(sizeof(struct fb_var_screeninfo) == 160, "Linux's fb_var_screeninfo");
_Static_assert(sizeof(struct fb_fix_screeninfo) == 80, "Linux's fb_fix_screeninfo on LP64");

/* Finds the display controller in the device table (spec section 9.5),
 * maps it and VRAM, reads its modes and enables the picture: 640x480 from
 * VRAM, the console's (fbcon.h). Nothing on a machine without one. After
 * kvminit, before the first process. */
void fb_init(void);

/* Whether the machine has a display: fb_init() found one. */
bool fb_present(void);

/* The vertical-blank interrupt, PLIC source `irq`: whether it was the
 * display's (external_interrupt, trap.c). */
bool fb_intr(u32 irq);

/* devsw's functions for FB (file.h). fb_open() fails with -ENODEV on a
 * machine without a display. fb_mmap() gives the buffer's pages from
 * `offset`: -EINVAL if [offset, offset + size) goes past them. fb_ioctl()
 * answers the requests above, -ENOTTY to any other. */
int fb_open(struct file *f);
void fb_close(struct file *f);
long fb_ioctl(struct file *f, u32 request, uintptr arg);
int fb_mmap(struct file *f, u64 offset, usize size, phys_addr_t *pa);
void fb_vm_open(void);
void fb_vm_close(void);

/* The kernel's pointer to VRAM, 640x480 pixels of 4 bytes, or NULL without
 * a display: where the console draws (fbcon.c). */
u32 *fb_vram(void);

/* Tells the display a new frame is in VRAM (FB_PRESENT): Grail shows it at
 * once, and --fb-dump writes it. */
void fb_present_frame(void);

#endif
