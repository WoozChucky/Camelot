/* The input core and its evdev interface (input.h), after Linux's
 * drivers/input/input.c and evdev.c: the devices the drivers register, the
 * readers of each, and their rings. */
#include "input.h"
#include "file.h"
#include "kalloc.h"
#include "kprintf.h"
#include "poll.h"
#include "proc.h"
#include "riscv.h"
#include "spinlock.h"
#include "string.h"
#include "timer.h"

/* Events a reader's ring holds: its page, less the reader's own fields. */
#define RING 128

/* An open file of a device: its reader (Linux's evdev_client). Events
 * [tail, head) wait in the ring, the counters running on and wrapping. */
struct reader {
    struct reader *next; /* the device's next reader */
    u32 minor;
    u32 head;
    u32 tail;
    struct input_event ring[RING];
};

_Static_assert(sizeof(struct reader) <= PGSIZE, "a reader fits a page");

#define BITS 64
#define WORDS(n) (((n) + BITS - 1) / BITS)

struct device {
    bool present;
    const char *name;
    u16 product;
    u64 evbit[WORDS(EV_CNT)];
    u64 keybit[WORDS(KEY_CNT)];
    u64 relbit[WORDS(REL_CNT)];
    u64 absbit[WORDS(ABS_CNT)];
    u64 down[WORDS(KEY_CNT)]; /* the keys and buttons held, as reported */
    struct reader *readers;
    struct reader *grab; /* EVIOCGRAB's: the one reader, or NULL */
    u32 nreaders;
};

static struct spinlock input_lock;
static struct device devices[INPUT_DEVICES];

static bool test_bit(const u64 *bits, u32 n) {
    return (bits[n / BITS] >> (n % BITS) & 1) != 0;
}

static void set_bit(u64 *bits, u32 n, bool on) {
    if (on) {
        bits[n / BITS] |= 1ull << (n % BITS);
    } else {
        bits[n / BITS] &= ~(1ull << (n % BITS));
    }
}

void input_lock_acquire(void) {
    acquire(&input_lock);
}

void input_lock_release(void) {
    release(&input_lock);
}

void input_init(void) {
    initlock(&input_lock, "input");
    keyboard_init();
    pointer_init();
}

void input_register(u32 minor, const char *name, u16 product) {
    struct device *d = &devices[minor];
    d->present = true;
    d->name = name;
    d->product = product;
    set_bit(d->evbit, EV_SYN, true);
}

void input_set_capability(u32 minor, u32 type, u32 code) {
    struct device *d = &devices[minor];
    set_bit(d->evbit, type, true);
    if (type == EV_KEY) {
        set_bit(d->keybit, code, true);
    } else if (type == EV_REL) {
        set_bit(d->relbit, code, true);
    } else if (type == EV_ABS) {
        set_bit(d->absbit, code, true);
    }
}

bool input_has_readers(u32 minor) {
    return devices[minor].nreaders > 0;
}

/* The time now since boot, as a timeval: the timestamp of a packet. */
static struct timeval now(void) {
    u64 t = r_time();
    u64 hz = timebase_hz();
    struct timeval tv;
    tv.tv_sec = (time_t)(t / hz);
    tv.tv_usec = (long)((t % hz) * 1000000 / hz);
    return tv;
}

/* Puts the `n` events of `ev` in `r`'s ring. A ring without room for them
 * loses everything waiting, which a SYN_DROPPED takes the place of (Linux's
 * evdev: the reader is to throw away what it has of the packet and ask the
 * device for its state). */
static void push(struct reader *r, const struct input_event *ev, u32 n) {
    if (RING - (r->head - r->tail) < n) {
        r->tail = r->head;
        struct input_event *d = &r->ring[r->head % RING];
        d->time = ev[0].time;
        d->type = EV_SYN;
        d->code = SYN_DROPPED;
        d->value = 0;
        r->head++;
    }
    for (u32 i = 0; i < n; i++) {
        r->ring[r->head % RING] = ev[i];
        r->head++;
    }
    wakeup(r);
}

/* Drops the events of `type` waiting in `r`'s ring, and the SYN_REPORTs that
 * then end empty packets, but for one that leads (it ends a packet the
 * reader has read part of): Linux's __evdev_flush_queue. EVIOCGKEY does it,
 * so that a reader that was given the keys' state isn't told again of the
 * changes that made it. Holding input_lock. */
