/* S-mode trap handling. Traps from S-mode go to kernel_vec (kernelvec.s),
 * which saves the registers on the kernel stack and calls kernel_trap();
 * traps from U-mode go to user_vec (uservec.s), which saves them in the
 * process's trap frame and calls user_trap().
 *
 * Two interrupts are handled: the timer tick, which arrives as a supervisor
 * software interrupt forwarded by machine_vec, and the supervisor external
 * interrupt from the PLIC: the UART's receive line, the disks' completions and
 * the network card. Either may wake sleeping processes (wakeup only marks them runnable). In the
 * kernel an exception is a bug, so it panics, unless a memory probe (probe.s)
 * caused it. In U-mode, ecall is a call into the kernel (syscall.c) and any
 * other exception kills the process. */
#include "trap.h"
#include "blk.h"
#include "console.h"
#include "kprintf.h"
#include "machine.h"
#include "net.h"
#include "nic.h"
#include "plic.h"
#include "probe.h"
#include "proc.h"
#include "riscv.h"
#include "syscall.h"
#include "vm.h"

volatile u32 ticks;

/* Set while panicking, so a fault inside panic() doesn't recurse. */
static bool panicking;

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

const char *trap_cause_name(u32 cause) {
    u32 code = cause & ~CAUSE_INTERRUPT;
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
    machine_halt(PANIC_EXIT_CODE);
}

void panic(const char *msg) {
    if (panicking) {
        halt_after_panic();
    }
    panicking = true;
    kprintf("panic: %s\n", msg);
    halt_after_panic();
}

/* panic() for a trap: the cause, the trap CSRs and every register. */
static void trap_panic(struct trap_frame *f) {
    if (panicking) {
        halt_after_panic();
    }
    panicking = true;
    kprintf("panic: %s\n", trap_cause_name(f->scause));
    kprintf("  scause 0x%08x  sepc 0x%08x  stval 0x%08x  sstatus 0x%08x\n", f->scause, f->sepc, f->stval,
            f->sstatus);
    /* The kernel's mappings are the same in every page table, so the active
     * one, a process's or the kernel's, shows how the kernel saw stval. */
    pagetable_t pt = vm_active_pagetable();
    if (is_page_fault(f->scause) && pt != NULL) {
        vm_print_mapping(pt, f->stval);
    }
    for (int i = 1; i < 32; i++) {
        kprintf("  %3s %08x", reg_names[i], f->regs[i]);
        if (i % 4 == 0 || i == 31) {
            kprintf("\n");
        }
    }
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
    if (f->sepc != (u32)(uintptr)probe_load_insn && f->sepc != (u32)(uintptr)probe_store_insn) {
        return false;
    }
    f->regs[REG_A0] = f->scause;
    f->regs[REG_A1] = f->stval;
    f->sepc = (u32)(uintptr)probe_fault;
    return true;
}

/* A timer tick from machine_vec. Clear it first: a tick that arrives while
 * this runs sets SSIP again and is taken after sret. Processes in the sleep
 * system call wait on `ticks`. */
static void clock_tick(void) {
    w_sip(r_sip() & ~SIP_SSIP);
    ticks = ticks + 1;
    wakeup((const void *)&ticks);
    net_timer();
}

void kernel_trap(struct trap_frame *f) {
    if (f->scause == (CAUSE_INTERRUPT | IRQ_S_SOFT)) {
        clock_tick();
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

void user_trap(struct trap_frame *f) {
    /* From here on, a trap is the kernel's own. Interrupts stay off (the trap
     * cleared sstatus.SIE) until the process is back in U-mode. */
    w_stvec((u32)(uintptr)kernel_vec);
    if (f->scause == (CAUSE_INTERRUPT | IRQ_S_SOFT)) {
        /* The end of the process's time slice. */
        clock_tick();
        yield();
    } else if (f->scause == (CAUSE_INTERRUPT | IRQ_S_EXT)) {
        external_interrupt();
    } else if (f->scause == CAUSE_ECALL_U) {
        /* sepc is the ecall itself: resume after it. */
        f->sepc += 4;
        syscall(f);
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
    w_stvec((u32)(uintptr)user_vec);
    /* sret goes to U-mode (SPP = 0) and turns interrupts on there (SPIE). */
    f->sstatus = (r_sstatus() & ~SSTATUS_SPP) | SSTATUS_SPIE;
    user_return(f);
}

bool is_page_fault(u32 scause) {
    return scause == CAUSE_INSTR_PAGE_FAULT || scause == CAUSE_LOAD_PAGE_FAULT ||
           scause == CAUSE_STORE_PAGE_FAULT;
}

void trap_init(void) {
    plic_init();
    /* Machine ticks that fired while the kernel set itself up left SSIP
     * pending: drop that one, so the first tick taken is a whole interval
     * after it arrives rather than straight away. */
    w_sip(r_sip() & ~SIP_SSIP);
    w_sie(r_sie() | SIE_SSIE | SIE_SEIE);
    w_sstatus(r_sstatus() | SSTATUS_SIE);
}
