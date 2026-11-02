/* sleep SECONDS: waits that long without using the CPU (the sleep system
 * call), then exits 0. SECONDS is a whole number with at most one decimal,
 * such as 2 or 0.5: the clock ticks ten times a second. Exits 1 if it is
 * missing or not such a number. */
#include "user.h"

/* Timer ticks a second (the kernel's 10 Hz tick, docs/camelot-syscalls.md). */
#define TICKS_PER_SECOND 10

/* SECONDS in ticks, or -1 if it isn't a valid time. */
static int parse_ticks(const char *s) {
    uint ticks = 0;
    const char *p = s;
    while (*p >= '0' && *p <= '9') {
        ticks = ticks * 10 + (uint)(*p - '0');
        if (ticks > 100000000) {
            return -1;
        }
        p++;
    }
    int digits = p != s;
    ticks *= TICKS_PER_SECOND;
    if (*p == '.') {
        p++;
        if (*p >= '0' && *p <= '9') {
            ticks += (uint)(*p - '0');
            p++;
            digits = 1;
        }
    }
    return digits && *p == 0 ? (int)ticks : -1;
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(STDERR, "usage: sleep SECONDS\n");
        return 1;
    }
    int ticks = parse_ticks(argv[1]);
    if (ticks < 0) {
        fprintf(STDERR, "sleep: %s: invalid time\n", argv[1]);
        return 1;
    }
    sleep((uint)ticks);
    return 0;
}
