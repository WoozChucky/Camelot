/* Sleep locks (spec section 10, "Kernel locking"), after xv6's
 * sleeplock.c: a lock that may be held across a sleep, such as a buffer's
 * while the disk reads it, or an inode's while its blocks are read. A
 * process that finds it held sleeps until it is released, rather than
 * spinning, so it can only be taken by a process, never by an interrupt
 * handler, and never while holding a spinlock. */
#ifndef CAMELOT_SLEEPLOCK_H
#define CAMELOT_SLEEPLOCK_H

#include "spinlock.h"
#include "types.h"

struct sleeplock {
    /* Held, by the thread `tid` (proc.h). */
    bool locked;
    u32 tid;
    /* Protects `locked` and `tid`; the waiters sleep on the sleep lock with
     * it. */
    struct spinlock lk;
    const char *name;
};

void initsleeplock(struct sleeplock *lk, const char *name);

/* Takes `lk`, sleeping while another thread holds it. */
void acquiresleep(struct sleeplock *lk);

/* Lets go of `lk` and wakes the threads waiting for it. */
void releasesleep(struct sleeplock *lk);

/* Whether the current thread holds `lk`. */
bool holdingsleep(struct sleeplock *lk);

#endif
