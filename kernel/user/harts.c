/* harts: lists the harts the kernel runs on, from the harts system call: a
 * header, then a line for each one, its id, the process running on it (`-`
 * while it idles; on its own hart, harts itself), and the timer ticks it has
 * taken and the processes it has switched to since it started:
 *
 *     HART   PID    TICKS SWITCHES
 *        0     -      193       41
 *        1     7      188       36
 *
 * A hart whose SWITCHES grow has been running processes (spec section 10,
 * "Kernel SMP"). Exits 1 if given an argument, else 0. */
#include "user.h"

/* The most harts there are (NCPU in kernel/include/spinlock.h). */
#define MAX_HARTS 8

int main(int argc, char **argv) {
    (void)argv;
    if (argc > 1) {
        fprintf(STDERR, "usage: harts\n");
        return 1;
    }
    struct hartinfo info[MAX_HARTS];
    int n = harts(info, MAX_HARTS);
    if (n < 0) {
        fprintf(STDERR, "harts: %s\n", strerror(-n));
        return 1;
    }
    printf("HART   PID    TICKS SWITCHES\n");
    for (int i = 0; i < n && i < MAX_HARTS; i++) {
        struct hartinfo *h = &info[i];
        if (h->pid == 0) {
            printf("%4u %5s %8u %8u\n", h->hart, "-", h->ticks, h->switches);
        } else {
            printf("%4u %5u %8u %8u\n", h->hart, h->pid, h->ticks, h->switches);
        }
    }
    return 0;
}
