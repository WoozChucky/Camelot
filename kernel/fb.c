/* The framebuffer device, /dev/fb0 (fb.h): Linux's fbdev requests over
 * Grail's display controller (spec section 9.3).
 *
 * The state is the mode, the buffer it scans out (VRAM, or pages of RAM),
 * the buffer's height (one frame or two) and the row shown from (the pan),
 * and who uses the device: open files and mappings of its memory. A mode
 * that needs a new buffer can't be set while the old one is mapped
 * (-EBUSY): a program would go on drawing into pages the display no longer
 * shows, or that went back to the free list.
 *
 * Locking, in order:
 *
 *   fb_mode_lock  a sleep lock: one request at a time changing the mode or
 *                 the pan; a new buffer is found and cleared under it,
 *                 outside fb_lock.
 *   fb_lock       a spinlock: the state and the counts. mmap's hooks take
 *                 it, under a process's vm_lock or under proc_lock (a
 *                 process reaped with the device still mapped, proc_free),
 *                 so nothing under it sleeps or wakes a sleeper, which
 *                 would take proc_lock, and frees nothing; only a leaf
 *                 comes after it, uart_tx_lock (fbcon_suspend,
 *                 fbcon_resume).
 *   vblank_lock   a spinlock: the vertical blank's count, its waiters and
 *                 the interrupt's enable; waiters sleep with it, so it
 *                 comes before proc_lock, and is never taken under fb_lock.
 *
 * A buffer in RAM that the last mapping's going lets go of may be let go
 * of under proc_lock: it is freed later, by fb_reclaim(), where no lock is
 * held (fb_close, the next mode change, proc_wait after a reap, and the
 * scheduler after each switch back, as it calls kstack_reclaim()), rather
 * than under locks that every hart's scheduler waits for. */
#include "fb.h"
#include "fbcon.h"
#include "file.h"
#include "kalloc.h"
#include "kprintf.h"
#include "memlayout.h"
#include "mmio.h"
#include "plic.h"
#include "poll.h"
#include "proc.h"
#include "sleeplock.h"
#include "spinlock.h"
#include "string.h"
#include "syscall.h"
#include "sysinfo.h"
#include "trap.h"

/* The display controller's registers (spec section 9.3). */
#define FB_WIDTH 0x00
#define FB_HEIGHT 0x04
#define FB_CTRL 0x10
#define FB_PRESENT 0x14
#define FB_VERSION 0x18
#define FB_MODES 0x1c
#define FB_MODE 0x20
#define FB_MODE_SEL 0x24
#define FB_MODE_WIDTH 0x28
#define FB_MODE_HEIGHT 0x2c
#define FB_STATUS 0x34
#define FB_SCANOUT 0x38
#define FB_FLIP 0x40
#define FB_IRQ 0x44

#define CTRL_ENABLE (1u << 0)
#define STATUS_FLIP_PENDING (1u << 0)
#define STATUS_BAD_MODE (1u << 1)
#define STATUS_BAD_SCANOUT (1u << 2)
#define STATUS_BAD_CURSOR (1u << 4)
#define IRQ_ENABLE (1u << 0)
#define IRQ_VBLANK (1u << 1)

/* Modes the driver keeps: Grail has 6. */
#define MAX_MODES 16

/* Bytes a pixel: XRGB8888 is the only format. */
#define BPP 4

static struct spinlock fb_lock;
static struct sleeplock fb_mode_lock;
static struct spinlock vblank_lock;

static uintptr regs;   /* the controller's registers (ioremap) */
static u32 *vram;      /* VRAM, the kernel's pointer */
static phys_addr_t vram_pa;
static u32 irq;        /* the vertical blank's PLIC source; 0: none */
static bool can_flip;  /* FB_VERSION >= 1: modes, FB_SCANOUT and flips */

static u32 nmodes;
static u32 mode_width[MAX_MODES];
static u32 mode_height[MAX_MODES];

/* The state, under fb_lock (and changed under fb_mode_lock too). */
static u32 cur_mode;
static u32 yres_virtual;  /* the buffer's rows: the mode's height, or twice it */
static u32 yoffset;       /* the first row shown */
static phys_addr_t smem_pa;
static u32 smem_len;      /* stride * yres_virtual */
static void *smem_ram;    /* the buffer in RAM (kalloc_contig); NULL: VRAM */
static usize smem_pages;
static bool blanked;

