/* The keyboard driver (input.h; spec section 9.4): /dev/input/event0, and
 * the console's typing when no process reads the keyboard.
 *
 * In raw mode the keyboard sends the USB HID usage of each key pressed and
 * released, which hid_keyboard[] turns into Linux's key code, as Linux's
 * hid-input.c does with the same table. In ASCII mode it sends the
 * character the host's layout typed, with Shift and Ctrl as bits: that is
 * the console's input (Ctrl with a letter its control character, as a
 * terminal sends it), or, on a keyboard without raw mode, a key code made
 * from the character on a US layout, with Shift and Ctrl presses made up
 * from the bits. */
#include "console.h"
#include "input.h"
#include "mmio.h"
#include "plic.h"
#include "sysinfo.h"
#include "vm.h"

#define KBD_STATUS 0x00
#define KBD_DATA 0x04
#define KBD_CTRL 0x08

#define STATUS_READY (1u << 0)
#define CTRL_IRQ (1u << 0)
#define CTRL_RAW (1u << 1)

/* Raw mode's event: the usage, and whether it is a release. */
#define RAW_USAGE 0xffffu
#define RAW_RELEASE (1u << 16)
/* ASCII mode's: the code, a release, Shift and Ctrl held. */
#define ASCII_CODE 0xffu
#define ASCII_RELEASE (1u << 8)
#define ASCII_SHIFT (1u << 9)
#define ASCII_CTRL (1u << 10)

/* ASCII mode's codes past ASCII (spec section 9.4). */
#define ASCII_UP 0x80
#define ASCII_DOWN 0x81
#define ASCII_LEFT 0x82
#define ASCII_RIGHT 0x83
#define ASCII_F1 0x90
#define ASCII_F12 0x9b

/* Linux's key code for each usage of the Keyboard/Keypad page, 0 for none:
 * hid_keyboard[] of drivers/hid/hid-input.c. */
static const u8 hid_keyboard[256] = {
    0,   0,   0,   0,   30,  48,  46,  32,  18,  33,  34,  35,  23,  36,  37,  38,  /* 0x00 */
    50,  49,  24,  25,  16,  19,  31,  20,  22,  47,  17,  45,  21,  44,  2,   3,   /* 0x10 */
    4,   5,   6,   7,   8,   9,   10,  11,  28,  1,   14,  15,  57,  12,  13,  26,  /* 0x20 */
    27,  43,  43,  39,  40,  41,  51,  52,  53,  58,  59,  60,  61,  62,  63,  64,  /* 0x30 */
    65,  66,  67,  68,  87,  88,  99,  70,  119, 110, 102, 104, 111, 107, 109, 106, /* 0x40 */
    105, 108, 103, 69,  98,  55,  74,  78,  96,  79,  80,  81,  75,  76,  77,  71,  /* 0x50 */
    72,  73,  82,  83,  86,  127, 116, 117, 183, 184, 185, 186, 187, 188, 189, 190, /* 0x60 */
    191, 192, 193, 194, 134, 138, 130, 132, 128, 129, 131, 137, 133, 135, 136, 113, /* 0x70 */
    115, 114, 0,   0,   0,   121, 0,   89,  93,  124, 92,  94,  95,  0,   0,   0,   /* 0x80 */
    122, 123, 90,  91,  85,  0,   0,   0,   0,   0,   0,   0,   111, 0,   0,   0,   /* 0x90 */
    0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   /* 0xa0 */
    0,   0,   0,   0,   0,   0,   179, 180, 0,   0,   0,   0,   0,   0,   0,   0,   /* 0xb0 */
    0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   /* 0xc0 */
    0,   0,   0,   0,   0,   0,   0,   0,   111, 0,   0,   0,   0,   0,   0,   0,   /* 0xd0 */
    29,  42,  56,  125, 97,  54,  100, 126, 164, 166, 165, 163, 161, 115, 114, 113, /* 0xe0 */
    150, 158, 159, 128, 136, 177, 178, 176, 142, 152, 173, 140, 0,   0,   0,   0,   /* 0xf0 */
};

/* The usage of the key that types each ASCII character on a US layout,
 * from ' ' (0x20) to '~' (0x7E), Shift or not: ASCII mode's codes made
 * keys. */
static const u8 us_usage[95] = {
    0x2c, 0x1e, 0x34, 0x20, 0x21, 0x22, 0x24, 0x34, 0x26, 0x27, 0x25, 0x2e, 0x36, 0x2d, 0x37, 0x38, /*  !"#$%&'()*+,-./ */
    0x27, 0x1e, 0x1f, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x33, 0x33, 0x36, 0x2e, 0x37, 0x38, /* 0123456789:;<=>? */
    0x1f, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12, /* @ABCDEFGHIJKLMNO */
    0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x2f, 0x31, 0x30, 0x23, 0x2d, /* PQRSTUVWXYZ[\]^_ */
    0x35, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12, /* `abcdefghijklmno */
    0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x2f, 0x31, 0x30, 0x35,       /* pqrstuvwxyz{|}~ */
};

static uintptr regs;
static u32 irq;
/* Whether the keyboard has raw mode (KBD_CTRL bit 1 holds), and whether it
 * is in it: while the keyboard has readers. Not static, so that a test can
 * take raw mode away (crates/xtask/tests/processes.rs). */
