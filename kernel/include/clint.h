/* Core-local interruptor (spec section 7). Its registers are 32 bits wide, so
 * the 64-bit mtime and mtimecmp are accessed as two halves. M-mode only. */
#ifndef CAMELOT_CLINT_H
#define CAMELOT_CLINT_H

#define CLINT_BASE 0x02000000
#define CLINT_MTIMECMP_LO (CLINT_BASE + 0x4000)
#define CLINT_MTIMECMP_HI (CLINT_BASE + 0x4004)
#define CLINT_MTIME_LO (CLINT_BASE + 0xbff8)
#define CLINT_MTIME_HI (CLINT_BASE + 0xbffc)

#endif
