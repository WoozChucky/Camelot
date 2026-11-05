/* The M-mode CSR fields, exception causes and interrupt numbers the firmware
 * uses (the privileged spec's, with OpenSBI's names from
 * include/sbi/riscv_encoding.h). Constants only, for assembly. */
#ifndef CAMELOT_RISCV_ENCODING_H
#define CAMELOT_RISCV_ENCODING_H

#define MSTATUS_SIE (1 << 1)
#define MSTATUS_MPP (3 << 11)
#define MSTATUS_MPP_S (1 << 11)

/* Interrupt numbers: bit i of mie, mip and mideleg; mcause's code with its
 * top bit set. */
#define IRQ_S_SOFT 1
#define IRQ_M_SOFT 3
#define IRQ_S_TIMER 5
#define IRQ_M_TIMER 7
#define IRQ_S_EXT 9

#define MIE_MSIE (1 << IRQ_M_SOFT)
#define MIE_MTIE (1 << IRQ_M_TIMER)
#define MIP_SSIP (1 << IRQ_S_SOFT)
#define MIP_STIP (1 << IRQ_S_TIMER)

/* Exception causes. */
#define CAUSE_MISALIGNED_FETCH 0
#define CAUSE_FETCH_ACCESS 1
#define CAUSE_ILLEGAL_INSTRUCTION 2
#define CAUSE_BREAKPOINT 3
#define CAUSE_MISALIGNED_LOAD 4
#define CAUSE_LOAD_ACCESS 5
#define CAUSE_MISALIGNED_STORE 6
#define CAUSE_STORE_ACCESS 7
#define CAUSE_USER_ECALL 8
#define CAUSE_SUPERVISOR_ECALL 9
#define CAUSE_FETCH_PAGE_FAULT 12
#define CAUSE_LOAD_PAGE_FAULT 13
#define CAUSE_STORE_PAGE_FAULT 15

/* The bits of mcounteren and scounteren (privileged spec, section 3.1.11):
 * the next mode down may read cycle, time, instret. */
#define COUNTEREN_CY (1 << 0)
#define COUNTEREN_TM (1 << 1)
#define COUNTEREN_IR (1 << 2)

#endif
