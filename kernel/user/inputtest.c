/* inputtest: checks the input devices, /dev/input/event0 (the keyboard)
 * and event1 (the pointer) (docs/camelot-syscalls.md, "Input devices";
 * kernel/include/input.h): evdev's requests about each device, EVIOCGRAB's
 * rules and the errors, then prints the events a test types or moves. On a
 * root image, which has the nodes.
 *
 *   inputtest               the checks: "inputtest: N checks, F failed"
 *   inputtest keys N        prints "inputtest: ready", then the next N
 *                           events of the keyboard, a line each:
 *                           "type code value"
 *   inputtest pointer N     the same, of the pointer
 *   inputtest overflow      prints "inputtest: ready", waits for a line
 *                           (the UART's: the keyboard is the reader's) while
 *                           a test types more than a ring holds, then reads
 *                           one event and then two, a packet, and prints them
 *                           ("type code value"), which of KEY_A and KEY_B
 *                           EVIOCGKEY says are held, and what is left: "N
 *                           events left, M keys"
 *   inputtest grab          two readers of the keyboard, the first grabbing
 *                           it: prints "inputtest: grabbed" and the key the
 *                           first reads (a press and its SYN_REPORT), lets
 *                           go and prints "inputtest: let go" and the next
 *                           key the second reads, which saw nothing of the
 *                           first */
#include "user.h"

static int checks;
static int failed;

static void check(int ok, const char *what) {
    checks++;
    if (!ok) {
        failed++;
        printf("inputtest: FAILED: %s\n", what);
    }
}

static void check_eq(long got, long want, const char *what) {
    checks++;
    if (got != want) {
        failed++;
        printf("inputtest: FAILED: %s: got %ld, want %ld\n", what, got, want);
    }
}

static int has_bit(const unsigned char *bits, int n) {
    return bits[n / 8] >> (n % 8) & 1;
}

