/* S-mode trap handling. Traps from S-mode go to kernel_vec (kernelvec.s),
 * which saves the registers on the kernel stack and calls kernel_trap();
 * traps from U-mode go to user_vec (uservec.s), which saves them in the
 * process's trap frame and calls user_trap().
 *
 * Three interrupts are handled: the supervisor timer interrupt, the hart's
 * tick, which the SBI raises at the deadline its set_timer was given
 * (timer.c); the supervisor software interrupt, an IPI from another hart
 * (a TLB shootdown, smp.c, or a kick for an idle hart, proc.c), which the
 * SBI's send_ipi raises; and the supervisor external interrupt from the
 * PLIC, which goes to hart 0 only: the UART's receive line, the disks'
 * completions and the network card. Each may wake sleeping processes
 * (wakeup only marks them runnable). In the
 * kernel an exception is a bug, so it panics, unless a memory probe (probe.s)
 * caused it. In U-mode, ecall is a call into the kernel (syscall.c) and any
 * other exception kills the process. */
#include "trap.h"
#include "blk.h"
#include "console.h"
#include "kprintf.h"
#include "net.h"
#include "nic.h"
#include "plic.h"
#include "mmap.h"
#include "probe.h"
#include "proc.h"
#include "riscv.h"
#include "sbi.h"
#include "smp.h"
#include "syscall.h"
#include "timer.h"
#include "vm.h"

volatile u32 ticks;
struct spinlock tickslock;

/* Set while panicking, so a fault inside panic() doesn't recurse, and so
 * kprintf() stops taking the UART's lock; panic_hart is the hart that
 * panicked, PANIC_NO_HART until it has said so. In .data, not .bss: no hart
 * may ever read 0 there before one panics, as 0 is hart 0's id. */
volatile bool panicking;
volatile u32 panic_hart = PANIC_NO_HART;

static const char *const exception_names[16] = {
    "instruction address misaligned",
    "instruction access fault",
    "illegal instruction",
    "breakpoint",
    "load address misaligned",
    "load access fault",
    "store address misaligned",
    "store access fault",
    "environment call from U-mode",
    "environment call from S-mode",
    NULL,
    "environment call from M-mode",
    "instruction page fault",
    "load page fault",
    NULL,
    "store page fault",
};

static const char *const interrupt_names[12] = {
    NULL,
    "supervisor software interrupt",
    NULL,
    "machine software interrupt",
    NULL,
    "supervisor timer interrupt",
    NULL,
    "machine timer interrupt",
    NULL,
    "supervisor external interrupt",
    NULL,
    "machine external interrupt",
};

static const char *const reg_names[32] = {
    "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2",  /* x0-x7 */
    "s0", "s1", "a0", "a1", "a2", "a3", "a4", "a5",    /* x8-x15 */
    "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7",    /* x16-x23 */
    "s8", "s9", "s10", "s11", "t3", "t4", "t5", "t6", /* x24-x31 */
};

const char *trap_cause_name(uintptr cause) {
    uintptr code = cause & ~CAUSE_INTERRUPT;
    const char *name = NULL;
    if (cause & CAUSE_INTERRUPT) {
        if (code < 12) {
            name = interrupt_names[code];
        }
    } else if (code < 16) {
        name = exception_names[code];
    }
    return name != NULL ? name : "unknown cause";
}

static void halt_after_panic(void) {
    machine_power_off(PANIC_EXIT_CODE);
}

/* The start of a panic, on whichever hart: the first hart to get here
 * reports it; another that panics too meanwhile stops where it is, as do
 * the others the next time they write to the UART (uart_putc), so the
 * report comes out whole; and a panic inside the report halts at once.
 *
 * Between the swap and the store of panic_hart, `panicking` is set but
 * panic_hart is still PANIC_NO_HART, which is no hart's id: a hart that
 * looks then takes the panic for another's and stops, as it should. Hence
 * interrupts off first, so that nothing runs on this hart in between. */
static void panic_begin(void) {
    intr_off();
    if (atomic_swap((volatile u32 *)&panicking, true)) {
        if (panic_hart == cpuid()) {
            halt_after_panic();
        }
        panic_freeze();
    }
    panic_hart = cpuid();
}

void panic_freeze(void) {
    intr_off();
    for (;;) {
        wfi();
    }
}

void panic_stop_others(void) {
    if (panicking && panic_hart != cpuid()) {
        panic_freeze();
    }
}

void panic(const char *msg) {
    panic_begin();
    kprintf("panic: %s\n", msg);
    halt_after_panic();
}

/* The trap CSRs, how the active page table maps stval for a page fault, and
 * every register. */
