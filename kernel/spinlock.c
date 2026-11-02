/* Spinlocks and per-CPU state (spinlock.h): the C half. acquire(),
 * release(), holding(), push_off(), pop_off() and mycpu() are in
 * spinlock.s; their error paths come here to panic. */
#include "spinlock.h"
#include "kprintf.h"
#include "trap.h"

struct cpu cpus[NCPU];

void initlock(struct spinlock *lk, const char *name) {
    lk->locked = 0;
    lk->cpu = NULL;
    lk->name = name;
}

u32 cpuid(void) {
    return r_tp();
}

void cpu_init(void) {
    struct cpu *c = mycpu();
    c->proc = NULL;
    c->thread = NULL;
    c->noff = 0;
    c->intena = 0;
    c->hartid = cpuid();
    c->pagetable = NULL;
    c->ticks = 0;
    c->switches = 0;
    c->idle = false;
}

/* The message, built here because panic() takes a plain string; it stays
 * valid since panic() never returns. */
static char spin_msg[96];

void spin_panic(const struct spinlock *lk, const char *what) {
    const char *name = lk->name != NULL ? lk->name : "?";
    if (what[0] == 'a') {
        ksnprintf(spin_msg, sizeof spin_msg, "%s: lock \"%s\" already held by hart %u", what, name, cpuid());
    } else {
        ksnprintf(spin_msg, sizeof spin_msg, "%s: lock \"%s\" not held by hart %u", what, name, cpuid());
    }
    panic(spin_msg);
}