static u32 opens;      /* open files of the device */
static u32 maps;       /* areas of processes that map its memory */
/* A buffer in RAM the display let go of, for fb_reclaim() to free. */
static void *stale_ram;
static usize stale_pages;

/* Under vblank_lock. */
static u32 waiters;    /* threads waiting for a vertical blank */
static u32 vblank_seq; /* vertical blanks the interrupt has seen */

static u32 rd(u32 off) {
    return mmio_read32(regs + off);
}

static void wr(u32 off, u32 val) {
    mmio_write32(regs + off, val);
}

static u32 stride(u32 mode) {
    return mode_width[mode] * BPP;
}

/* What FB_SCANOUT takes for the buffer from row `row`: 0 is VRAM. */
static phys_addr_t scanout(u32 row) {
    return smem_ram == NULL ? 0 : smem_pa + (u64)row * stride(cur_mode);
}

/* Rows a pan moves by in `mode`: a page's worth, the least number of rows
 * whose bytes are a whole number of pages, since FB_SCANOUT takes only an
 * address on a page. */
static u32 panstep(u32 mode) {
    u32 s = stride(mode);
    u32 step = 1;
    while (s * step % PGSIZE != 0) {
        step = step * 2;
    }
    return step;
}

bool fb_present(void) {
    return regs != 0;
}

void fb_mode_size(u32 *width, u32 *height) {
    if (regs == 0) {
        *width = 0;
        *height = 0;
        return;
    }
    /* The display's own registers: what it shows, whoever set it. */
    *width = rd(FB_WIDTH);
    *height = rd(FB_HEIGHT);
}

u32 *fb_vram(void) {
    return vram;
}

void fb_present_frame(void) {
    if (regs != 0) {
        wr(FB_PRESENT, 1);
    }
}

void fb_init(void) {
    initlock(&fb_lock, "fb");
    initsleeplock(&fb_mode_lock, "fbmode");
    initlock(&vblank_lock, "vblank");
    struct sysinfo_device ctrl;
    struct sysinfo_device mem;
    bool have_ctrl = false;
    bool have_vram = false;
    u32 n = sysinfo_device_count();
    for (u32 i = 0; i < n; i++) {
        struct sysinfo_device dev;
        sysinfo_device(i, &dev);
        if (dev.type == DEV_FB_CTRL && !have_ctrl) {
            ctrl = dev;
            have_ctrl = true;
        } else if (dev.type == DEV_FB_VRAM && !have_vram) {
            mem = dev;
            have_vram = true;
        }
    }
    if (!have_ctrl || !have_vram || mem.base % PGSIZE != 0) {
        return;
    }
    regs = ioremap(ctrl.base, ctrl.size);
    vram = (u32 *)ioremap(mem.base, mem.size);
    vram_pa = mem.base;
    /* The modes, as a KMS client reads a connector's list: each number to
     * FB_MODE_SEL, its size back. A controller before them (FB_VERSION 0)
     * has the one it shows. */
    can_flip = rd(FB_VERSION) >= 1;
    if (can_flip) {
        u32 count = rd(FB_MODES);
        for (u32 i = 0; i < count && nmodes < MAX_MODES; i++) {
            wr(FB_MODE_SEL, i);
            mode_width[nmodes] = rd(FB_MODE_WIDTH);
            mode_height[nmodes] = rd(FB_MODE_HEIGHT);
            nmodes++;
        }
    }
    if (nmodes == 0) {
        mode_width[0] = rd(FB_WIDTH);
        mode_height[0] = rd(FB_HEIGHT);
        nmodes = 1;
    }
    if ((u64)mode_width[0] * mode_height[0] * BPP > mem.size) {
        regs = 0; /* VRAM doesn't hold the first mode: not a display to use */
        return;
    }
    /* Mode 0 from VRAM, as at reset, whatever the firmware did; the
     * interrupt off, and the picture on. */
    if (can_flip) {
        mmio_write64(regs + FB_SCANOUT, 0);
        wr(FB_MODE, 0);
        wr(FB_STATUS, STATUS_BAD_MODE | STATUS_BAD_SCANOUT | STATUS_BAD_CURSOR);
        wr(FB_IRQ, IRQ_VBLANK);
    }
    cur_mode = 0;
    yres_virtual = mode_height[0];
    smem_pa = vram_pa;
    smem_len = stride(0) * mode_height[0];
    wr(FB_CTRL, CTRL_ENABLE);
    if (can_flip && ctrl.irq != 0 && ctrl.irq < 32) {
        irq = ctrl.irq;
        plic_enable(irq);
    }
    kprintf("fb: %ux%u, %u modes, irq %u\n", mode_width[0], mode_height[0], nmodes, irq);
    fbcon_init();
}

