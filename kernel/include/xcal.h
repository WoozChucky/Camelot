/* Excalibur extensions (spec section 9). Implemented in entry.S, since the
 * kernel keeps custom instructions and CSR access in assembly instead of using
 * inline asm (#15). */
#ifndef CAMELOT_XCAL_H
#define CAMELOT_XCAL_H

#include "types.h"

/* xcal.dbg: print `val` on the emulator's debug console. */
void xcal_dbg(u32 val);

/* xcal.halt is M-only: the kernel stops the machine through the SBI
 * (machine_power_off(), include/sbi.h), whose implementation on Grail ends
 * with it (machine.S). */

#endif
