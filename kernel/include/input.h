/* The input devices, /dev/input/event0 (the keyboard) and event1 (the
 * pointer), major INPUT (file.h; spec section 10, "Input drivers"): Linux's
 * evdev interface. A read gives whole `struct input_event`s, 24 bytes each,
 * a packet of them ending with EV_SYN/SYN_REPORT for each change the device
 * reported; the key codes are Linux's (input-event-codes.h), the keyboard's
 * from the USB HID usages Grail's raw mode sends, as Linux's hid-input.c
 * maps them. ioctl answers evdev's requests about the device, and
 * EVIOCGRAB.
 *
 * Each open file is a reader with a ring of its own (Linux's evdev_client):
 * every event goes to every reader of the device, or only to the one that
 * grabbed it. A reader that falls a whole ring behind loses what it had not
 * read and finds SYN_DROPPED in its place, as Linux's evdev does.
 *
 * The keyboard is the console's when no process has /dev/input/event0 open
 * (console.h): its keys are then typed characters, from the keyboard's ASCII
 * mode, the host's layout. While a reader has it open it is in raw mode, the
 * physical keys; a keyboard without raw mode (FB-era Grail) has its ASCII
 * codes turned into key codes, with Shift and Ctrl presses made up around
 * them (US layout).
 *
 * The drivers (keyboard.c, pointer.c) call input_report() with each change
 * from their interrupt handlers; input.c is the core between them and the
 * readers. Locking: input_lock, a spinlock, protects the devices, their
 * readers and the rings; readers sleep on their ring with it. It comes
 * before the console's lock (the keyboard feeds the console under it) and
 * after nothing but a process's own locks. */
#ifndef CAMELOT_INPUT_H
#define CAMELOT_INPUT_H

#include "syscall.h"
#include "types.h"
#include "vm.h"

struct file;

/* The devices, by minor number. */
#define INPUT_KEYBOARD 0
#define INPUT_POINTER 1
#define INPUT_DEVICES 2

/* Linux's struct input_event: 24 bytes on LP64. */
struct input_event {
    struct timeval time; /* when, since boot */
    u16 type;
    u16 code;
    i32 value;
};

_Static_assert(sizeof(struct input_event) == 24, "Linux's input_event on LP64");

/* EVIOCGID's: the device's bus and numbers. */
struct input_id {
    u16 bustype;
    u16 vendor;
    u16 product;
    u16 version;
};

/* EVIOCGABS's: an axis's value and range. */
struct input_absinfo {
    i32 value;
    i32 minimum;
    i32 maximum;
    i32 fuzz;
    i32 flat;
    i32 resolution;
};

/* Event types and codes (Linux's input-event-codes.h). */
#define EV_SYN 0x00
#define EV_KEY 0x01
#define EV_REL 0x02
#define EV_ABS 0x03
#define EV_MSC 0x04
#define EV_SW 0x05
#define EV_LED 0x11
#define EV_SND 0x12
#define EV_REP 0x14
#define EV_FF 0x15
#define EV_CNT 0x20
#define SYN_REPORT 0
#define SYN_DROPPED 3
#define REL_X 0x00
#define REL_Y 0x01
#define REL_WHEEL 0x08
#define REL_CNT 0x10
#define ABS_X 0x00
#define ABS_Y 0x01
#define ABS_CNT 0x40
#define BTN_LEFT 0x110
#define BTN_RIGHT 0x111
#define BTN_MIDDLE 0x112
#define KEY_CNT 0x300
#define BUS_HOST 0x19

/* The keys the console and the ASCII mode's codes use. */
#define KEY_ESC 1
#define KEY_1 2
#define KEY_0 11
#define KEY_BACKSPACE 14
#define KEY_TAB 15
#define KEY_ENTER 28
#define KEY_LEFTCTRL 29
#define KEY_LEFTSHIFT 42
#define KEY_SPACE 57
#define KEY_F1 59
#define KEY_F11 87
#define KEY_F12 88
#define KEY_UP 103
#define KEY_LEFT 105
#define KEY_RIGHT 106
#define KEY_DOWN 108

/* evdev's requests (Linux's input.h): _IOR('E', nr, size) and the like. A
 * request's direction is bits 31:30, its size 29:16, 'E' 15:8, nr 7:0. */
#define EVIOCGVERSION 0x80044501   /* int: EV_VERSION */
#define EVIOCGID 0x80084502        /* struct input_id */
#define EVIOCGRAB 0x40044590       /* int, by value: grab (1) or let go (0) */
#define EVIOC_NAME_NR 0x06         /* EVIOCGNAME(len): the name, len bytes at most */
#define EVIOC_BIT_NR 0x20          /* EVIOCGBIT(ev, len): nr 0x20 + ev, a bitmap */
#define EVIOC_ABS_NR 0x40          /* EVIOCGABS(abs): nr 0x40 + abs, struct input_absinfo */
#define EV_VERSION 0x010001

/* Sets up the lock, then the drivers (keyboard_init, pointer_init), which
 * register the devices they find. Before the first process. */
void input_init(void);

/* A driver's device `minor`, there, with its name (EVIOCGNAME) and product
 * number (EVIOCGID), and each event it reports (EVIOCGBIT): EV_SYN is
 * every device's. At boot. */
void input_register(u32 minor, const char *name, u16 product);
void input_set_capability(u32 minor, u32 type, u32 code);

/* A driver's change to its device `minor`: `n` events, which input.c ends
 * with a SYN_REPORT, all at the time now, to the readers (or the one that
 * grabbed it). A key's press while it is down already, or its release while
 * it is up, is dropped, as Linux's input core drops them; so is a packet
 * left with nothing. Holding input_lock (the drivers' handlers take it). */
void input_report(u32 minor, struct input_event *ev, u32 n);

/* Whether device `minor` has a reader. Holding input_lock. */
bool input_has_readers(u32 minor);

/* input_lock, for the drivers' handlers: they read their device's events
 * and report them under it. */
void input_lock_acquire(void);
void input_lock_release(void);

/* devsw's functions for INPUT (file.h): open makes the file a reader of
 * device `minor` (-ENODEV for a device the machine lacks, -ENOMEM with no
 * page for its ring), read takes whole events, waiting for one (-EINVAL for
 * fewer than 24 bytes), ioctl answers EVIOCGVERSION, EVIOCGID, EVIOCGNAME,
 * EVIOCGBIT, EVIOCGABS and EVIOCGRAB. */
int input_open(struct file *f);
void input_close(struct file *f);
isize input_read(struct file *f, pagetable_t pt, uintptr dst, usize n);
long input_ioctl(struct file *f, u32 request, uintptr arg);

/* The drivers (keyboard.c, pointer.c). Each finds its device in the device
 * table, maps it and enables its interrupt; nothing without one. */
void keyboard_init(void);
void pointer_init(void);
bool keyboard_present(void);
bool pointer_present(void);

/* Their interrupts, PLIC source `irq`: whether it was theirs
 * (external_interrupt, trap.c). */
bool keyboard_intr(u32 irq);
bool pointer_intr(u32 irq);

/* The keyboard's mode as its readers come and go: raw with a reader, ASCII
 * (the console's) without. Holding input_lock. */
void keyboard_readers_changed(bool readers);

/* The pointer's absolute axes' current values and ranges: the display's
 * mode (EVIOCGABS). */
void pointer_absinfo(u32 axis, struct input_absinfo *a);

#endif