bool fb_intr(u32 source) {
    if (irq == 0 || source != irq) {
        return false;
    }
    acquire(&vblank_lock);
    /* Clearing VBLANK lowers the line; the interrupt stays on while
     * someone waits for the next one. */
    wr(FB_IRQ, waiters > 0 ? IRQ_ENABLE | IRQ_VBLANK : IRQ_VBLANK);
    vblank_seq++;
    release(&vblank_lock);
    /* A waiter checks the count under vblank_lock and is asleep before it
     * lets go of it (sleep), so none misses this. */
    wakeup(&vblank_seq);
    return true;
}

u32 fb_poll(struct file *f, struct poll_table *pt) {
    (void)f;
    if (!can_flip) {
        return POLL_READY_OUT; /* no flips: the buffer is always shown */
    }
    /* Without the interrupt nothing says when the flip is done: each tick
     * looks again. */
    poll_wait(pt, irq != 0 ? (const void *)&vblank_seq : (const void *)&ticks);
    acquire(&vblank_lock);
    u32 mask = POLL_READY_OUT;
    if ((rd(FB_STATUS) & STATUS_FLIP_PENDING) != 0) {
        mask = 0;
        /* The interrupt on, for the vertical blank that takes the flip: the
         * handler turns it off again unless a thread waits in
         * wait_vblank_locked(), and the poller, woken, turns it on again
         * if a flip is still pending. Enabled from a cleared VBLANK, as for
         * a waiter; and the flip may have been taken meanwhile. */
        if (irq != 0 && (rd(FB_IRQ) & IRQ_ENABLE) == 0) {
            wr(FB_IRQ, IRQ_ENABLE | IRQ_VBLANK);
            if ((rd(FB_STATUS) & STATUS_FLIP_PENDING) == 0) {
                mask = POLL_READY_OUT;
            }
        }
    }
    release(&vblank_lock);
    return mask;
}

/* Waits for the next vertical blank, holding vblank_lock (which sleep lets
 * go of meanwhile): 0, -EINTR if the process was killed, or -ENODEV without
 * the interrupt. It is enabled while anyone waits, from a cleared VBLANK so
 * that only a vertical blank from now on counts. */
static int wait_vblank_locked(void) {
    if (irq == 0) {
        return -ENODEV;
    }
    if (waiters == 0) {
        wr(FB_IRQ, IRQ_ENABLE | IRQ_VBLANK);
    }
    waiters++;
    u32 seq = vblank_seq;
    int r = 0;
    while (vblank_seq == seq) {
        if (proc_killed()) {
            r = -EINTR;
            break;
        }
        sleep(&vblank_seq, &vblank_lock);
    }
    waiters--;
    if (waiters == 0) {
        wr(FB_IRQ, IRQ_VBLANK);
    }
    return r;
}

/* Back to 640x480 from VRAM, the console's, with any buffer in RAM left
 * for fb_reclaim() to free: when the last open file and mapping go. Holding
 * fb_lock, and maybe proc_lock. */
static void release_display(void) {
    if (can_flip) {
        /* A modeset: at once, dropping a flip still pending. */
        mmio_write64(regs + FB_SCANOUT, 0);
        wr(FB_MODE, 0);
    }
    if (smem_ram != NULL) {
        /* At most one: a new buffer is made only after fb_reclaim(), by a
         * request on an open file, which keeps the display from being let
         * go of meanwhile. */
        if (stale_ram != NULL) {
            panic("fb: a second buffer to free");
        }
        stale_ram = smem_ram;
        stale_pages = smem_pages;
        smem_ram = NULL;
        smem_pages = 0;
    }
    cur_mode = 0;
    yres_virtual = mode_height[0];
    yoffset = 0;
    smem_pa = vram_pa;
    smem_len = stride(0) * mode_height[0];
    if (blanked) {
        wr(FB_CTRL, CTRL_ENABLE);
        blanked = false;
    }
    fbcon_resume();
}

