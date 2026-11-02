/* ticks: prints the timer ticks since boot (10 a second; without Grail's
 * --clock, the machine skips ahead to the next tick whenever it is idle). */
#include "user.h"

int main(void) {
    printf("ticks: %u\n", uptime());
    return 0;
}