bool keyboard_has_raw;
static bool raw;
/* ASCII mode with readers: the Shift and Ctrl made up for them. */
static bool shift_down;
static bool ctrl_down;

bool keyboard_present(void) {
    return regs != 0;
}

void keyboard_init(void) {
    u32 n = sysinfo_device_count();
    struct sysinfo_device dev;
    bool found = false;
    for (u32 i = 0; i < n && !found; i++) {
        sysinfo_device(i, &dev);
        found = dev.type == DEV_KEYBOARD;
    }
    if (!found) {
        return;
    }
    regs = ioremap(dev.base, dev.size);
    /* Raw mode is there if its bit holds; then back to ASCII mode, the
     * console's, which empties the FIFO of anything typed before. */
    mmio_write32(regs + KBD_CTRL, CTRL_RAW);
    keyboard_has_raw = (mmio_read32(regs + KBD_CTRL) & CTRL_RAW) != 0;
    mmio_write32(regs + KBD_CTRL, 0);
    input_register(INPUT_KEYBOARD, "Grail keyboard", 1);
    for (u32 u = 0; u < 256; u++) {
        if (hid_keyboard[u] != 0) {
            input_set_capability(INPUT_KEYBOARD, EV_KEY, hid_keyboard[u]);
        }
    }
    if (dev.irq != 0 && dev.irq < 32) {
        irq = dev.irq;
        plic_enable(irq);
        mmio_write32(regs + KBD_CTRL, CTRL_IRQ);
    }
}

void keyboard_readers_changed(bool readers) {
    shift_down = false;
    ctrl_down = false;
    if (!keyboard_has_raw) {
        return;
    }
    /* A change of mode empties the FIFO: no event of the other mode is
     * read. */
    raw = readers;
    mmio_write32(regs + KBD_CTRL, (irq != 0 ? CTRL_IRQ : 0) | (raw ? CTRL_RAW : 0));
}

/* The key code for ASCII mode's `code`, or 0 for none. */
static u32 ascii_key(u32 code) {
    if (code >= ' ' && code <= '~') {
        return hid_keyboard[us_usage[code - ' ']];
    }
    switch (code) {
    case 0x08:
        return KEY_BACKSPACE;
    case 0x09:
        return KEY_TAB;
    case 0x0a:
        return KEY_ENTER;
    case 0x1b:
        return KEY_ESC;
    case ASCII_UP:
        return KEY_UP;
    case ASCII_DOWN:
        return KEY_DOWN;
    case ASCII_LEFT:
        return KEY_LEFT;
    case ASCII_RIGHT:
        return KEY_RIGHT;
    default:
        break;
    }
    if (code >= ASCII_F1 && code <= ASCII_F12) {
        u32 f = code - ASCII_F1;
        return f < 10 ? KEY_F1 + f : KEY_F11 + (f - 10);
    }
    return 0;
}

/* ASCII mode's event, for readers: Shift and Ctrl as the bits say, then the
 * key. */
static void report_ascii(u32 v) {
    u32 key = ascii_key(v & ASCII_CODE);
    if (key == 0) {
        return;
    }
    struct input_event ev[3];
    u32 n = 0;
    bool press = (v & ASCII_RELEASE) == 0;
    if (press) {
        bool shift = (v & ASCII_SHIFT) != 0;
        bool ctrl = (v & ASCII_CTRL) != 0;
        if (shift != shift_down) {
            ev[n].type = EV_KEY;
            ev[n].code = KEY_LEFTSHIFT;
            ev[n].value = shift;
            n++;
            shift_down = shift;
        }
        if (ctrl != ctrl_down) {
            ev[n].type = EV_KEY;
            ev[n].code = KEY_LEFTCTRL;
            ev[n].value = ctrl;
            n++;
            ctrl_down = ctrl;
        }
    }
    ev[n].type = EV_KEY;
    ev[n].code = (u16)key;
    ev[n].value = press;
    n++;
    input_report(INPUT_KEYBOARD, ev, n);
}

/* ASCII mode's event, for the console: a press of a character, with Ctrl
 * its control character. */
static void type_ascii(u32 v) {
    u32 c = v & ASCII_CODE;
    if ((v & ASCII_RELEASE) != 0 || c >= 0x80) {
        return;
    }
    if ((v & ASCII_CTRL) != 0 && ((c >= 'a' && c <= 'z') || (c >= '@' && c <= '_'))) {
        c = c & 0x1f;
    }
    console_input((char)c);
}

bool keyboard_intr(u32 source) {
    if (irq == 0 || source != irq) {
        return false;
    }
    input_lock_acquire();
    while ((mmio_read32(regs + KBD_STATUS) & STATUS_READY) != 0) {
        u32 v = mmio_read32(regs + KBD_DATA);
        if (v == 0) {
            break;
        }
        if (raw) {
            u32 usage = v & RAW_USAGE;
            u32 key = usage < 256 ? hid_keyboard[usage] : 0;
            if (key != 0) {
                struct input_event ev;
                ev.type = EV_KEY;
                ev.code = (u16)key;
                ev.value = (v & RAW_RELEASE) == 0;
                input_report(INPUT_KEYBOARD, &ev, 1);
            }
        } else if (input_has_readers(INPUT_KEYBOARD)) {
            report_ascii(v);
        } else {
            type_ascii(v);
        }
    }
    input_lock_release();
    return true;
}