static void flush_type(struct reader *r, u16 type) {
    u32 to = r->tail;
    u32 in_packet = 1; /* events kept since the last SYN_REPORT kept */
    for (u32 i = r->tail; i != r->head; i++) {
        struct input_event ev = r->ring[i % RING];
        bool report = ev.type == EV_SYN && ev.code == SYN_REPORT;
        if (ev.type == type || (report && in_packet == 0)) {
            continue;
        }
        r->ring[to % RING] = ev;
        to++;
        in_packet = report ? 0 : in_packet + 1;
    }
    r->head = to;
}

/* The most events a driver reports at once, its SYN_REPORT added. */
#define PACKET_MAX 8

void input_report(u32 minor, struct input_event *ev, u32 n) {
    struct device *d = &devices[minor];
    struct input_event packet[PACKET_MAX];
    u32 k = 0;
    for (u32 i = 0; i < n && k < PACKET_MAX - 1; i++) {
        if (ev[i].type == EV_KEY) {
            bool held = test_bit(d->down, ev[i].code);
            bool press = ev[i].value != 0;
            /* A press of a key held already, or a release of one that
             * isn't: nothing changed (a key held across the keyboard's
             * switch to raw mode, say, gives its release alone). */
            if (held == press) {
                continue;
            }
            set_bit(d->down, ev[i].code, press);
        }
        packet[k] = ev[i];
        k++;
    }
    if (k == 0) {
        return;
    }
    packet[k].type = EV_SYN;
    packet[k].code = SYN_REPORT;
    packet[k].value = 0;
    k++;
    struct timeval t = now();
    for (u32 i = 0; i < k; i++) {
        packet[i].time = t;
    }
    if (d->grab != NULL) {
        push(d->grab, packet, k);
        return;
    }
    for (struct reader *r = d->readers; r != NULL; r = r->next) {
        push(r, packet, k);
    }
}

int input_open(struct file *f) {
    if (f->minor >= INPUT_DEVICES || !devices[f->minor].present) {
        return -ENODEV;
    }
    struct reader *r = kalloc();
    if (r == NULL) {
        return -ENOMEM;
    }
    r->minor = f->minor;
    r->head = 0;
    r->tail = 0;
    struct device *d = &devices[f->minor];
    acquire(&input_lock);
    r->next = d->readers;
    d->readers = r;
    d->nreaders++;
    if (d->nreaders == 1) {
        /* What was held is the console's, or was before raw mode. */
        memset(d->down, 0, sizeof d->down);
        if (f->minor == INPUT_KEYBOARD) {
            keyboard_readers_changed(true);
        }
    }
    release(&input_lock);
    f->priv = r;
    return 0;
}

void input_close(struct file *f) {
    struct reader *r = f->priv;
    struct device *d = &devices[r->minor];
    acquire(&input_lock);
    struct reader **link = &d->readers;
    while (*link != r) {
        link = &(*link)->next;
    }
    *link = r->next;
    if (d->grab == r) {
        d->grab = NULL;
    }
    d->nreaders--;
    if (d->nreaders == 0) {
        memset(d->down, 0, sizeof d->down);
        if (r->minor == INPUT_KEYBOARD) {
            keyboard_readers_changed(false);
        }
    }
    release(&input_lock);
    kfree(r);
}

/* Events a read moves at a time, through the kernel stack. */
#define READ_BATCH 16

isize input_read(struct file *f, pagetable_t pt, uintptr dst, usize n) {
    struct reader *r = f->priv;
    if (n < sizeof(struct input_event)) {
        return -EINVAL;
    }
    if (!uvm_range_ok(pt, dst, n, PTE_W)) {
        return -EFAULT;
    }
    usize want = n / sizeof(struct input_event);
    usize done = 0;
    struct input_event batch[READ_BATCH];
    acquire(&input_lock);
    while (r->head == r->tail) {
        if (proc_killed()) {
            release(&input_lock);
            return -EINTR;
        }
        sleep(r, &input_lock);
    }
    while (done < want && r->head != r->tail) {
        u32 k = 0;
        while (k < READ_BATCH && done + k < want && r->head != r->tail) {
            batch[k] = r->ring[r->tail % RING];
            r->tail++;
            k++;
        }
        /* copyout() never sleeps: it may run under the lock. Checked
         * above, but another thread may have unmapped the buffer since. */
        if (copyout(pt, dst + done * sizeof(struct input_event), batch, k * sizeof(struct input_event)) != 0) {
            release(&input_lock);
            return done > 0 ? (isize)(done * sizeof(struct input_event)) : -EFAULT;
        }
        done += k;
    }
    release(&input_lock);
    return (isize)(done * sizeof(struct input_event));
}