static void keyboard_checks(int kbd) {
    int version = 0;
    check_eq(ioctl(kbd, EVIOCGVERSION, &version), 0, "EVIOCGVERSION");
    check_eq(version, EV_VERSION, "evdev's version");
    struct input_id id;
    check_eq(ioctl(kbd, EVIOCGID, &id), 0, "EVIOCGID");
    check(id.bustype == BUS_HOST && id.product == 1, "the keyboard's id");
    char name[64];
    memset(name, 'x', sizeof name);
    check_eq(ioctl(kbd, EVIOCGNAME(sizeof name), name), 15, "EVIOCGNAME: the name and its NUL");
    check(strcmp(name, "Grail keyboard") == 0, "the keyboard's name");
    check_eq(ioctl(kbd, EVIOCGNAME(5), name), 5, "EVIOCGNAME cut short");
    check(memcmp(name, "Grail keyboard", 5) == 0, "its first 5 bytes");
    unsigned char bits[96];
    memset(bits, 0, sizeof bits);
    check_eq(ioctl(kbd, EVIOCGBIT(0, sizeof bits), bits), 8, "EVIOCGBIT(0): a long of types");
    check(has_bit(bits, EV_SYN) && has_bit(bits, EV_KEY) && !has_bit(bits, EV_REL) && !has_bit(bits, EV_ABS),
          "keys only");
    memset(bits, 0, sizeof bits);
    check_eq(ioctl(kbd, EVIOCGBIT(EV_KEY, sizeof bits), bits), 96, "EVIOCGBIT(EV_KEY): KEY_CNT bits");
    check(has_bit(bits, KEY_A) && has_bit(bits, KEY_ENTER) && has_bit(bits, KEY_LEFTMETA) &&
              has_bit(bits, KEY_F12) && !has_bit(bits, BTN_LEFT) && !has_bit(bits, 0),
          "the keyboard's keys");
    check_eq(ioctl(kbd, EVIOCGBIT(EV_REL, sizeof bits), bits), 8, "EVIOCGBIT(EV_REL)");
    check_eq(ioctl(kbd, EVIOCGBIT(EV_KEY, 4), bits), 4, "EVIOCGBIT cut short");
    check_eq(ioctl(kbd, EVIOCGBIT(7, sizeof bits), bits), -EINVAL, "EVIOCGBIT of a type evdev has not");
    memset(bits, 0xff, sizeof bits);
    check_eq(ioctl(kbd, EVIOCGKEY(sizeof bits), bits), 96, "EVIOCGKEY: KEY_CNT bits");
    int held = 0;
    for (unsigned i = 0; i < sizeof bits; i++) {
        held |= bits[i];
    }
    check(!held, "no key held");
    struct input_absinfo abs;
    check_eq(ioctl(kbd, EVIOCGABS(ABS_X), &abs), -EINVAL, "EVIOCGABS of a keyboard");
    check_eq(ioctl(kbd, 0x4680, 0), -ENOTTY, "an unknown request");
    check_eq(ioctl(kbd, EVIOCGVERSION, (void *)8), -EFAULT, "EVIOCGVERSION into page 0");

    /* Reads take whole events. */
    char small[23];
    check_eq(read(kbd, small, sizeof small), -EINVAL, "a read of less than an event");
    check_eq(write(kbd, small, sizeof small), -EBADF, "write to a reader open for reading");
    check_eq(lseek(kbd, 0, SEEK_SET), -ESPIPE, "lseek");

    /* EVIOCGRAB: one reader at a time; only it lets go. */
    int other = open("/dev/input/event0", O_RDONLY);
    check(other >= 0, "a second reader");
    check_eq(ioctl(kbd, EVIOCGRAB, (void *)1), 0, "grab");
    check_eq(ioctl(other, EVIOCGRAB, (void *)1), -EBUSY, "a second grab");
    check_eq(ioctl(kbd, EVIOCGRAB, (void *)1), -EBUSY, "the same reader again");
    check_eq(ioctl(other, EVIOCGRAB, 0), -EINVAL, "letting go of another's grab");
    check_eq(ioctl(kbd, EVIOCGRAB, 0), 0, "let go");
    check_eq(ioctl(kbd, EVIOCGRAB, 0), -EINVAL, "let go again");
    check_eq(ioctl(other, EVIOCGRAB, (void *)1), 0, "the second grabs");
    close(other);
    check_eq(ioctl(kbd, EVIOCGRAB, (void *)1), 0, "its close let go");
    check_eq(ioctl(kbd, EVIOCGRAB, 0), 0, "let go");
}

static void pointer_checks(int ptr) {
    struct input_id id;
    check_eq(ioctl(ptr, EVIOCGID, &id), 0, "the pointer's EVIOCGID");
    check_eq(id.product, 2, "its product");
    char name[64];
    check_eq(ioctl(ptr, EVIOCGNAME(sizeof name), name), 14, "its EVIOCGNAME");
    check(strcmp(name, "Grail pointer") == 0, "its name");
    unsigned char bits[96];
    memset(bits, 0, sizeof bits);
    check_eq(ioctl(ptr, EVIOCGBIT(0, sizeof bits), bits), 8, "its EVIOCGBIT(0)");
    check(has_bit(bits, EV_SYN) && has_bit(bits, EV_KEY) && has_bit(bits, EV_REL) && has_bit(bits, EV_ABS),
          "keys, a wheel and two axes");
    memset(bits, 0, sizeof bits);
    ioctl(ptr, EVIOCGBIT(EV_KEY, sizeof bits), bits);
    check(has_bit(bits, BTN_LEFT) && has_bit(bits, BTN_RIGHT) && has_bit(bits, BTN_MIDDLE) && !has_bit(bits, KEY_A),
          "three buttons");
    memset(bits, 0, sizeof bits);
    ioctl(ptr, EVIOCGBIT(EV_REL, sizeof bits), bits);
    check(has_bit(bits, REL_WHEEL) && !has_bit(bits, REL_X), "the wheel");
    memset(bits, 0, sizeof bits);
    ioctl(ptr, EVIOCGBIT(EV_ABS, sizeof bits), bits);
    check(has_bit(bits, ABS_X) && has_bit(bits, ABS_Y) && bits[0] == 3, "ABS_X and ABS_Y");
    struct input_absinfo abs;
    check_eq(ioctl(ptr, EVIOCGABS(ABS_X), &abs), 0, "EVIOCGABS(ABS_X)");
    check(abs.minimum == 0 && abs.maximum == 639 && abs.value == 0, "0 to 639, at 0");
    check_eq(ioctl(ptr, EVIOCGABS(ABS_Y), &abs), 0, "EVIOCGABS(ABS_Y)");
    check(abs.minimum == 0 && abs.maximum == 479, "0 to 479");
    check_eq(ioctl(ptr, EVIOCGABS(2), &abs), -EINVAL, "EVIOCGABS(ABS_Z)");
}