static void trap_report(struct trap_frame *f) {
    kprintf("  scause " REG_FMT "  sepc " REG_FMT "  stval " REG_FMT "  sstatus " REG_FMT "\n", f->scause,
            f->sepc, f->stval, f->sstatus);
    /* The kernel's mappings are the same in every page table, so the active
     * one, a process's or the kernel's, shows how the kernel saw stval. */
    pagetable_t pt = vm_active_pagetable();
    if (is_page_fault(f->scause) && pt != NULL) {
        vm_print_mapping(pt, f->stval);
    }
    for (int i = 1; i < 32; i++) {
        kprintf("  %3s " REG_HEX, reg_names[i], f->regs[i]);
        if (i % 4 == 0 || i == 31) {
            kprintf("\n");
        }
    }
}

/* panic() for a trap: the cause, the trap CSRs and every register. */
static void trap_panic(struct trap_frame *f) {
    panic_begin();
    kprintf("panic: %s\n", trap_cause_name(f->scause));
    trap_report(f);
    halt_after_panic();
}

void kstack_overflow(struct trap_frame *f) {
    panic_begin();
    /* The stack sp was on, or ran out of: the KSTACK_SIZE bytes at a
     * KSTACK_SLOT boundary, which sp is less than KSTACK_SIZE above or below
     * (vm.h). */
    uintptr sp = f->regs[REG_SP];
    uintptr bottom = (sp + KSTACK_SIZE) & ~(KSTACK_SLOT - 1);
    struct thread *t = mythread();
    if (t != NULL) {
        kprintf("panic: kernel stack overflow in %s (hart %u)\n", t->proc->name, cpuid());
    } else {
        kprintf("panic: kernel stack overflow on hart %u's boot stack\n", cpuid());
    }
    kprintf("  sp " REG_FMT ", the stack " REG_FMT "-" REG_FMT "\n", sp, bottom, bottom + KSTACK_SIZE);
    kprintf("  %s\n", trap_cause_name(f->scause));
    trap_report(f);
    halt_after_panic();
}

/* A supervisor external interrupt: claim the source from the PLIC, service
 * the device, complete. sip.SEIP follows the PLIC's line, so there is nothing
 * to clear: once the claim is taken the line drops (unless another source is
 * waiting, which then traps again after sret). */
static void external_interrupt(void) {
    u32 irq = plic_claim();
    if (irq == PLIC_IRQ_UART) {
        console_intr();
    } else if (!blk_intr(irq)) {
        nic_intr(irq);
    }
    /* 0: nothing left to claim. Only enabled sources can be claimed, so any
     * other number is one the kernel set up. */
    if (irq != 0) {
        plic_complete(irq);
    }
}

/* An exception at one of the probes' memory accesses: resume at probe_fault,
 * which returns scause to the probe's caller. Returns false for any other
 * trap. */
static bool probe_fixup(struct trap_frame *f) {
    if (f->scause & CAUSE_INTERRUPT) {
        return false;
    }
    if (f->sepc != (uintptr)probe_load_insn && f->sepc != (uintptr)probe_store_insn) {
        return false;
    }
    f->regs[REG_A0] = f->scause;
    f->regs[REG_A1] = f->stval;
    f->sepc = (uintptr)probe_fault;
    return true;
}

/* `n` timer ticks on this hart (more than 1 if some were missed). Every
 * hart counts its own (for the harts system call); hart 0's are the clock:
 * `ticks`, on which processes in the sleep system call wait (with
 * tickslock), and the network's timer, which comes after, without
 * tickslock: it takes the network's lock. */
static void clock_tick(u32 n) {
    struct cpu *c = mycpu();
    c->ticks = c->ticks + n;
    if (c->hartid != 0) {
        return;
    }
    acquire(&tickslock);
    ticks = ticks + n;
    wakeup((const void *)&ticks);
    release(&tickslock);
    net_timer();
}

/* The supervisor timer interrupt: this hart's tick. The next one first,
 * which also clears this one (S-mode can't clear STIP itself: set_timer
 * does, timer.c), then the tick counted. A tick also serves any TLB flush
 * another hart asked for (smp.c): a hart that was slow to take its IPI, or
 * took it with the request not yet in sight, still flushes within a tick,
 * and the hart waiting for it isn't left waiting for good. */
static void timer_interrupt(void) {
    clock_tick(timer_next());
    tlb_serve();
}

/* The supervisor software interrupt: an IPI from another hart. Clear SSIP
 * first: one that arrives while this runs sets it again and is taken after
 * sret (and finds nothing to do, if this one did it). Then serve what other
 * harts asked for. An IPI that only woke an idle hart (kick_idle(), proc.c)
 * has nothing more to do: the hart's scheduler looks for work once wfi
 * returns. */
static void ipi_interrupt(void) {
    w_sip(r_sip() & ~SIP_SSIP);
    tlb_serve();
}

