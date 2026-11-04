/* Memory probes (probe.s): a word load or store that returns the trap cause
 * instead of panicking when it faults. kernel_trap recognises a fault at
 * probe_load_insn or probe_store_insn and resumes at probe_fault. */
#ifndef CAMELOT_PROBE_H
#define CAMELOT_PROBE_H

#include "types.h"

/* *val = *addr. Returns 0, or the scause of the fault, with stval in *stval. */
uintptr probe_load(const u32 *addr, u32 *val, uintptr *stval);

/* *addr = val. Returns 0, or the scause of the fault, with stval in *stval. */
uintptr probe_store(u32 *addr, u32 val, uintptr *stval);

/* Labels inside probe.s, declared as functions only to take their addresses:
 * the probes' faulting instructions and where a fault resumes. */
void probe_load_insn(void);
void probe_store_insn(void);
void probe_fault(void);

#endif