/* Prints the next `n` events of `fd`, a line each. */
static void print_events(int fd, int n) {
    struct input_event ev[8];
    while (n > 0) {
        int want = n < 8 ? n : 8;
        long got = read(fd, ev, want * sizeof ev[0]);
        if (got <= 0 || got % sizeof ev[0] != 0) {
            printf("inputtest: read: %ld\n", got);
            return;
        }
        for (long i = 0; i < got / (long)sizeof ev[0]; i++) {
            printf("%d %d %d\n", ev[i].type, ev[i].code, ev[i].value);
        }
        n -= got / sizeof ev[0];
    }
}

int main(int argc, char **argv) {
    if (argc == 3 && (strcmp(argv[1], "keys") == 0 || strcmp(argv[1], "pointer") == 0)) {
        int fd = open(argv[1][0] == 'k' ? "/dev/input/event0" : "/dev/input/event1", O_RDONLY);
        if (fd < 0) {
            printf("inputtest: open: %d\n", fd);
            return 1;
        }
        printf("inputtest: ready\n");
        print_events(fd, atoi(argv[2]));
        return 0;
    }
    if (argc == 2 && strcmp(argv[1], "overflow") == 0) {
        int fd = open("/dev/input/event0", O_RDONLY);
        if (fd < 0) {
            printf("inputtest: open: %d\n", fd);
            return 1;
        }
        printf("inputtest: ready\n");
        char line[8];
        read(0, line, sizeof line);
        static struct input_event ev[256];
        long n = read(fd, ev, sizeof ev[0]) / (long)sizeof ev[0];
        n += read(fd, ev + 1, 2 * sizeof ev[0]) / (long)sizeof ev[0];
        for (long i = 0; i < n; i++) {
            printf("%d %d %d\n", ev[i].type, ev[i].code, ev[i].value);
        }
        unsigned char keys[96];
        ioctl(fd, EVIOCGKEY(sizeof keys), keys);
        printf("held: KEY_A %d, KEY_B %d\n", has_bit(keys, KEY_A), has_bit(keys, KEY_B));
        n = read(fd, ev, sizeof ev) / (long)sizeof ev[0];
        int key_events = 0;
        for (long i = 0; i < n; i++) {
            key_events += ev[i].type == EV_KEY;
        }
        printf("%ld events left, %d keys\n", n, key_events);
        return 0;
    }
    if (argc == 2 && strcmp(argv[1], "grab") == 0) {
        int a = open("/dev/input/event0", O_RDONLY);
        int b = open("/dev/input/event0", O_RDONLY);
        if (a < 0 || b < 0 || ioctl(a, EVIOCGRAB, (void *)1) != 0) {
            printf("inputtest: no grab\n");
            return 1;
        }
        printf("inputtest: grabbed\n");
        print_events(a, 2);
        ioctl(a, EVIOCGRAB, 0);
        printf("inputtest: let go\n");
        print_events(b, 2);
        return 0;
    }
    int kbd = open("/dev/input/event0", O_RDONLY);
    int ptr = open("/dev/input/event1", O_RDONLY);
    check(kbd >= 0 && ptr >= 0, "open both");
    if (kbd < 0 || ptr < 0) {
        return 1;
    }
    keyboard_checks(kbd);
    pointer_checks(ptr);
    close(kbd);
    close(ptr);
    printf("inputtest: %d checks, %d failed\n", checks, failed);
    return failed;
}
