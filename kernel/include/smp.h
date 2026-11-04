/* Running the kernel on every hart of the machine (spec section 10, "Kernel
 * SMP"): starting the other harts, inter-processor interrupts and TLB
 * shootdown.
 *
 * Hart 0 boots the kernel alone; once its self-tests are done, smp_boot()
 * wakes every other hart Grail has (SYS_HARTS), each on a stack page of its
 * own, and they all run the scheduler, taking processes from the one shared
 * table (proc.c). Device interrupts stay with hart 0 (plic.c); each hart has
 * its own timer tick, which ends the time slice of the process it runs. */
#ifndef CAMELOT_SMP_H
#define CAMELOT_SMP_H

#include "spinlock.h"
#include "types.h"
#include "vm.h"

/* The stacks of every hart but hart 0 (which has the boot stack), set by
 * smp_boot() before it wakes the hart. hart_boot_sp[i]: the physical address
 * of the top of a page, which _start (entry.S) loads into sp for start() in
 * M-mode, which does not translate, and the jump to the high half. hart_sp[i]:
 * the top of the hart's kernel stack, which supervisor_entry loads once the
 * hart runs at its link addresses, and its scheduler runs on (on RV64 a
 * stack from kstack_alloc(), on RV32 the same page). */
extern uintptr hart_boot_sp[NCPU];
extern uintptr hart_sp[NCPU];

/* Hart 0, after the boot self-tests: wakes the other harts, waits until
 * every one of them runs the kernel, checks that each answers a TLB
 * shootdown, and prints `smp: N harts running`. Nothing at all on a
 * one-hart machine. Returns the number of failed checks. */
int smp_boot(void);

/* Every other hart, from supervisor_entry (entry.s) in S-mode: sets up its
 * struct cpu, paging and interrupts, says it is up, and runs the scheduler
 * for good. */
void hart_main(void);

/* The harts running the kernel, bit i for hart i: hart 0 from smp_boot()
 * on, every other one from when it can take IPIs. */
u32 harts_running(void);

/* After a change to page table `pt` that takes a mapping away or makes it
 * weaker: flushes this hart's TLB, then has every other hart that may hold
 * translations from `pt` flush its own (an IPI each), and waits until they
 * all have. Those are the harts whose satp is `pt` (cpus[i].pagetable: the
 * harts running a thread of the process that owns it, which may be
 * several), or
 * every hart for the kernel page table, whose second-level tables every
 * process's page table shares. Must be called holding no spinlock unless no
 * other hart can be using `pt`: a hart that spins on a lock with
 * interrupts off can't answer. */
void tlb_shootdown(pagetable_t pt);

/* Flushes this hart's TLB if another hart has asked it to (tlb_shootdown).
 * The supervisor software interrupt calls it (trap.c), and so does a hart
 * waiting for its own shootdown, so that two harts waiting for each other
 * both get through. */
void tlb_serve(void);

#endif
