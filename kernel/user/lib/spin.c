/* spin: CPU time, as opposed to sleep(). */
#include "../user.h"

void spin(uint n) {
    /* volatile, so the compiler keeps the loop. */
    volatile uint i;
    for (i = 0; i < n; i++) {
    }
}
