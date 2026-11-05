/* The kernel on several harts (spec section 10, "Kernel SMP"; smp.h).
 *
 * Booting. Hart 0 boots the kernel alone, self-tests and all, while the
 * SBI holds every other hart STOPPED (Camelot's firmware/sbi.S, or OpenSBI
 * on a board).
 * Then smp_boot() starts each of them with the SBI's hart_start, as Linux's
 * cpu_ops_sbi.c does: at secondary_entry (entry.S), in S-mode with paging
 * off, with its id in a0 and the top of a kernel stack of its own in a1.
 * There it turns paging on and runs hart_main(): its struct cpu, the kernel
 * page table, its interrupts and its tick, then the scheduler, never to
 * return. A hart the SBI can't start, or that doesn't come up within a
 * second, is reported and left out; the kernel runs on the others. One that
 * comes up after all, once it has been given up on, stops itself with the
 * SBI's hart_stop, as arm64's cpu_die_early() does, rather than join a
 * kernel that has counted it out.
 *
 * IPIs. A hart interrupts others with the SBI's send_ipi, which raises the
 * supervisor software interrupt on each of them (on Grail through their
 * msip, firmware/sbi.S). What the sender wants is in memory: a TLB flush here,
 * or a look for a runnable thread (kick_idle(), proc.c). The SBI orders
 * the sender's stores before the interrupt (#170).
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
#include "memlayout.h"
#include "proc.h"
#include "riscv.h"
#include "sbi.h"
#include "sysinfo.h"
#include "timer.h"
#include "trap.h"

/* The harts running the kernel, a bit each. Bits are only ever added
 * (atomic_or), so it is read without a lock. */
static volatile u32 harts_up;

/* Who got there first, for each started hart: the hart, saying it is in
 * hart_main (HART_ARRIVED), or hart 0, giving up on it once the second is
 * over (HART_GIVEN_UP). Each sets its bit with one atomic_or and looks at
 * the other's in the old value, so exactly one of them wins: a hart either
 * joins, and is counted, or stops. */
static volatile u32 hart_claims;
#define HART_ARRIVED(i) (1u << (i))
#define HART_GIVEN_UP(i) (1u << (16 + (i)))
_Static_assert(NCPU <= 16, "hart_claims has 16 bits a side");

/* TLB shootdown requests, per hart: made of it, and served by it. */
static volatile u32 tlb_asked[NCPU];
static volatile u32 tlb_done[NCPU];

/* Where the boot self-test maps its page: vmtest.c's alias address, whose
 * second-level table the kernel page table keeps, so the test adds no root
 * entry (which processes' page tables would miss, vm.h). */
#define SMP_TEST_VA (VM_TEST_AREA + 32 * LEVEL_SIZE(1))

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
    sbi_send_ipi(targets, 0);
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
    if (map_pages(kernel_pagetable, SMP_TEST_VA, pa(page), PGSIZE, PAGE_KERNEL) != 0) {
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
    /* Where they start, at its physical address: paging is off there. */
    phys_addr_t entry = pa(secondary_entry);
    u32 started = 0;
    for (u32 i = 1; i < n; i++) {
        /* The hart's kernel stack, which its scheduler runs on: one from the
         * vmalloc area, with its guard page (vm.h). */
        u8 *stack = kstack_alloc();
        usize size = KSTACK_SIZE;
        if (stack == NULL) {
            kprintf("smp: no memory for hart %u's stack\n", i);
            break;
        }
        long err = sbi_hart_start(i, entry, (uintptr)stack + size);
        if (err != SBI_SUCCESS) {
            kprintf("smp: hart %u failed to start (%s)\n", i, sbi_strerror(err));
            kstack_free(stack);
            continue;
        }
        started |= 1u << i;
    }
    /* Each says so in hart_main(). One that doesn't within a second may
     * have crashed, or never got going: it is left out, as Linux leaves out
     * a CPU that "failed to come online", with the state the SBI has it in
     * (START_PENDING: it never left the SBI; STARTED: it is stuck in the
     * kernel). Its stack stays where it is, as the hart may still be using
     * it; should it get to hart_main(), it stops there. */
    u64 deadline = r_time() + timebase_hz();
    while ((harts_up & started) != started && r_time() < deadline) {
    }
    for (u32 i = 1; i < n; i++) {
        if ((started & (1u << i)) == 0 || (harts_up & (1u << i)) != 0) {
            continue;
        }
        if ((atomic_or(&hart_claims, HART_GIVEN_UP(i)) & HART_ARRIVED(i)) != 0) {
            /* It got to hart_main() first, just: it joins, in a moment. */
            while ((harts_up & (1u << i)) == 0) {
            }
            continue;
        }
        kprintf("smp: hart %u failed to come online (%s)\n", i, sbi_hart_state_name(sbi_hart_get_status(i)));
        started &= ~(1u << i);
    }
    u32 up = harts_up & started;
    int failed = tlb_selftest(up);
    u32 count = 1;
    for (u32 i = 1; i < n; i++) {
        if ((up & (1u << i)) != 0) {
            count++;
        }
    }
    kprintf("smp: %u harts running\n", count);
    return failed;
}

void hart_main(void) {
    /* This hart's struct cpu, which every lock uses: first of all. */
    cpu_init();
    kvminithart();
    u32 me = cpuid();
    if ((atomic_or(&hart_claims, HART_ARRIVED(me)) & HART_GIVEN_UP(me)) != 0) {
        /* Too late: smp_boot() has gone on without this hart. Back to the
         * SBI, STOPPED, as arm64's cpu_die_early() stops a CPU that can't
         * join. If the SBI won't stop it, it is a bug, as Linux's riscv
         * sbi_cpu_stop() and arch_cpu_idle_dead() take it. */
        kprintf("smp: hart %u came online too late, stopping\n", me);
        long err = sbi_hart_stop();
        char msg[64];
        ksnprintf(msg, sizeof msg, "smp: hart %u can't stop (%s)", me, sbi_strerror(err));
        panic(msg);
    }
    /* Its tick and IPIs. The devices' interrupts stay with hart 0 (plic.c),
     * so not SEIE. */
    w_sie(r_sie() | SIE_STIE | SIE_SSIE);
    timer_start();
    atomic_or(&harts_up, 1u << me);
    /* Interrupts come on in the scheduler loop. */
    scheduler();
    panic("hart_main: the scheduler returned");
}
