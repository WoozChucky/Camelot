/* Privileged architecture: CSR bits (spec sections 5 and 6) and the CSR
 * accessors implemented in riscv.S (no inline asm, #15).
 *
 * A CSR is a register's width, XLEN bits: 32 on RV32, 64 on RV64, so the
 * accessors take and return a uintptr, as Linux's csr_read() an unsigned
 * long. Writing a CSR back from a narrower value would clear its upper
 * bits: mstatus's SXL and UXL (the S- and U-mode widths) and SD, say. */
#ifndef CAMELOT_RISCV_H
#define CAMELOT_RISCV_H

#include "types.h"

/* mstatus / sstatus */
#define MSTATUS_SIE (1ul << 1)
#define MSTATUS_MIE (1ul << 3)
#define MSTATUS_SPIE (1ul << 5)
#define MSTATUS_MPIE (1ul << 7)
#define MSTATUS_SPP (1ul << 8)
#define MSTATUS_MPP_MASK (3ul << 11)
#define MSTATUS_MPP_S (1ul << 11)
#define SSTATUS_SIE MSTATUS_SIE
#define SSTATUS_SPIE MSTATUS_SPIE
#define SSTATUS_SPP MSTATUS_SPP

/* Interrupt numbers: bit positions in mie/mip/sie/sip and the low bits of
 * xcause when its top bit is set. */
#define IRQ_S_SOFT 1
#define IRQ_M_SOFT 3
#define IRQ_S_TIMER 5
#define IRQ_M_TIMER 7
#define IRQ_S_EXT 9
#define IRQ_M_EXT 11

#define MIE_MSIE (1ul << IRQ_M_SOFT)
#define MIE_MTIE (1ul << IRQ_M_TIMER)
#define SIE_SSIE (1ul << IRQ_S_SOFT)
#define SIE_SEIE (1ul << IRQ_S_EXT)
#define SIP_SSIP (1ul << IRQ_S_SOFT)

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
#define CAUSE_ECALL_M 11
#define CAUSE_INSTR_PAGE_FAULT 12
#define CAUSE_LOAD_PAGE_FAULT 13
#define CAUSE_STORE_PAGE_FAULT 15

/* Top bit of xcause: the trap is an interrupt. Bit XLEN-1: 31 on RV32, 63
 * on RV64. */
#define CAUSE_INTERRUPT (1ul << (__riscv_xlen - 1))

#if __riscv_xlen == 64
/* satp: MODE in bits 63:60 (0 Bare, 8 Sv39, 9 Sv48), ASID in 59:44 (Camelot
 * uses 0), and the root page table's physical page number in 43:0. */
#define SATP_MODE_SHIFT 60
#define SATP_MODE_SV39 (8ul << SATP_MODE_SHIFT)
#define SATP_MODE_SV48 (9ul << SATP_MODE_SHIFT)
#define SATP_MODE_MASK (0xful << SATP_MODE_SHIFT)
#define SATP_PPN_MASK 0x00000fffffffffffull
#else
/* satp: MODE (bit 31) = 1 selects Sv32; PPN (bits 21:0) is the physical page
 * number of the root page table. */
#define SATP_SV32 (1ul << 31)
#define SATP_MODE_MASK SATP_SV32
#define SATP_PPN_MASK 0x003fffffu
#endif

/* M-mode CSRs (only usable before start() drops to S-mode). */
uintptr r_mstatus(void);
void w_mstatus(uintptr v);
void w_mepc(uintptr v);
void w_mtvec(uintptr v);
void w_mscratch(uintptr v);
uintptr r_medeleg(void);
void w_medeleg(uintptr v);
uintptr r_mideleg(void);
void w_mideleg(uintptr v);
void w_mie(uintptr v);
/* Which counters S-mode may read: bit 1 (TM) is `time`. */
void w_mcounteren(uintptr v);
#define MCOUNTEREN_TM (1ul << 1)

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

/* `time` (rdtime), the CLINT's mtime, all 64 bits (on RV32 read as two
 * halves, timeh and time, until the high half holds still): how S-mode reads
 * the clock, with no CLINT mapping (start() lets it, through mcounteren). */
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
