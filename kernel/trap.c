/* S-mode trap handling: kernel_vec (kernelvec.s) saves the registers and calls
 * kernel_trap(). The only interrupt so far is the timer tick, which arrives as
 * a supervisor software interrupt forwarded by machine_vec. An exception is a
 * kernel bug, so it panics, unless a memory probe (probe.s) caused it. */
#include "trap.h"
#include "kprintf.h"
#include "machine.h"
#include "probe.h"
#include "riscv.h"
#include "vm.h"

/* Register numbers in struct trap_frame's regs[]. */
#define REG_A0 10
#define REG_A1 11

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
    if (f->scause == CAUSE_INSTR_PAGE_FAULT || f->scause == CAUSE_LOAD_PAGE_FAULT ||
        f->scause == CAUSE_STORE_PAGE_FAULT) {
        vm_print_mapping(f->stval);
    }
    for (int i = 1; i < 32; i++) {
        kprintf("  %3s %08x", reg_names[i], f->regs[i]);
        if (i % 4 == 0 || i == 31) {
            kprintf("\n");
        }
    }
    halt_after_panic();
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

void kernel_trap(struct trap_frame *f) {
    if (f->scause == (CAUSE_INTERRUPT | IRQ_S_SOFT)) {
        /* A timer tick from machine_vec. Clear it first: a tick that arrives
         * while this runs sets SSIP again and is taken after sret. */
        w_sip(r_sip() & ~SIP_SSIP);
        ticks = ticks + 1;
        return;
    }
    if (probe_fixup(f)) {
        return;
    }
    trap_panic(f);
}

void trap_init(void) {
    w_sie(r_sie() | SIE_SSIE);
    w_sstatus(r_sstatus() | SSTATUS_SIE);
}