int fb_open(struct file *f) {
    (void)f;
    if (!fb_present()) {
        return -ENODEV;
    }
    acquire(&fb_lock);
    if (opens == 0 && maps == 0) {
        fbcon_suspend(); /* the display is the program's */
    }
    opens++;
    release(&fb_lock);
    return 0;
}

void fb_close(struct file *f) {
    (void)f;
    acquire(&fb_lock);
    opens--;
    if (opens == 0 && maps == 0) {
        release_display();
    }
    release(&fb_lock);
    fb_reclaim();
}

void fb_reclaim(void) {
    /* Read without the lock first: the scheduler calls this after every
     * switch back, and there is seldom anything to free. */
    if (stale_ram == NULL) {
        return;
    }
    acquire(&fb_lock);
    void *pages = stale_ram;
    usize n = stale_pages;
    stale_ram = NULL;
    stale_pages = 0;
    release(&fb_lock);
    if (pages != NULL) {
        kfree_contig(pages, n);
    }
}

int fb_mmap(struct file *f, u64 offset, usize size, phys_addr_t *pa) {
    (void)f;
    acquire(&fb_lock);
    /* Whole pages: the buffer starts on one, and its last is mapped whole,
     * as Linux's fb_mmap rounds smem_len up. */
    u64 len = PGROUNDUP(smem_len);
    if (offset > len || size > len - offset) {
        release(&fb_lock);
        return -EINVAL;
    }
    *pa = smem_pa + offset;
    maps++;
    release(&fb_lock);
    return 0;
}

void fb_vm_open(void) {
    acquire(&fb_lock);
    maps++;
    release(&fb_lock);
}

void fb_vm_close(void) {
    acquire(&fb_lock);
    maps--;
    if (opens == 0 && maps == 0) {
        release_display();
    }
    release(&fb_lock);
}

/* The current mode and pan, as FBIOGET_VSCREENINFO gives them. Holding
 * fb_lock. */
static void get_var(struct fb_var_screeninfo *v) {
    memset(v, 0, sizeof *v);
    v->xres = mode_width[cur_mode];
    v->yres = mode_height[cur_mode];
    v->xres_virtual = v->xres;
    v->yres_virtual = yres_virtual;
    v->yoffset = yoffset;
    v->bits_per_pixel = 32;
    v->red.offset = 16;
    v->red.length = 8;
    v->green.offset = 8;
    v->green.length = 8;
    v->blue.length = 8;
    v->height = 0xffffffffu;
    v->width = 0xffffffffu;
}

static void get_fix(struct fb_fix_screeninfo *x) {
    memset(x, 0, sizeof *x);
    memcpy(x->id, "grail-fb", 9);
    x->smem_start = smem_pa;
    x->smem_len = smem_len;
    x->type = FB_TYPE_PACKED_PIXELS;
    x->visual = FB_VISUAL_TRUECOLOR;
    /* Pans only where there is a second frame: can_flip. */
    x->ypanstep = can_flip ? (u16)panstep(cur_mode) : 0;
    x->line_length = stride(cur_mode);
}

/* Checks `v` as FBIOPUT_VSCREENINFO takes it and fills in the rest, as
 * Linux's fb_check_var() rounds: the mode found by its size, a virtual size
 * below the mode's taken as the mode's, 0 bits a pixel as 32. Returns the
 * mode's number, or -EINVAL: no mode of that size, another pixel format, a
 * buffer wider than the mode or taller than two frames (or than one without
 * flips), a pan off the buffer or not on a page. */
