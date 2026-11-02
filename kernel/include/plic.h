/* Platform-level interrupt controller (spec section 7.1): the QEMU `virt`
 * register layout, one hart, two contexts. The kernel uses context 1 (hart 0
 * in S-mode), which drives sip.SEIP. Registers are 32 bits wide. */
#ifndef CAMELOT_PLIC_H
#define CAMELOT_PLIC_H

#include "types.h"

#define PLIC_BASE 0x0c000000
#define PLIC_PRIORITY(src) (PLIC_BASE + 4 * (src))
#define PLIC_ENABLE(ctx) (PLIC_BASE + 0x2000 + 0x80 * (ctx))
#define PLIC_THRESHOLD(ctx) (PLIC_BASE + 0x200000 + 0x1000 * (ctx))
#define PLIC_CLAIM(ctx) (PLIC_BASE + 0x200004 + 0x1000 * (ctx))

/* Context 1: hart 0 in S-mode. (Context 0 is M-mode, which the kernel leaves
 * alone: mie.MEIE stays clear.) */
#define PLIC_CTX_S 1

/* Interrupt sources (spec section 7.1). Excalibur numbers its devices from 1,
 * unlike QEMU `virt`, which puts the UART on 10. */
#define PLIC_IRQ_UART 1

/* UART source priority 1, enabled for the S-mode context, threshold 0: the
 * UART's receive line then raises sip.SEIP. */
void plic_init(void);

/* Claims the highest-priority pending source for the S-mode context, or 0 if
 * there is none. */
u32 plic_claim(void);

/* Tells the PLIC that the claimed source `irq` has been serviced. */
void plic_complete(u32 irq);

#endif
