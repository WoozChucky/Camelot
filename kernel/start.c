/* M-mode setup. _start calls start() once, on the boot stack, with .bss
 * cleared; when it returns, _start executes mret and the kernel continues in
 * S-mode at supervisor_entry. After that M-mode only runs machine_vec: the
 * timer trampoline and machine calls (machine.s). */
#include "machine.h"
#include "riscv.h"
#include "timer.h"
#include "trap.h"

/* Every exception the kernel handles itself. Left out: ecall from S-mode,
 * which is how the kernel calls M-mode (MCALL_*), and ecall from M-mode,
 * which can't be delegated. */
#define KERNEL_EXCEPTIONS                                                                                  \
    ((1u << CAUSE_INSTR_MISALIGNED) | (1u << CAUSE_INSTR_ACCESS_FAULT) | (1u << CAUSE_ILLEGAL_INSTR) |     \
     (1u << CAUSE_BREAKPOINT) | (1u << CAUSE_LOAD_MISALIGNED) | (1u << CAUSE_LOAD_ACCESS_FAULT) |          \
     (1u << CAUSE_STORE_MISALIGNED) | (1u << CAUSE_STORE_ACCESS_FAULT) | (1u << CAUSE_ECALL_U) |           \
     (1u << CAUSE_INSTR_PAGE_FAULT) | (1u << CAUSE_LOAD_PAGE_FAULT) | (1u << CAUSE_STORE_PAGE_FAULT))

/* The S-level interrupts: software, timer and external. M-level interrupts
 * can't be delegated, so the machine timer always traps to machine_vec. */
#define KERNEL_INTERRUPTS ((1u << IRQ_S_SOFT) | (1u << IRQ_S_TIMER) | (1u << IRQ_S_EXT))

u32 machine_scratch[MSCRATCH_WORDS];

void start(void) {
    /* Traps first, so that S-mode can trap from its first instruction. */
    w_medeleg(KERNEL_EXCEPTIONS);
    w_mideleg(KERNEL_INTERRUPTS);
    w_stvec((u32)(uintptr)kernel_vec);
    w_mscratch((u32)(uintptr)machine_scratch);
    w_mtvec((u32)(uintptr)machine_vec);

    /* The machine timer is the only M-level interrupt. M interrupts are always
     * enabled while the hart runs in S-mode, whatever mstatus.MIE says. */
    timer_init();
    w_mie(MIE_MTIE);

    /* mret goes to supervisor_entry in S-mode. */
    w_mstatus((r_mstatus() & ~MSTATUS_MPP_MASK) | MSTATUS_MPP_S);
    w_mepc((u32)(uintptr)supervisor_entry);
}