void kernel_trap(struct trap_frame *f) {
    if (f->scause == (CAUSE_INTERRUPT | IRQ_S_TIMER)) {
        timer_interrupt();
        return;
    }
    if (f->scause == (CAUSE_INTERRUPT | IRQ_S_SOFT)) {
        ipi_interrupt();
        return;
    }
    if (f->scause == (CAUSE_INTERRUPT | IRQ_S_EXT)) {
        external_interrupt();
        return;
    }
    if (probe_fixup(f)) {
        return;
    }
    trap_panic(f);
}

/* Whether `f`, a fault in U-mode, is a load or a store to a stack page not
 * mapped yet, now mapped (proc_grow_stack). Not when the page can't be had:
 * that is said, and the fault kills the process. */
static bool stack_fault(struct trap_frame *f) {
    if (f->scause != CAUSE_LOAD_PAGE_FAULT && f->scause != CAUSE_STORE_PAGE_FAULT) {
        return false;
    }
    struct proc *p = myproc();
    if (f->stval < USER_STACK_LIMIT || f->stval >= p->stack_low) {
        return false;
    }
    if (proc_grow_stack(f->stval) == 0) {
        return true;
    }
    kprintf("pid %u (%s): no memory to grow its stack to " REG_FMT "\n", p->pid, p->name, f->stval);
    return false;
}

/* Whether `f`, a page fault in U-mode, is the first touch of a page of one
 * of the process's memory mappings that allows the access, now mapped
 * (mm_fault(), mmap.h: demand paging). Any other fault in a mapping (an
 * access its protections don't allow, a page past a shared object's end,
 * no memory left) is the program's: it kills it, as Linux's SIGSEGV and
 * SIGBUS would. */
static bool area_fault(struct trap_frame *f) {
    u32 access;
    if (f->scause == CAUSE_LOAD_PAGE_FAULT) {
        access = PTE_R;
    } else if (f->scause == CAUSE_STORE_PAGE_FAULT) {
        access = PTE_W;
    } else if (f->scause == CAUSE_INSTR_PAGE_FAULT) {
        access = PTE_X;
    } else {
        return false;
    }
    if (!mm_fault(myproc()->pagetable, f->stval, access)) {
        return false;
    }
    /* A new mapping only: this hart may have cached the missing one. */
    sfence_vma();
    return true;
}

void user_trap(struct trap_frame *f) {
    /* From here on, a trap is the kernel's own. Interrupts stay off (the trap
     * cleared sstatus.SIE) until the process is back in U-mode. */
    w_stvec((uintptr)kernel_vec);
    if (f->scause == (CAUSE_INTERRUPT | IRQ_S_TIMER)) {
        /* A tick ends the process's time slice; an IPI doesn't. */
        timer_interrupt();
        yield();
    } else if (f->scause == (CAUSE_INTERRUPT | IRQ_S_SOFT)) {
        ipi_interrupt();
    } else if (f->scause == (CAUSE_INTERRUPT | IRQ_S_EXT)) {
        external_interrupt();
    } else if (f->scause == CAUSE_ECALL_U) {
        /* sepc is the ecall itself: resume after it. */
        f->sepc += 4;
        syscall(f);
    } else if (stack_fault(f)) {
        /* The stack grew to the faulting address: run the instruction again. */
    } else if (area_fault(f)) {
        /* A mapping's page, mapped now: run the instruction again. */
    } else {
        /* A fault in U-mode is the program's, not the kernel's. */
        proc_kill(f);
    }
    /* kill() takes effect here, on the way back to U-mode. */
    if (proc_killed()) {
        proc_exit(-1);
    }
    user_trap_return(f);
}

void user_trap_return(struct trap_frame *f) {
    /* No interrupt from here to sret: stvec is about to point at user_vec,
     * which only works for traps from U-mode. */
    intr_off();
    w_stvec((uintptr)user_vec);
    /* user_vec's tp for the next trap: this hart's id. */
    f->kernel_tp = cpuid();
    /* sret goes to U-mode (SPP = 0) and turns interrupts on there (SPIE). */
    f->sstatus = (r_sstatus() & ~SSTATUS_SPP) | SSTATUS_SPIE;
    user_return(f);
}

bool is_page_fault(uintptr scause) {
    return scause == CAUSE_INSTR_PAGE_FAULT || scause == CAUSE_LOAD_PAGE_FAULT ||
           scause == CAUSE_STORE_PAGE_FAULT;
}

void trap_init(void) {
    initlock(&tickslock, "ticks");
    plic_init();
    /* The timer, IPIs and devices; then the first tick, one interval from
     * now. */
    w_sie(r_sie() | SIE_STIE | SIE_SSIE | SIE_SEIE);
    timer_start();
    w_sstatus(r_sstatus() | SSTATUS_SIE);
}
