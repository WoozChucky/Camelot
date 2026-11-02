/* Mutexes on futexes (lib/README.md, "Threads"), after Ulrich Drepper's
 * "Futexes Are Tricky" (2011), its mutex 2. The word holds:
 *
 *   0  unlocked
 *   1  locked, nobody waiting
 *   2  locked, and some thread may be waiting in futex_wait
 *
 * An uncontended lock or unlock is one atomic instruction and no system
 * call; the kernel is asked only to sleep while the word stays 2, and to
 * wake a sleeper once it is unlocked. futex_wait checks that the word is
 * still 2 as it goes to sleep, as one step against futex_wake, so an
 * unlock that comes in between is never missed. */
#include "../user.h"

void mutex_lock(struct mutex *m) {
    int c = atomic_cas(&m->state, 0, 1);
    if (c == 0) {
        return; /* it was free: ours, with nobody waiting */
    }
    /* Taken. Say there is a waiter (2) and sleep while it stays taken; the
     * swap that finds it 0 takes it, as 2, since others may still wait. */
    if (c != 2) {
        c = atomic_swap(&m->state, 2);
    }
    while (c != 0) {
        futex_wait(&m->state, 2);
        c = atomic_swap(&m->state, 2);
    }
}

int mutex_trylock(struct mutex *m) {
    return atomic_cas(&m->state, 0, 1) == 0;
}

void mutex_unlock(struct mutex *m) {
    /* 1 -> 0: nobody waits. 2 -> 1 -> 0, and wake one waiter, which takes
     * it as 2 again (so it wakes the next one in turn). */
    if (atomic_add(&m->state, -1) != 1) {
        atomic_store(&m->state, 0);
        futex_wake(&m->state, 1);
    }
}