static int check_var(struct fb_var_screeninfo *v) {
    u32 mode = nmodes;
    for (u32 i = 0; i < nmodes; i++) {
        if (mode_width[i] == v->xres && mode_height[i] == v->yres) {
            mode = i;
            break;
        }
    }
    if (mode == nmodes) {
        return -EINVAL;
    }
    if (v->bits_per_pixel != 0 && v->bits_per_pixel != 32) {
        return -EINVAL;
    }
    u32 xv = v->xres_virtual < v->xres ? v->xres : v->xres_virtual;
    u32 yv = v->yres_virtual < v->yres ? v->yres : v->yres_virtual;
    /* Room for a second frame from the first row on a page from yres on
     * (FB_SCANOUT takes only those): yres itself in every mode but
     * 800x600, whose frame is 468.75 pages, and whose second starts at row
     * 608. */
    u32 step = panstep(mode);
    u32 ymax = can_flip ? (v->yres + step - 1) / step * step + v->yres : v->yres;
    if (xv != v->xres || yv > ymax || v->xoffset != 0 || v->yoffset > yv - v->yres) {
        return -EINVAL;
    }
    if (v->yoffset % step != 0) {
        return -EINVAL;
    }
    u32 activate = v->activate;
    u32 yoff = v->yoffset;
    u32 cur = cur_mode;
    u32 cur_yv = yres_virtual;
    u32 cur_yoff = yoffset;
    /* The answer: as GET would give it once set. */
    cur_mode = mode;
    yres_virtual = yv;
    yoffset = yoff;
    get_var(v);
    cur_mode = cur;
    yres_virtual = cur_yv;
    yoffset = cur_yoff;
    v->activate = activate;
    return (int)mode;
}

/* Shows the buffer from row `row` now (a modeset of the current mode, which
 * switches buffers at once): 0, or -EINVAL if the display refused it. Holding
 * fb_lock. */
static int show(u32 mode, u32 row) {
    if (!can_flip) {
        return 0; /* only VRAM, from row 0 */
    }
    mmio_write64(regs + FB_SCANOUT, scanout(row));
    wr(FB_MODE, mode);
    if ((rd(FB_STATUS) & (STATUS_BAD_MODE | STATUS_BAD_SCANOUT)) != 0) {
        wr(FB_STATUS, STATUS_BAD_MODE | STATUS_BAD_SCANOUT);
        return -EINVAL;
    }
    return 0;
}

static long put_var(pagetable_t pt, uintptr arg) {
    struct fb_var_screeninfo v;
    if (copyin(pt, &v, arg, sizeof v) != 0) {
        return -EFAULT;
    }
    acquiresleep(&fb_mode_lock);
    /* A buffer the display let go of goes back before another is made. */
    fb_reclaim();
    acquire(&fb_lock);
    int mode = check_var(&v);
    release(&fb_lock);
    long r = mode < 0 ? mode : 0;
    if (r == 0 && (v.activate & FB_ACTIVATE_MASK) != FB_ACTIVATE_TEST) {
        u32 len = stride((u32)mode) * v.yres_virtual;
        /* VRAM holds one frame of mode 0; anything else is in RAM. */
        bool in_vram = mode == 0 && v.yres_virtual == mode_height[0];
        acquire(&fb_lock);
        bool same = (u32)mode == cur_mode && v.yres_virtual == yres_virtual;
        bool busy = !same && maps > 0;
        release(&fb_lock);
        void *buf = NULL;
        usize pages = PGROUNDUP(len) / PGSIZE;
        if (busy) {
            r = -EBUSY;
        } else if (!same && !in_vram) {
            buf = kalloc_contig(pages);
            if (buf == NULL) {
                r = -ENOMEM;
            } else {
                memset(buf, 0, pages * PGSIZE);
            }
        }
        if (r == 0) {
            void *old = NULL;
            usize old_pages = 0;
            acquire(&fb_lock);
            if (!same && maps > 0) {
                r = -EBUSY; /* mapped meanwhile */
            } else {
                /* The new state, then the display told; if it refuses,
                 * the old state back, and shown again. */
                void *was_ram = smem_ram;
                usize was_pages = smem_pages;
                phys_addr_t was_pa = smem_pa;
                u32 was_len = smem_len;
                u32 was_yv = yres_virtual;
                u32 was_mode = cur_mode;
                u32 was_yoff = yoffset;
                if (!same) {
                    smem_ram = buf;
                    smem_pages = buf != NULL ? pages : 0;
                    smem_pa = buf != NULL ? pa(buf) : vram_pa;
                    smem_len = len;
                    yres_virtual = v.yres_virtual;
                }
                cur_mode = (u32)mode;
                yoffset = v.yoffset;
                r = show(cur_mode, yoffset);
                if (r != 0) {
                    smem_ram = was_ram;
                    smem_pages = was_pages;
                    smem_pa = was_pa;
                    smem_len = was_len;
                    yres_virtual = was_yv;
                    cur_mode = was_mode;
                    yoffset = was_yoff;
                    show(cur_mode, yoffset);
                } else if (!same) {
                    old = was_ram;
                    old_pages = was_pages;
                    buf = NULL;
                }
            }
            release(&fb_lock);
            if (old != NULL) {
                kfree_contig(old, old_pages);
            }
        }
        if (buf != NULL) {
            kfree_contig(buf, pages);
        }
    }
    releasesleep(&fb_mode_lock);
    if (r == 0 && copyout(pt, arg, &v, sizeof v) != 0) {
        r = -EFAULT;
    }
    return r;
}

