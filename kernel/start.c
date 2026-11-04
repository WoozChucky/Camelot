/* M-mode setup. _start calls start() once on each hart: on hart 0 on the
 * boot stack, with .bss cleared; on every other hart, once hart 0 has woken
 * it (smp.c), on the stack page hart 0 gave it. When it returns, _start
 * executes mret and the hart continues in S-mode at supervisor_entry. After
 * that M-mode only runs machine_vec: the timer trampoline, IPIs and machine
 * calls (machine.S).
 *
 * M-mode does not translate addresses, and the RV64 kernel is linked high
 * but runs here at its physical address (entry.S): every address taken here
 * is pc-relative, so it is the physical one, which is what mtvec, mscratch
 * and mepc want (supervisor_entry starts with paging off). Nothing here may
 * use a pointer stored in initialised data: that would be a link address. */
#include "machine.h"
#include "riscv.h"
#include "spinlock.h"
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
 * can't be delegated, so the machine timer and the machine software
 * interrupt (an IPI) always trap to machine_vec. */
#define KERNEL_INTERRUPTS ((1u << IRQ_S_SOFT) | (1u << IRQ_S_TIMER) | (1u << IRQ_S_EXT))

uintptr machine_scratch[NCPU * MSCRATCH_WORDS];

void start(void) {
    /* tp is the hart's id already (_start). */
    u32 hart = r_tp();
    /* Traps first, so that S-mode can trap from its first instruction. On
     * RV64 supervisor_entry sets stvec itself: first to a vector at its
     * physical address, for while paging is off, then to kernel_vec at its
     * link address. */
    w_medeleg(KERNEL_EXCEPTIONS);
    w_mideleg(KERNEL_INTERRUPTS);
#if __riscv_xlen == 32
    w_stvec((uintptr)kernel_vec);
#endif
    w_mscratch((uintptr)&machine_scratch[hart * MSCRATCH_WORDS]);
    w_mtvec((uintptr)machine_vec);

    /* The machine timer and IPIs are the only M-level interrupts. M
     * interrupts are always enabled while the hart runs in S-mode, whatever
     * mstatus.MIE says. */
    timer_init();
    w_mie(MIE_MTIE | MIE_MSIE);
    /* S-mode reads the clock through the time CSR, not the CLINT, which its
     * page tables don't map (it is M-mode's, as is the test device:
     * kvminit()). */
    w_mcounteren(MCOUNTEREN_TM);

    /* mret goes to supervisor_entry in S-mode. A whole-register update: on
     * RV64 mstatus's upper half holds SXL, UXL and SD. */
    w_mstatus((r_mstatus() & ~MSTATUS_MPP_MASK) | MSTATUS_MPP_S);
    w_mepc((uintptr)supervisor_entry);
}
