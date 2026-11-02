/* Excalibur extensions (spec section 9). Implemented in entry.s, since the
 * kernel keeps custom instructions and CSR access in assembly instead of using
 * inline asm (#15). */
#ifndef CAMELOT_XCAL_H
#define CAMELOT_XCAL_H

#include "types.h"

/* xcal.dbg: print `val` on the emulator's debug console. */
void xcal_dbg(u32 val);

/* xcal.halt: stop the machine; the low 8 bits of `code` become Grail's exit
 * code. Never returns. M-mode only. */
void xcal_halt(u32 code);

#endif
