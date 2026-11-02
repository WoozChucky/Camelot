/* The kernel on several harts (spec section 10, "Kernel SMP"; smp.h).
 *
 * Booting. Grail starts hart 0 at the program's entry point and parks the
 * others in the boot ROM until an IPI (spec section 7, "Boot"). Hart 0 boots
 * the kernel alone, self-tests and all; then smp_boot() gives every other
 * hart a stack page (hart_stacks[], which _start reads) and wakes them all
 * with one MCALL_IPI. Each comes through _start and start() as hart 0 did
 * (its own M-mode trap vector, scratch block and timer), skips clearing
 * .bss, and in S-mode runs hart_main(): its struct cpu, the kernel page
 * table, its interrupts, then the scheduler, never to return.
 *
 * IPIs. S-mode can't write the CLINT, so a hart interrupts others with a
 * machine call, MCALL_IPI, which sets their msip; machine_vec on each of
 * them clears it and raises the supervisor software interrupt, the same
 * SSIP that a timer tick raises (machine.s). What the sender wants is in
 * memory, here: the only thing a hart asks of another is a TLB flush.
 *
 * TLB shootdown. A hart may cache translations from the page table in its
 * satp. After a mapping is taken away (unmap_pages), the page may be reused
 * only once no hart can still translate to it: tlb_shootdown() flushes its
 * own TLB, asks each hart that may use the table to flush its own, and
 * waits for all of them. Requests are counted per hart: tlb_asked[i] is
 * bumped (atomically, as several harts may ask at once) by each request,
 * and hart i, when it serves them, reads tlb_asked[i] first, then flushes,
 * then stores what it read in tlb_done[i]. A requester that got ticket t
 * waits until tlb_done[i] reaches t: the flush that got it there began
 * after the requester's ticket, so after its change to the table.
 *
 * Which harts may use the table is cpus[i].pagetable, read without a lock.
 * The requester stores the PTEs and then loads cpus[i].pagetable; a hart
 * switching to the table stores cpus[i].pagetable and then walks it. Each
 * side's store must be seen before its own load, or both could read the old
 * values (the requester skips the hart, which keeps the old PTE), so a full
 * fence sits in between on both sides: here, and in the scheduler and exec
 * (proc.c). A process's threads share its page table, and run on any hart,
 * several at once (spec section 10, "Kernel threads"): so a user page table
 * may be in the satp of several harts, and a shootdown of it (sbrk freeing
 * heap pages) reaches each of them, while the harts running other
 * processes are left alone. The kernel page table, in every hart's satp
 * (its second-level tables are in every process's), always reaches every
 * hart. Grail has no TLB at all, so on it every sfence.vma is free, but the
 * protocol is what real hardware needs. */
#include "smp.h"
#include "kalloc.h"
#include "kprintf.h"
#include "machine.h"
#include "memlayout.h"
#include "proc.h"
#include "riscv.h"
#include "sysinfo.h"
#include "trap.h"

u32 hart_stacks[NCPU];

/* The harts running the kernel, a bit each. Bits are only ever added
 * (atomic_or), so it is read without a lock. */
static volatile u32 harts_up;

/* TLB shootdown requests, per hart: made of it, and served by it. */
static volatile u32 tlb_asked[NCPU];
static volatile u32 tlb_done[NCPU];

/* Where the boot self-test maps its page: vmtest.c's alias address, whose
 * second-level table the kernel page table keeps, so the test adds no root
 * entry (which processes' page tables would miss, vm.h). */
#define SMP_TEST_VA 0x40000000u

u32 harts_running(void) {
    return harts_up;
}

void tlb_serve(void) {
    /* Interrupts off: a hart serving its own requests while it waits
     * (tlb_shootdown) may be interrupted by an IPI, whose handler serves
     * them too. If that one read a later count and stored it, this call
     * would store its earlier one over it, and tlb_done[me] would go back. */
    push_off();
    u32 me = cpuid();
    u32 asked = tlb_asked[me];
    if (asked != tlb_done[me]) {
        sfence_vma();
        tlb_done[me] = asked;
    }
    pop_off();
}

