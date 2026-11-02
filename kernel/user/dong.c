/* dong: prints "dong 1" to "dong 4", about half a timer tick of CPU time
 * apart, and exits with status 0. See ding. */
#include "user.h"

/* spin() iterations in about half a tick (500 000 instructions: its loop is
 * five). */
#define HALF_TICK 100000

int main(void) {
    for (int i = 1; i <= 4; i++) {
        printf("dong %d\n", i);
        spin(HALF_TICK);
    }
    return 0;
}
