/* Privileged architecture: CSR bits (spec sections 5 and 6) and the CSR
 * accessors implemented in riscv.s (no inline asm, #15). */
#ifndef CAMELOT_RISCV_H
#define CAMELOT_RISCV_H

#include "types.h"

/* mstatus / sstatus */
#define MSTATUS_SIE (1u << 1)
#define MSTATUS_MIE (1u << 3)
#define MSTATUS_SPIE (1u << 5)
#define MSTATUS_MPIE (1u << 7)
#define MSTATUS_SPP (1u << 8)
#define MSTATUS_MPP_MASK (3u << 11)
#define MSTATUS_MPP_S (1u << 11)
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

#define MIE_MTIE (1u << IRQ_M_TIMER)
#define SIE_SSIE (1u << IRQ_S_SOFT)
#define SIP_SSIP (1u << IRQ_S_SOFT)

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

/* Top bit of xcause: the trap is an interrupt. */
#define CAUSE_INTERRUPT 0x80000000u

/* satp: MODE (bit 31) = 1 selects Sv32; PPN (bits 21:0) is the physical page
 * number of the root page table. */
#define SATP_SV32 (1u << 31)

/* M-mode CSRs (only usable before start() drops to S-mode). */
u32 r_mstatus(void);
void w_mstatus(u32 v);
void w_mepc(u32 v);
void w_mtvec(u32 v);
void w_mscratch(u32 v);
u32 r_medeleg(void);
void w_medeleg(u32 v);
u32 r_mideleg(void);
void w_mideleg(u32 v);
void w_mie(u32 v);

/* S-mode CSRs. */
u32 r_sstatus(void);
void w_sstatus(u32 v);
u32 r_sie(void);
void w_sie(u32 v);
u32 r_sip(void);
void w_sip(u32 v);
u32 r_stvec(void);
void w_stvec(u32 v);
u32 r_satp(void);
void w_satp(u32 v);

/* sfence.vma zero, zero: orders earlier page-table writes before later
 * translations, for every address. Grail has no TLB, so this costs nothing
 * there, but the ISA requires it after changing a page table or satp. */
void sfence_vma(void);

/* Waits for an interrupt (wfi). Grail fast-forwards mtime to mtimecmp. */
void wfi(void);

/* Turn S-mode interrupts off / on (sstatus.SIE). */
void intr_off(void);
void intr_on(void);

#endif