/* FBIOPAN_DISPLAY: shows the buffer from row yoffset at the next vertical
 * blank (a flip). With FB_ACTIVATE_VBL it returns once the flip shows. */
static long pan(pagetable_t pt, uintptr arg) {
    struct fb_var_screeninfo v;
    if (copyin(pt, &v, arg, sizeof v) != 0) {
        return -EFAULT;
    }
    bool vbl = (v.activate & FB_ACTIVATE_VBL) != 0;
    acquiresleep(&fb_mode_lock);
    acquire(&fb_lock);
    long r = 0;
    bool queued = false;
    if (v.xoffset != 0 || v.yoffset > yres_virtual - mode_height[cur_mode] ||
        v.yoffset % panstep(cur_mode) != 0) {
        r = -EINVAL;
    } else if (vbl && can_flip && irq == 0) {
        r = -ENODEV; /* no vertical blank to wait for: nothing queued */
    } else if (can_flip) {
        mmio_write64(regs + FB_SCANOUT, scanout(v.yoffset));
        wr(FB_FLIP, 1);
        if ((rd(FB_STATUS) & STATUS_BAD_SCANOUT) != 0) {
            wr(FB_STATUS, STATUS_BAD_SCANOUT);
            r = -EINVAL;
        } else {
            yoffset = v.yoffset;
            queued = true;
        }
    }
    release(&fb_lock);
    /* The wait for the flip, outside fb_lock (vblank_lock's sleep takes
     * proc_lock); fb_mode_lock keeps other pans and mode changes out. */
    if (queued && vbl) {
        acquire(&vblank_lock);
        while (r == 0 && (rd(FB_STATUS) & STATUS_FLIP_PENDING) != 0) {
            r = wait_vblank_locked();
        }
        release(&vblank_lock);
    }
    releasesleep(&fb_mode_lock);
    return r;
}

long fb_ioctl(struct file *f, u32 request, uintptr arg) {
    (void)f;
    pagetable_t pt = myproc()->pagetable;
    switch (request) {
    case FBIOGET_VSCREENINFO: {
        struct fb_var_screeninfo v;
        acquire(&fb_lock);
        get_var(&v);
        release(&fb_lock);
        return copyout(pt, arg, &v, sizeof v) != 0 ? -EFAULT : 0;
    }
    case FBIOGET_FSCREENINFO: {
        struct fb_fix_screeninfo x;
        acquire(&fb_lock);
        get_fix(&x);
        release(&fb_lock);
        return copyout(pt, arg, &x, sizeof x) != 0 ? -EFAULT : 0;
    }
    case FBIOPUT_VSCREENINFO:
        return put_var(pt, arg);
    case FBIOPAN_DISPLAY:
        return pan(pt, arg);
    case FBIOBLANK:
        /* The argument is the level itself, Linux's FB_BLANK_*. */
        if (arg > FB_BLANK_POWERDOWN) {
            return -EINVAL;
        }
        acquire(&fb_lock);
        blanked = arg != FB_BLANK_UNBLANK;
        wr(FB_CTRL, blanked ? 0 : CTRL_ENABLE);
        release(&fb_lock);
        return 0;
    case FBIO_WAITFORVSYNC: {
        u32 crtc;
        if (copyin(pt, &crtc, arg, sizeof crtc) != 0) {
            return -EFAULT;
        }
        if (crtc != 0) {
            return -ENODEV; /* one display */
        }
        acquire(&vblank_lock);
        int r = wait_vblank_locked();
        release(&vblank_lock);
        return r;
    }
    default:
        return -ENOTTY;
    }
}
