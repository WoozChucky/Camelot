/* Sleep locks (sleeplock.h), after xv6's sleeplock.c. */
#include "sleeplock.h"
#include "proc.h"

void initsleeplock(struct sleeplock *lk, const char *name) {
    initlock(&lk->lk, "sleep lock");
    lk->name = name;
    lk->locked = false;
    lk->tid = 0;
}

void acquiresleep(struct sleeplock *lk) {
    acquire(&lk->lk);
    while (lk->locked) {
        sleep(lk, &lk->lk);
    }
    lk->locked = true;
    lk->tid = mythread()->tid;
    release(&lk->lk);
}

void releasesleep(struct sleeplock *lk) {
    acquire(&lk->lk);
    lk->locked = false;
    lk->tid = 0;
    wakeup(lk);
    release(&lk->lk);
}

bool holdingsleep(struct sleeplock *lk) {
    acquire(&lk->lk);
    struct thread *t = mythread();
    bool r = lk->locked && t != NULL && lk->tid == t->tid;
    release(&lk->lk);
    return r;
}
