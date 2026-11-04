/* Core-local interruptor (spec section 7). mtime and mtimecmp are 64-bit
 * registers: RV64 reads and writes them whole, RV32 as two 32-bit halves.
 * msip is 32-bit. M-mode only, at its physical address: the kernel's S-mode
 * code reaches it through machine calls (MCALL_IPI). */
#ifndef CAMELOT_CLINT_H
#define CAMELOT_CLINT_H

#define CLINT_BASE 0x02000000
/* Hart h's software interrupt (IPI) register and timer compare value. */
#define CLINT_MSIP(h) (CLINT_BASE + 4 * (h))
#define CLINT_MTIMECMP(h) (CLINT_BASE + 0x4000 + 8 * (h))
#define CLINT_MTIMECMP_LO(h) (CLINT_BASE + 0x4000 + 8 * (h))
#define CLINT_MTIMECMP_HI(h) (CLINT_BASE + 0x4004 + 8 * (h))
/* mtime, one for the whole machine. */
#define CLINT_MTIME (CLINT_BASE + 0xbff8)
#define CLINT_MTIME_LO (CLINT_BASE + 0xbff8)
#define CLINT_MTIME_HI (CLINT_BASE + 0xbffc)

#endif
