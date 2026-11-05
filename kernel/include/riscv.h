/* Privileged architecture: CSR bits (spec sections 5 and 6) and the CSR
 * accessors implemented in riscv.S (no inline asm, #15).
 *
 * A CSR is a register's width, 64 bits, so the accessors take and return a
 * uintptr, as Linux's csr_read() an unsigned long. Writing a CSR back from a
 * narrower value would clear its upper bits: sstatus's UXL (U-mode's width)
 * and SD, say. */
#ifndef CAMELOT_RISCV_H
#define CAMELOT_RISCV_H

#include "const.h"

/* Assembly includes this file too (spinlock.S), for the bits and causes:
 * the accessors are under #ifndef __ASSEMBLER__ (const.h). */
#ifndef __ASSEMBLER__
#include "types.h"
#endif

/* sstatus: S-mode's view of mstatus, the bits the kernel uses. */
#define SSTATUS_SIE (_UL(1) << 1)
#define SSTATUS_SPIE (_UL(1) << 5)
#define SSTATUS_SPP (_UL(1) << 8)

/* Interrupt numbers: bit positions in sie/sip and the low bits of scause
 * when its top bit is set (M-mode's are the firmware's). */
#define IRQ_S_SOFT 1
#define IRQ_S_TIMER 5
#define IRQ_S_EXT 9

#define SIE_SSIE (_UL(1) << IRQ_S_SOFT)
#define SIE_STIE (_UL(1) << IRQ_S_TIMER)
#define SIE_SEIE (_UL(1) << IRQ_S_EXT)
/* S-mode can clear SSIP itself, but not STIP: the SBI's set_timer does. */
#define SIP_SSIP (_UL(1) << IRQ_S_SOFT)

/* Exception causes (spec section 6). */
#define CAUSE_INSTR_MISALIGNED 0
#define CAUSE_INSTR_ACCESS_FAULT 1
#define CAUSE_ILLEGAL_INSTR 2
#define CAUSE_BREAKPOINT 3
#define CAUSE_LOAD_MISALIGNED 4
#define CAUSE_LOAD_ACCESS_FAULT 5
#define CAUSE_STORE_MISALIGNED 6
#define CAUSE_STORE_ACCESS_FAULT 7
#define CAUSE_ECALL_U 8
#define CAUSE_ECALL_S 9
#define CAUSE_INSTR_PAGE_FAULT 12
#define CAUSE_LOAD_PAGE_FAULT 13
#define CAUSE_STORE_PAGE_FAULT 15

/* Top bit of scause, bit XLEN-1 (63): the trap is an interrupt. */
#define CAUSE_INTERRUPT (_UL(1) << (__riscv_xlen - 1))

/* satp: MODE in bits 63:60 (0 Bare, 8 Sv39, 9 Sv48), ASID in 59:44 (Camelot
 * uses 0), and the root page table's physical page number in 43:0. */
#define SATP_MODE_SHIFT 60
#define SATP_MODE_SV39 (_UL(8) << SATP_MODE_SHIFT)
#define SATP_MODE_SV48 (_UL(9) << SATP_MODE_SHIFT)
#define SATP_MODE_MASK (_UL(0xf) << SATP_MODE_SHIFT)
#define SATP_PPN_MASK _ULL(0x00000fffffffffff)

#ifndef __ASSEMBLER__
/* S-mode CSRs. */
uintptr r_sstatus(void);
void w_sstatus(uintptr v);
uintptr r_sie(void);
void w_sie(uintptr v);
uintptr r_sip(void);
void w_sip(uintptr v);
uintptr r_stvec(void);
void w_stvec(uintptr v);
uintptr r_satp(void);
void w_satp(uintptr v);

/* `time` (rdtime), the CLINT's mtime: how S-mode reads the clock, with no
 * CLINT mapping (the SBI lets it, through mcounteren:
 * firmware/sbi.S's sbi_hart_init). */
u64 r_time(void);

/* sfence.vma zero, zero: orders earlier page-table writes before later
 * translations, for every address. Grail has no TLB, so this costs nothing
 * there, but the ISA requires it after changing a page table or satp. */
void sfence_vma(void);

/* Waits for an interrupt (wfi). Grail fast-forwards mtime to mtimecmp. */
void wfi(void);

/* Turn S-mode interrupts off / on (sstatus.SIE). */
void intr_off(void);
void intr_on(void);

/* One AMO each (amoswap.w, amoadd.w, amoor.w, with aq and rl): *p = v,
 * *p += v, *p |= v, as one indivisible step that another hart's accesses
 * can't come between. Each returns the old value. For words that harts
 * share without a lock (smp.c). */
u32 atomic_swap(volatile u32 *p, u32 v);
u32 atomic_add(volatile u32 *p, u32 v);
u32 atomic_or(volatile u32 *p, u32 v);

/* fence rw, rw: every load and store before it is seen by other harts
 * before any load or store after it. For the one place where two harts each
 * store a word and then load the other's (cpus[i].pagetable against a PTE,
 * smp.c), which locks and AMOs don't cover. */
void fence_rw(void);
#endif

#endif
