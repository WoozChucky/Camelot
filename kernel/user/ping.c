/* ping: prints "ping 1" to "ping 4", about half a timer tick of CPU time
 * apart, and exits with status 0. Run beside pong (`ping & pong`), the two
 * take turns: each timer tick hands the CPU to the other one. */
#include "user.h"

/* spin() iterations in about half a tick (500 000 instructions: its loop is
 * five). */
#define HALF_TICK 100000

int main(void) {
    for (int i = 1; i <= 4; i++) {
        printf("ping %d\n", i);
        spin(HALF_TICK);
    }
    return 0;
}