void tlb_shootdown(pagetable_t pt) {
    sfence_vma();
    /* The caller's PTE stores before our loads of cpus[i].pagetable (see
     * the top of the file). */
    fence_rw();
    u32 me = cpuid();
    u32 up = harts_up & ~(1u << me);
    if (up == 0) {
        return;
    }
    u32 targets = 0;
    u32 ticket[NCPU];
    for (u32 i = 0; i < NCPU; i++) {
        if ((up & (1u << i)) == 0) {
            continue;
        }
        /* cpus[i].pagetable may change as we look: a hart that switches to
         * `pt` after this check flushes as it does (the scheduler's
         * sfence.vma), which comes after the change our caller made. */
        if (pt != kernel_pagetable && cpus[i].pagetable != pt) {
            continue;
        }
        ticket[i] = atomic_add(&tlb_asked[i], 1) + 1;
        targets |= 1u << i;
    }
    if (targets == 0) {
        return;
    }
    machine_ipi(targets);
    for (u32 i = 0; i < NCPU; i++) {
        if ((targets & (1u << i)) == 0) {
            continue;
        }
        /* Signed, so the counts may wrap. Serving our own requests while
         * we wait: a hart waiting for us may be waiting with interrupts
         * off, as we are. */
        while ((i32)(tlb_done[i] - ticket[i]) < 0) {
            tlb_serve();
        }
    }
}

/* Maps a page in the kernel page table, which every hart uses, writes
 * through the mapping, and unmaps it: unmap_pages() shoots it down, so
 * every other hart in `others` must have flushed its TLB by the time it
 * returns, before the page is freed. Returns the number of failed checks. */
static int tlb_selftest(u32 others) {
    u32 before[NCPU];
    for (u32 i = 0; i < NCPU; i++) {
        before[i] = tlb_done[i];
    }
    u32 *page = kzalloc();
    if (page == NULL) {
        kprintf("smp: TLB self-test: out of memory\n");
        return 1;
    }
    if (map_pages(kernel_pagetable, SMP_TEST_VA, (u32)(uintptr)page, PGSIZE, PTE_R | PTE_W) != 0) {
        kprintf("smp: TLB self-test: out of memory\n");
        kfree(page);
        return 1;
    }
    int failed = 0;
    *(volatile u32 *)SMP_TEST_VA = 0x5ca1ab1e;
    if (page[0] != 0x5ca1ab1e) {
        kprintf("smp: TLB self-test: the test page isn't mapped\n");
        failed++;
    }
    /* Not freed by unmap_pages(): the check comes first, as it would be
     * too late once the page could be reused. */
    unmap_pages(kernel_pagetable, SMP_TEST_VA, PGSIZE, false);
    for (u32 i = 0; i < NCPU; i++) {
        if ((others & (1u << i)) != 0 && tlb_done[i] == before[i]) {
            kprintf("smp: hart %u missed the TLB shootdown\n", i);
            failed++;
        }
    }
    kfree(page);
    return failed;
}

int smp_boot(void) {
    atomic_or(&harts_up, 1u << cpuid());
    u32 n = sysinfo_read(SYS_HARTS);
    if (n > NCPU) {
        n = NCPU;
    }
    if (n <= 1) {
        return 0;
    }
    u32 wake = 0;
    u32 count = 1;
    for (u32 i = 1; i < n; i++) {
        u8 *stack = kalloc();
        if (stack == NULL) {
            kprintf("smp: no memory for hart %u's stack\n", i);
            break;
        }
        hart_stacks[i] = (u32)(uintptr)stack + PGSIZE;
        wake |= 1u << i;
        count++;
    }
    /* They come out of the boot ROM's park loop into _start. */
    machine_ipi(wake);
    while ((harts_up & wake) != wake) {
    }
    int failed = tlb_selftest(wake);
    kprintf("smp: %u harts running\n", count);
    return failed;
}

void hart_main(void) {
    /* This hart's struct cpu, which every lock uses: first of all. */
    cpu_init();
    kvminithart();
    /* Timer ticks and IPIs, both a supervisor software interrupt. The
     * devices' interrupts stay with hart 0 (plic.c), so not SEIE. */
    w_sie(r_sie() | SIE_SSIE);
    atomic_or(&harts_up, 1u << cpuid());
    /* Interrupts come on in the scheduler loop. */
    scheduler();
    panic("hart_main: the scheduler returned");
}
