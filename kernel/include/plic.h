/* Platform-level interrupt controller (spec section 7.1): the QEMU `virt`
 * register layout, two contexts per hart. The kernel uses context 1 (hart 0
 * in S-mode), which drives hart 0's sip.SEIP: every device interrupt goes to
 * hart 0, and the other harts enable none (spec section 10, "Kernel SMP").
 * Registers are 32 bits wide. */
#ifndef CAMELOT_PLIC_H
#define CAMELOT_PLIC_H

#include "types.h"

#define PLIC_BASE 0x0c000000
#define PLIC_SIZE 0x04000000
/* Register offsets from the PLIC's base (plic.c reaches them through its
 * mapping, ioremap(), vm.h). */
#define PLIC_PRIORITY(src) (4 * (src))
#define PLIC_ENABLE(ctx) (0x2000 + 0x80 * (ctx))
#define PLIC_THRESHOLD(ctx) (0x200000 + 0x1000 * (ctx))
#define PLIC_CLAIM(ctx) (0x200004 + 0x1000 * (ctx))

/* Context 1: hart 0 in S-mode. (Context 0 is M-mode, which the kernel leaves
 * alone: mie.MEIE stays clear.) */
#define PLIC_CTX_S 1

/* Interrupt sources (spec section 7.1). Excalibur numbers its devices from 1,
 * unlike QEMU `virt`, which puts the UART on 10. Other devices' sources (the
 * disks') come from the sysinfo device table. */
#define PLIC_IRQ_UART 1

/* Threshold 0 for the S-mode context and the UART's source enabled: the
 * UART's receive line then raises sip.SEIP. */
void plic_init(void);

/* Gives source `irq` (1-31) priority 1 and enables it for the S-mode
 * context, keeping the sources already enabled. */
void plic_enable(u32 irq);

/* Claims the highest-priority pending source for the S-mode context, or 0 if
 * there is none. */
u32 plic_claim(void);

/* Tells the PLIC that the claimed source `irq` has been serviced. */
void plic_complete(u32 irq);

#endif
