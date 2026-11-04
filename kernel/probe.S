# Memory probes: a load or store that reports a fault instead of panicking,
# for the paging self-tests now and for copying to and from user memory later.
#
# Each probe's memory access is at a global label. When that one instruction
# traps, kernel_trap (trap.c) doesn't panic: it resumes at probe_fault with
# a0 = scause and a1 = stval, and probe_fault stores stval through the probe's
# third argument and returns scause. (Linux calls this an exception table.)
#
#   uintptr probe_load(const u32 *addr, u32 *val, uintptr *stval)
#       *val = *addr; returns 0, or the scause of the fault (*val unchanged).
#   uintptr probe_store(u32 *addr, u32 val, uintptr *stval)
#       *addr = val; returns 0, or the scause of the fault.
#
# *stval is written only after a fault. scause and stval are a register wide.

#include <asm.h>

    .text

    .globl probe_load
probe_load:
    .globl probe_load_insn
probe_load_insn:
    lw    t0, 0(a0)
    sw    t0, 0(a1)
    li    a0, 0
    ret

    .globl probe_store
probe_store:
    .globl probe_store_insn
probe_store_insn:
    sw    a1, 0(a0)
    li    a0, 0
    ret

# kernel_trap resumes a faulting probe here: a0 = scause, a1 = stval, and a2
# still holds the probe's stval pointer.
    .globl probe_fault
probe_fault:
    REG_S a1, 0(a2)
    ret
