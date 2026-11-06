/* The pointer driver (input.h; spec section 9.7): /dev/input/event1, a
 * tablet in Linux's terms. The pointer stays in absolute mode, its
 * position the display's pixel: ABS_X and ABS_Y, from 0 to the mode's width
 * and height less 1. The buttons are BTN_LEFT, BTN_RIGHT and BTN_MIDDLE,
 * and the wheel's notches REL_WHEEL, positive up. */
#include "fb.h"
#include "input.h"
#include "mmio.h"
#include "plic.h"
#include "sysinfo.h"
#include "vm.h"

#define PTR_STATUS 0x00
#define PTR_EVENT 0x04
#define PTR_EVENT_XY 0x08
#define PTR_CTRL 0x0c
#define PTR_POS 0x10
#define PTR_BUTTONS 0x14

#define STATUS_READY (1u << 0)
#define STATUS_OVERFLOW (1u << 1)
#define CTRL_IRQ (1u << 0)
#define EVENT_BUTTONS 0x7u
#define EVENT_RELATIVE (1u << 3)
#define EVENT_VALID (1u << 31)

static uintptr regs;
static u32 irq;

/* What the readers were last told, under input_lock. */
static u32 buttons;
static u32 last_x;
static u32 last_y;

bool pointer_present(void) {
    return regs != 0;
}

void pointer_init(void) {
    u32 n = sysinfo_device_count();
    struct sysinfo_device dev;
    bool found = false;
    for (u32 i = 0; i < n && !found; i++) {
        sysinfo_device(i, &dev);
        found = dev.type == DEV_POINTER;
    }
    if (!found) {
        return;
    }
    regs = ioremap(dev.base, dev.size);
    /* Absolute mode, from where the pointer is. */
    mmio_write32(regs + PTR_CTRL, 0);
    u32 pos = mmio_read32(regs + PTR_POS);
    last_x = pos & 0xffff;
    last_y = pos >> 16;
    buttons = mmio_read32(regs + PTR_BUTTONS) & EVENT_BUTTONS;
    input_register(INPUT_POINTER, "Grail pointer", 2);
    input_set_capability(INPUT_POINTER, EV_KEY, BTN_LEFT);
    input_set_capability(INPUT_POINTER, EV_KEY, BTN_RIGHT);
    input_set_capability(INPUT_POINTER, EV_KEY, BTN_MIDDLE);
    input_set_capability(INPUT_POINTER, EV_REL, REL_WHEEL);
    input_set_capability(INPUT_POINTER, EV_ABS, ABS_X);
    input_set_capability(INPUT_POINTER, EV_ABS, ABS_Y);
    if (dev.irq != 0 && dev.irq < 32) {
        irq = dev.irq;
        plic_enable(irq);
        mmio_write32(regs + PTR_CTRL, CTRL_IRQ);
    }
}

void pointer_absinfo(u32 axis, struct input_absinfo *a) {
    u32 w;
    u32 h;
    fb_mode_size(&w, &h);
    a->value = (i32)(axis == ABS_X ? last_x : last_y);
    a->minimum = 0;
    a->maximum = (i32)(axis == ABS_X ? w : h) - 1;
    a->fuzz = 0;
    a->flat = 0;
    a->resolution = 0;
}

/* Reports a change to (x, y) with buttons `held` and `wheel` notches: only
 * what differs from what the readers have. */
static void report(u32 x, u32 y, u32 held, i32 wheel) {
    struct input_event ev[6];
    u32 n = 0;
    if (x != last_x) {
        ev[n].type = EV_ABS;
        ev[n].code = ABS_X;
        ev[n].value = (i32)x;
        n++;
        last_x = x;
    }
    if (y != last_y) {
        ev[n].type = EV_ABS;
        ev[n].code = ABS_Y;
        ev[n].value = (i32)y;
        n++;
        last_y = y;
    }
    for (u32 b = 0; b < 3; b++) {
        if (((held ^ buttons) >> b & 1) != 0) {
            ev[n].type = EV_KEY;
            ev[n].code = (u16)(BTN_LEFT + b);
            ev[n].value = (i32)(held >> b & 1);
            n++;
        }
    }
    buttons = held;
    if (wheel != 0) {
        ev[n].type = EV_REL;
        ev[n].code = REL_WHEEL;
        ev[n].value = wheel;
        n++;
    }
    input_report(INPUT_POINTER, ev, n);
}

bool pointer_intr(u32 source) {
    if (irq == 0 || source != irq) {
        return false;
    }
    input_lock_acquire();
    bool overflow = (mmio_read32(regs + PTR_STATUS) & STATUS_OVERFLOW) != 0;
    while ((mmio_read32(regs + PTR_STATUS) & STATUS_READY) != 0) {
        u32 e = mmio_read32(regs + PTR_EVENT);
        if ((e & EVENT_VALID) == 0) {
            break;
        }
        u32 xy = mmio_read32(regs + PTR_EVENT_XY);
        i32 wheel = (i8)(e >> 8 & 0xff);
        /* An event of relative mode (which the driver never sets) moves
         * nothing it reports. */
        u32 x = (e & EVENT_RELATIVE) != 0 ? last_x : (xy & 0xffff);
        u32 y = (e & EVENT_RELATIVE) != 0 ? last_y : (xy >> 16);
        report(x, y, e & EVENT_BUTTONS, wheel);
    }
    if (overflow) {
        /* Events were lost: catch up with where the pointer is now. */
        mmio_write32(regs + PTR_STATUS, STATUS_OVERFLOW);
        u32 pos = mmio_read32(regs + PTR_POS);
        report(pos & 0xffff, pos >> 16, mmio_read32(regs + PTR_BUTTONS) & EVENT_BUTTONS, 0);
    }
    input_lock_release();
    return true;
}