/* Copies the first `len` bytes of `size` at `src` (fewer if `len` is
 * smaller) to user address `arg`: how many, or -EFAULT. Linux's
 * bits_to_user() and str_to_user(). */
static long to_user(pagetable_t pt, uintptr arg, const void *src, usize size, usize len) {
    usize count = len < size ? len : size;
    if (copyout(pt, arg, src, count) != 0) {
        return -EFAULT;
    }
    return (long)count;
}

/* EVIOCGBIT(ev, len): the bitmap of event types (ev 0) or of the codes of
 * type `ev`, in whole longs as Linux gives them; a type the device has no
 * codes of, an empty one. */
static long get_bits(struct device *d, pagetable_t pt, uintptr arg, u32 ev, usize len) {
    static const u64 none[WORDS(KEY_CNT)];
    switch (ev) {
    case 0:
        return to_user(pt, arg, d->evbit, sizeof d->evbit, len);
    case EV_KEY:
        return to_user(pt, arg, d->keybit, sizeof d->keybit, len);
    case EV_REL:
        return to_user(pt, arg, d->relbit, sizeof d->relbit, len);
    case EV_ABS:
        return to_user(pt, arg, d->absbit, sizeof d->absbit, len);
    case EV_MSC:
    case EV_SW:
    case EV_LED:
    case EV_SND:
        return to_user(pt, arg, none, 8, len);
    case EV_FF:
        return to_user(pt, arg, none, 16, len);
    default:
        return -EINVAL;
    }
}

u32 input_poll(struct file *f, struct poll_table *pt) {
    struct reader *r = f->priv;
    /* push() wakes the reader's ring. */
    poll_wait(pt, r);
    acquire(&input_lock);
    u32 mask = r->head != r->tail ? POLL_READY_IN : 0;
    release(&input_lock);
    return mask;
}

long input_ioctl(struct file *f, u32 request, uintptr arg) {
    struct reader *r = f->priv;
    struct device *d = &devices[r->minor];
    pagetable_t pt = myproc()->pagetable;
    if (request == EVIOCGVERSION) {
        int v = EV_VERSION;
        return copyout(pt, arg, &v, sizeof v) != 0 ? -EFAULT : 0;
    }
    if (request == EVIOCGID) {
        struct input_id id = {.bustype = BUS_HOST, .vendor = 0, .product = d->product, .version = 1};
        return copyout(pt, arg, &id, sizeof id) != 0 ? -EFAULT : 0;
    }
    if (request == EVIOCGRAB) {
        /* The argument is the value: grab with any but 0. */
        long ret = 0;
        acquire(&input_lock);
        if (arg != 0) {
            if (d->grab != NULL) {
                ret = -EBUSY;
            } else {
                d->grab = r;
            }
        } else if (d->grab != r) {
            ret = -EINVAL;
        } else {
            d->grab = NULL;
        }
        release(&input_lock);
        return ret;
    }
    /* The rest read, _IOC_READ, their size in the request. */
    u32 dir = request >> 30;
    usize size = request >> 16 & 0x3fff;
    u32 type = request >> 8 & 0xff;
    u32 nr = request & 0xff;
    if (dir != 2 || type != 'E') {
        return -ENOTTY;
    }
    if (nr == EVIOC_NAME_NR) {
        return to_user(pt, arg, d->name, strlen(d->name) + 1, size);
    }
    if (nr == EVIOC_KEY_NR) {
        /* The keys held, as the readers were told: what a reader asks for
         * after a SYN_DROPPED, to learn what it missed. The key events
         * waiting for this reader go, since the state has them (Linux's
         * evdev_handle_get_val), even if the copy then fails. */
        u64 down[WORDS(KEY_CNT)];
        acquire(&input_lock);
        memcpy(down, d->down, sizeof down);
        flush_type(r, EV_KEY);
        release(&input_lock);
        return to_user(pt, arg, down, KEY_CNT / 8, size);
    }
    if (nr >= EVIOC_BIT_NR && nr < EVIOC_BIT_NR + EV_CNT) {
        return get_bits(d, pt, arg, nr - EVIOC_BIT_NR, size);
    }
    if (nr >= EVIOC_ABS_NR && nr < EVIOC_ABS_NR + ABS_CNT) {
        u32 axis = nr - EVIOC_ABS_NR;
        if (!test_bit(d->absbit, axis)) {
            return -EINVAL;
        }
        struct input_absinfo a;
        acquire(&input_lock);
        pointer_absinfo(axis, &a);
        release(&input_lock);
        long got = to_user(pt, arg, &a, sizeof a, size);
        return got < 0 ? got : 0;
    }
    return -ENOTTY;
}
