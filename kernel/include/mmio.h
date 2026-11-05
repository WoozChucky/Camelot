/* Memory-mapped I/O accessors. Every device access goes through a volatile
 * pointer so the compiler neither drops nor merges it (Merlin must honour
 * `volatile` the same way, #15). */
#ifndef CAMELOT_MMIO_H
#define CAMELOT_MMIO_H

#include "types.h"

static inline u8 mmio_read8(uintptr addr) {
    return *(volatile u8 *)addr;
}

static inline void mmio_write8(uintptr addr, u8 val) {
    *(volatile u8 *)addr = val;
}

static inline u32 mmio_read32(uintptr addr) {
    return *(volatile u32 *)addr;
}

static inline void mmio_write32(uintptr addr, u32 val) {
    *(volatile u32 *)addr = val;
}

/* A 64-bit register read or written whole, with one 8-byte access: version 2
 * of the devices' interface (spec section 9) has them, and the CLINT's
 * mtime and mtimecmp take them (#147). */
static inline u64 mmio_read64(uintptr addr) {
    return *(volatile u64 *)addr;
}

static inline void mmio_write64(uintptr addr, u64 val) {
    *(volatile u64 *)addr = val;
}

#endif
